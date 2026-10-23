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
    let affected_keys = conflict_affected_keys(connection, workspace_id)?;
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
        let (mut change, author) = change_from_envelope(&envelope)?;
        if matches!(change, MembershipChange::Admitted { member } if affected_keys.contains(&member))
        {
            let snapshot = snapshot_at_epoch(connection, workspace_id, epoch)?;
            change = crate::membership::projection::effective_change_from_action(
                &envelope.action,
                &snapshot,
            )
            .ok_or(SubscriptionError::Storage(StorageError::CorruptWorkspace))?;
        }
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

fn conflict_affected_keys(
    connection: &Connection,
    workspace_id: WorkspaceId,
) -> Result<std::collections::BTreeSet<PublicIdentity>, SubscriptionError> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut statement = connection
        .prepare(
            "SELECT canonical_envelope FROM membership_operations
         WHERE workspace_id = ?1 AND action IN (1, 4, 5)",
        )
        .map_err(|_| SubscriptionError::Storage(StorageError::Database))?;
    let rows = statement
        .query_map([workspace_id.as_bytes().as_slice()], |row| {
            row.get::<_, Vec<u8>>(0)
        })
        .map_err(|_| SubscriptionError::Storage(StorageError::Database))?;
    let mut claimed = BTreeSet::new();
    let mut conflicted = BTreeSet::new();
    let mut person_groups: BTreeMap<PublicIdentity, BTreeSet<MembershipOperationId>> =
        BTreeMap::new();
    let mut agent_groups = Vec::new();
    for row in rows {
        let bytes = row.map_err(|_| SubscriptionError::Storage(StorageError::Database))?;
        let operation = crate::membership::authenticate(&bytes)
            .map_err(|_| SubscriptionError::Storage(StorageError::CorruptWorkspace))?;
        let key = match operation.action {
            crate::membership::MembershipAction::Admit { target, proof } => {
                person_groups
                    .entry(target)
                    .or_default()
                    .insert(proof.claim.create_person.id);
                Some(target)
            }
            crate::membership::MembershipAction::AttachPerson { .. } => Some(operation.author),
            crate::membership::MembershipAction::AddAgentKey { agent, group, .. } => {
                agent_groups.push((agent, group));
                Some(agent)
            }
            _ => None,
        };
        if let Some(key) = key
            && !claimed.insert(key)
        {
            conflicted.insert(key);
        }
    }
    let conflicted_groups = person_groups
        .iter()
        .filter(|(key, _)| conflicted.contains(key))
        .flat_map(|(_, groups)| groups.iter().copied())
        .collect::<BTreeSet<_>>();
    for (agent, group) in agent_groups {
        if conflicted_groups.contains(&group) {
            conflicted.insert(agent);
        }
    }
    Ok(conflicted)
}

/// Reconstructs only the requested transition's accepted prefix. The durable
/// cause/epoch order is already verified on open; no arrival-order winner or
/// new persisted change field is needed for a conflict's effective event.
fn snapshot_at_epoch(
    connection: &Connection,
    workspace_id: WorkspaceId,
    epoch: i64,
) -> Result<crate::MembershipSnapshot, SubscriptionError> {
    use std::collections::BTreeMap;
    #[cfg(test)]
    PREFIX_REPLAYS.with(|count| count.set(count.get() + 1));
    let (creator, genesis): (Vec<u8>, Vec<u8>) = connection.query_row(
        "SELECT creator_public_identity, genesis_operation_id FROM workspaces WHERE workspace_id = ?1",
        [workspace_id.as_bytes().as_slice()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).map_err(|_| SubscriptionError::Storage(StorageError::CorruptWorkspace))?;
    let creator = PublicIdentity::from_bytes(fixed(&creator)?)
        .map_err(|_| SubscriptionError::Storage(StorageError::CorruptWorkspace))?;
    let genesis = MembershipOperationId::from_hash(Hash::from_bytes(fixed(&genesis)?));
    let mut statement = connection
        .prepare(
            "SELECT o.canonical_envelope FROM membership_transitions t
         JOIN membership_operations o ON o.operation_id = t.cause_operation_id
         WHERE t.workspace_id = ?1 AND t.epoch <= ?2 ORDER BY t.epoch",
        )
        .map_err(|_| SubscriptionError::Storage(StorageError::Database))?;
    let rows = statement
        .query_map(
            rusqlite::params![workspace_id.as_bytes().as_slice(), epoch],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .map_err(|_| SubscriptionError::Storage(StorageError::Database))?;
    let mut operations = BTreeMap::new();
    let genesis_bytes: Vec<u8> = connection
        .query_row(
            "SELECT genesis_envelope FROM workspaces WHERE workspace_id = ?1",
            [workspace_id.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .map_err(|_| SubscriptionError::Storage(StorageError::CorruptWorkspace))?;
    let genesis_envelope = crate::membership::authenticate(&genesis_bytes)
        .map_err(|_| SubscriptionError::Storage(StorageError::CorruptWorkspace))?;
    operations.insert(genesis, genesis_envelope);
    for row in rows {
        let bytes = row.map_err(|_| SubscriptionError::Storage(StorageError::Database))?;
        let envelope = crate::membership::authenticate(&bytes)
            .map_err(|_| SubscriptionError::Storage(StorageError::CorruptWorkspace))?;
        operations.insert(envelope.id, envelope);
    }
    crate::membership::projection::replay(workspace_id, creator, genesis, operations)
        .map(|projection| projection.snapshot())
        .map_err(|_| SubscriptionError::Storage(StorageError::CorruptWorkspace))
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
thread_local! {
    static PREFIX_REPLAYS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
mod tests {
    use iroh_base::SecretKey;

    use crate::invitation::{sign_claim, sign_token};
    use crate::membership::{AdmissionProof, MembershipAction, MembershipEnvelope};
    use crate::{MembershipOperationId, PublicIdentity, WorkspaceId};

    use super::{MembershipChange, SubscriptionError, change_from_envelope};

    #[tokio::test]
    async fn durable_catch_up_replays_only_conflict_subjects() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};

        use crate::membership::test_authoring::{
            TestAction, identity_of, operation_id, sign_admission, sign_operation,
        };
        use crate::{
            CoreRuntime, CoreRuntimeConfig, MembershipApplyOutcomeForTests, MembershipEpoch,
        };

        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "teamarness-subscription-catch-up-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
            .await
            .unwrap();
        let workspace = runtime.create_workspace("Catch up").unwrap().id();
        let creator = runtime.installation_secret_for_tests();
        let genesis = runtime.genesis_id_for_tests(workspace);
        let now = i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap();
        let mut groups = Vec::new();
        for (index, steward) in [[31; 32], [32; 32]].iter().enumerate() {
            let (create, admit) = sign_admission(
                &creator,
                steward,
                workspace,
                genesis,
                now,
                now + 100_000,
                [u8::try_from(index + 1).unwrap(); 32],
                now,
                &runtime.membership_heads_for_tests(workspace),
            );
            runtime
                .apply_membership_operation_for_tests(&create)
                .unwrap();
            runtime
                .apply_membership_operation_for_tests(&admit)
                .unwrap();
            groups.push((operation_id(&create), operation_id(&admit)));
        }
        let contested = [33; 32];
        for (index, steward) in [[31; 32], [32; 32]].iter().enumerate() {
            let add = sign_operation(
                steward,
                workspace,
                now + 1,
                TestAction::AddAgentKey {
                    group: groups[index].0,
                    agent_secret: contested,
                },
                &[groups[index].1],
            );
            assert!(matches!(
                runtime.apply_membership_operation_for_tests(&add).unwrap(),
                MembershipApplyOutcomeForTests::Accepted(_)
            ));
        }
        for index in 0..8 {
            let invitee = [u8::try_from(index + 50).unwrap(); 32];
            let (create, admit) = sign_admission(
                &creator,
                &invitee,
                workspace,
                genesis,
                now,
                now + 100_000,
                [u8::try_from(index + 10).unwrap(); 32],
                now + i64::from(index) + 2,
                &runtime.membership_heads_for_tests(workspace),
            );
            runtime
                .apply_membership_operation_for_tests(&create)
                .unwrap();
            runtime
                .apply_membership_operation_for_tests(&admit)
                .unwrap();
        }
        runtime.shutdown().await.unwrap();
        drop(runtime);
        let connection = rusqlite::Connection::open(directory.join("core.sqlite3")).unwrap();
        super::PREFIX_REPLAYS.with(|count| count.set(0));
        let transitions =
            super::load_after(&connection, workspace, MembershipEpoch::new(0)).unwrap();
        assert_eq!(transitions.len(), 12);
        assert_eq!(
            transitions[3].change(),
            MembershipChange::Revoked {
                member: identity_of(&contested)
            }
        );
        for (index, transition) in transitions.iter().skip(4).enumerate() {
            assert_eq!(
                transition.change(),
                MembershipChange::Admitted {
                    member: identity_of(&[u8::try_from(index + 50).unwrap(); 32])
                }
            );
        }
        assert_eq!(super::PREFIX_REPLAYS.with(std::cell::Cell::get), 2);
        super::PREFIX_REPLAYS.with(|count| count.set(0));
        let bounded =
            super::load_after_bounded(&connection, workspace, transitions[3].epoch(), 3).unwrap();
        assert_eq!(bounded.len(), 3);
        assert_eq!(super::PREFIX_REPLAYS.with(std::cell::Cell::get), 0);
        drop(connection);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn durable_catch_up_includes_agents_of_conflicted_person() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};

        use crate::membership::test_authoring::{
            TestAction, identity_of, operation_id, sign_admission, sign_operation,
        };
        use crate::{CoreRuntime, CoreRuntimeConfig, MembershipEpoch};

        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "teamarness-subscription-dependent-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
            .await
            .unwrap();
        let workspace = runtime.create_workspace("Dependent agent").unwrap().id();
        let creator = runtime.installation_secret_for_tests();
        let genesis = runtime.genesis_id_for_tests(workspace);
        let now = i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap();
        let steward = [41; 32];
        let shared = [42; 32];
        let child = [43; 32];
        let (create_steward, admit_steward) = sign_admission(
            &creator,
            &steward,
            workspace,
            genesis,
            now,
            now + 100_000,
            [1; 32],
            now,
            &runtime.membership_heads_for_tests(workspace),
        );
        runtime
            .apply_membership_operation_for_tests(&create_steward)
            .unwrap();
        runtime
            .apply_membership_operation_for_tests(&admit_steward)
            .unwrap();
        let (create_shared, admit_shared) = sign_admission(
            &creator,
            &shared,
            workspace,
            genesis,
            now,
            now + 100_000,
            [2; 32],
            now + 2,
            &runtime.membership_heads_for_tests(workspace),
        );
        let add_shared = sign_operation(
            &steward,
            workspace,
            now + 1,
            TestAction::AddAgentKey {
                group: operation_id(&create_steward),
                agent_secret: shared,
            },
            &[operation_id(&admit_steward)],
        );
        runtime
            .apply_membership_operation_for_tests(&add_shared)
            .unwrap();
        runtime
            .apply_membership_operation_for_tests(&create_shared)
            .unwrap();
        runtime
            .apply_membership_operation_for_tests(&admit_shared)
            .unwrap();
        let add_child = sign_operation(
            &shared,
            workspace,
            now + 3,
            TestAction::AddAgentKey {
                group: operation_id(&create_shared),
                agent_secret: child,
            },
            &[operation_id(&admit_shared)],
        );
        runtime
            .apply_membership_operation_for_tests(&add_child)
            .unwrap();
        runtime.shutdown().await.unwrap();
        drop(runtime);
        let connection = rusqlite::Connection::open(directory.join("core.sqlite3")).unwrap();
        super::PREFIX_REPLAYS.with(|count| count.set(0));
        let transitions =
            super::load_after(&connection, workspace, MembershipEpoch::new(0)).unwrap();
        assert_eq!(transitions.len(), 4);
        assert_eq!(
            transitions[1].change(),
            MembershipChange::Admitted {
                member: identity_of(&shared)
            }
        );
        assert_eq!(
            transitions[2].change(),
            MembershipChange::Revoked {
                member: identity_of(&shared)
            }
        );
        assert_eq!(
            transitions[3].change(),
            MembershipChange::Revoked {
                member: identity_of(&child)
            }
        );
        assert_eq!(super::PREFIX_REPLAYS.with(std::cell::Cell::get), 3);
        drop(connection);
        std::fs::remove_dir_all(directory).unwrap();
    }

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
