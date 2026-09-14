use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use p2panda_core::Hash;
use rusqlite::Connection;

use crate::error::{StorageError, SubscriptionError};
use crate::{
    MembershipEpoch, MembershipOperationId, MembershipSnapshotDigest, MembershipTransition,
    MembershipVersion, WorkspaceId,
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
            "SELECT epoch, membership_version, cause_operation_id, snapshot_digest
             FROM membership_transitions
             WHERE workspace_id = ?1 AND epoch > ?2 ORDER BY epoch LIMIT ?3",
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
                ))
            },
        )
        .map_err(|_| SubscriptionError::Storage(StorageError::Database))?;
    let mut transitions = Vec::new();
    for row in rows {
        let (epoch, version, cause, digest) =
            row.map_err(|_| SubscriptionError::Storage(StorageError::Database))?;
        transitions.push(MembershipTransition::new(
            workspace_id,
            MembershipEpoch::new(
                u64::try_from(epoch)
                    .map_err(|_| SubscriptionError::Storage(StorageError::CorruptWorkspace))?,
            ),
            MembershipVersion::from_bytes(fixed(&version)?),
            MembershipOperationId::from_hash(Hash::from_bytes(fixed(&cause)?)),
            MembershipSnapshotDigest::from_bytes(fixed(&digest)?),
        ));
    }
    Ok(transitions)
}

fn fixed<const N: usize>(bytes: &[u8]) -> Result<[u8; N], SubscriptionError> {
    bytes
        .try_into()
        .map_err(|_| SubscriptionError::Storage(StorageError::CorruptWorkspace))
}

use rusqlite::OptionalExtension;
