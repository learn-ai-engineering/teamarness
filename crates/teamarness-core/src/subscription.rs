use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use p2panda_core::Hash;
use rusqlite::Connection;

use crate::error::{StorageError, SubscriptionError};
use crate::membership::MembershipEnvelope;
use crate::{
    MembershipChange, MembershipEpoch, MembershipOperationId, MembershipSnapshotDigest,
    MembershipTransition, MembershipVersion, PublicIdentity, WorkspaceId,
};

pub(crate) type TransitionSender = Sender<MembershipTransition>;

#[derive(Debug)]
pub(crate) struct SubscriptionState {
    durable: VecDeque<MembershipTransition>,
    live: Receiver<MembershipTransition>,
    last_epoch: MembershipEpoch,
}

impl SubscriptionState {
    pub(crate) fn new(
        durable: Vec<MembershipTransition>,
        live: Receiver<MembershipTransition>,
        after_epoch: MembershipEpoch,
    ) -> Self {
        Self {
            durable: durable.into(),
            live,
            last_epoch: after_epoch,
        }
    }

    pub(crate) fn next(&mut self) -> Option<MembershipTransition> {
        loop {
            let transition = if let Some(transition) = self.durable.pop_front() {
                transition
            } else {
                self.live.recv().ok()?
            };
            if transition.epoch() <= self.last_epoch {
                continue;
            }
            self.last_epoch = transition.epoch();
            return Some(transition);
        }
    }

    pub(crate) fn next_timeout(&mut self, timeout: Duration) -> Option<MembershipTransition> {
        loop {
            let transition = if let Some(transition) = self.durable.pop_front() {
                transition
            } else {
                match self.live.recv_timeout(timeout) {
                    Ok(transition) => transition,
                    Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return None,
                }
            };
            if transition.epoch() <= self.last_epoch {
                continue;
            }
            self.last_epoch = transition.epoch();
            return Some(transition);
        }
    }
}

pub(crate) fn load_after(
    connection: &Connection,
    workspace_id: WorkspaceId,
    after_epoch: MembershipEpoch,
) -> Result<Vec<MembershipTransition>, SubscriptionError> {
    load_after_with_limit(connection, workspace_id, after_epoch, None)
}

pub(crate) fn load_after_bounded(
    connection: &Connection,
    workspace_id: WorkspaceId,
    after_epoch: MembershipEpoch,
    limit: usize,
) -> Result<Vec<MembershipTransition>, SubscriptionError> {
    load_after_with_limit(
        connection,
        workspace_id,
        after_epoch,
        Some(limit.clamp(1, 32)),
    )
}

fn load_after_with_limit(
    connection: &Connection,
    workspace_id: WorkspaceId,
    after_epoch: MembershipEpoch,
    limit: Option<usize>,
) -> Result<Vec<MembershipTransition>, SubscriptionError> {
    let current_epoch: Option<i64> = connection
        .query_row(
            "SELECT epoch FROM workspace_membership_state WHERE workspace_id = ?1",
            [workspace_id.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| SubscriptionError::Storage(StorageError::Database))?;
    let current_epoch = current_epoch.ok_or(SubscriptionError::WorkspaceNotFound)?;
    if after_epoch.value()
        > u64::try_from(current_epoch)
            .map_err(|_| SubscriptionError::Storage(StorageError::CorruptWorkspace))?
    {
        return Err(SubscriptionError::CursorAhead);
    }

    let after = i64::try_from(after_epoch.value()).map_err(|_| SubscriptionError::CursorAhead)?;
    let mut statement = connection
        .prepare(
            "SELECT t.epoch, t.membership_version, t.cause_operation_id, t.snapshot_digest,
                    o.canonical_envelope
             FROM membership_transitions t
             JOIN membership_operations o ON o.operation_id = t.cause_operation_id
             WHERE t.workspace_id = ?1 AND t.epoch > ?2 AND o.action IN (1, 2, 5, 6)
             ORDER BY t.epoch LIMIT ?3",
        )
        .map_err(|_| SubscriptionError::Storage(StorageError::Database))?;
    let rows = statement
        .query_map(
            rusqlite::params![
                workspace_id.as_bytes().as_slice(),
                after,
                limit
                    .and_then(|value| i64::try_from(value).ok())
                    .unwrap_or(i64::MAX)
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                ))
            },
        )
        .map_err(|_| SubscriptionError::Storage(StorageError::Database))?;
    let mut transitions = Vec::new();
    for row in rows {
        let (epoch, version, cause, digest, canonical_envelope) =
            row.map_err(|_| SubscriptionError::Storage(StorageError::Database))?;
        let cause = MembershipOperationId::from_hash(Hash::from_bytes(fixed(&cause)?));
        let envelope = crate::membership::authenticate(&canonical_envelope)
            .map_err(|_| SubscriptionError::Storage(StorageError::CorruptWorkspace))?;
        if envelope.id != cause || envelope.workspace_id != workspace_id {
            return Err(SubscriptionError::Storage(StorageError::CorruptWorkspace));
        }
        let (change, author) = change_from_envelope(&envelope)?;
        transitions.push(MembershipTransition::new(
            workspace_id,
            MembershipEpoch::new(
                u64::try_from(epoch)
                    .map_err(|_| SubscriptionError::Storage(StorageError::CorruptWorkspace))?,
            ),
            MembershipVersion::from_bytes(fixed(&version)?),
            cause,
            MembershipSnapshotDigest::from_bytes(fixed(&digest)?),
            change,
            author,
        ));
    }
    Ok(transitions)
}

/// Derives a durable transition's change and author from the canonical bytes of
/// the operation that caused it. A `create` operation causes no transition, so
/// finding one stored as a cause means the workspace is corrupt.
fn change_from_envelope(
    envelope: &MembershipEnvelope,
) -> Result<(MembershipChange, PublicIdentity), SubscriptionError> {
    let change = crate::membership::projection::change_from_action(&envelope.action)
        .ok_or(SubscriptionError::Storage(StorageError::CorruptWorkspace))?;
    Ok((change, envelope.author))
}

fn fixed<const N: usize>(bytes: &[u8]) -> Result<[u8; N], SubscriptionError> {
    bytes
        .try_into()
        .map_err(|_| SubscriptionError::Storage(StorageError::CorruptWorkspace))
}

use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests {
    use iroh_base::SecretKey;

    use crate::invitation::{sign_claim, sign_token};
    use crate::membership::{AdmissionProof, MembershipAction, MembershipEnvelope};
    use crate::{MembershipOperationId, PublicIdentity, WorkspaceId};

    use super::{MembershipChange, SubscriptionError, change_from_envelope};

    fn secret(byte: u8) -> SecretKey {
        SecretKey::from_bytes(&[byte; 32])
    }

    fn envelope(author: &SecretKey, action: MembershipAction) -> MembershipEnvelope {
        MembershipEnvelope::sign(
            author,
            WorkspaceId::from_bytes([3; 16]),
            1_100,
            action,
            vec![MembershipOperationId::digest(b"genesis")],
        )
        .unwrap()
    }

    #[test]
    fn change_is_derived_from_the_cause_envelope() {
        let manager = secret(7);
        let author = PublicIdentity::from_bytes(*manager.public().as_bytes()).unwrap();

        let create = envelope(&manager, MembershipAction::Create);
        assert!(matches!(
            change_from_envelope(&create),
            Err(SubscriptionError::Storage(
                crate::error::StorageError::CorruptWorkspace
            ))
        ));

        let token = sign_token(
            &manager,
            WorkspaceId::from_bytes([3; 16]),
            1_000,
            2_000,
            [9; 32],
        )
        .unwrap();
        let claim = sign_claim(
            &secret(8),
            &token,
            MembershipOperationId::digest(b"genesis"),
        )
        .unwrap();
        let joiner = claim.invitee;
        let admit = envelope(
            &manager,
            MembershipAction::Admit {
                target: joiner,
                proof: Box::new(AdmissionProof { claim }),
            },
        );
        assert_eq!(
            change_from_envelope(&admit).unwrap(),
            (MembershipChange::Admitted { member: joiner }, author)
        );

        let revoke = envelope(
            &manager,
            MembershipAction::Revoke {
                target: joiner,
                group: MembershipOperationId::digest(b"group"),
            },
        );
        assert_eq!(
            change_from_envelope(&revoke).unwrap(),
            (MembershipChange::Revoked { member: joiner }, author)
        );
    }
}
