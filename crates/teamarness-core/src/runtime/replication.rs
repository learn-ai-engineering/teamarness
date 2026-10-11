//! What the runtime does once a membership operation has been accepted.
//!
//! Admitting an Installation makes its parked records applicable; revoking one
//! must end its live sessions. Both reactions live here, with the promotion of
//! parked operations, so `runtime.rs` keeps only the capability surface.

use super::{CoreRuntime, MembershipChange, MembershipTransition, PublicIdentity, RuntimeState};
use crate::WorkspaceId;
use crate::error::StorageError;
use crate::membership::replication::Disclosure;
use crate::membership_sync::{ApplyOutcome, MembershipReplica, MembershipSummary, ReplicaError};
use crate::synchronization::SynchronizationError;
use crate::synchronization::parking::{self, ParkedOperation};
use crate::synchronization::{RemoteInsert, RemoteInsertError};

/// Applies everything parked for `author` now that its admission is known.
pub(super) fn promote_author(
    state: &mut RuntimeState,
    workspace: WorkspaceId,
    author: PublicIdentity,
) -> Result<usize, SynchronizationError> {
    let parked = parking::parked_for_author(&state.connection, workspace, author)?;
    let now = state.clock.now_unix_ms();
    promote(state, workspace, parked, now)
}

/// Applies everything parked for one scope now that its content key is held.
pub(super) fn promote_scope(
    state: &mut RuntimeState,
    workspace: WorkspaceId,
    package: &str,
) -> Result<usize, SynchronizationError> {
    let parked = parking::parked_for_scope(&state.connection, workspace, package)?;
    let now = state.clock.now_unix_ms();
    promote(state, workspace, parked, now)
}

/// Each promotion is its own transaction, so a crash between the insert and the
/// delete is repaired on the next pass: the re-insert reports a duplicate and
/// the parked row is dropped then.
fn promote(
    state: &mut RuntimeState,
    workspace: WorkspaceId,
    parked: Vec<ParkedOperation>,
    now_unix_ms: i64,
) -> Result<usize, SynchronizationError> {
    let mut applied = 0;
    for item in parked {
        match crate::synchronization::insert_remote_operation(
            &mut state.connection,
            workspace,
            &item.package,
            &item.operation,
            now_unix_ms,
        ) {
            Ok(RemoteInsert::Applied {
                delivery_sequence, ..
            }) => {
                parking::remove(&state.connection, item.operation_hash)?;
                crate::synchronization::record_subscription::publish_commit(
                    state,
                    workspace,
                    &item.package,
                    delivery_sequence,
                );
                if crate::synchronization::is_reserved_record_scope(&item.package) {
                    let _ = super::person_name_sync::reconcile(state, workspace);
                }
                applied += 1;
            }
            Ok(RemoteInsert::Duplicate(_)) => {
                parking::remove(&state.connection, item.operation_hash)?;
            }
            // Still unapplicable: the scope's key has not arrived yet.
            Err(RemoteInsertError::MissingScope) => {}
            // The window may open as this scope's own heights grow, so the
            // operation is kept until its time-to-live expires.
            Err(RemoteInsertError::HeightOutOfRange) => {}
            Err(RemoteInsertError::Storage) => {
                return Err(SynchronizationError::Storage(StorageError::Database));
            }
            Err(_) => parking::remove(&state.connection, item.operation_hash)?,
        }
    }
    Ok(applied)
}

/// The single reaction to any accepted membership transition.
///
/// Every path that accepts an operation — local authoring, invitation
/// redemption, quarantine maintenance, and remote application — goes through
/// here, so an admission always promotes and a revocation always cancels.
pub(super) fn after_membership_accepted(
    state: &mut RuntimeState,
    transition: &MembershipTransition,
) {
    CoreRuntime::publish_transition(state, transition.clone());
    let workspace = transition.workspace_id();
    match transition.change() {
        MembershipChange::Revoked { member } => {
            state.peer_sessions.cancel(member);
            if member.as_bytes() == state.secret_key.public().as_bytes()
                && let Err(error) = state.connection.execute(
                    "DELETE FROM sync_scope_key_demands WHERE workspace_id = ?1",
                    [workspace.as_bytes().as_slice()],
                )
            {
                eprintln!("pending scope demand cleanup after local revocation failed: {error}");
            }
        }
        MembershipChange::Admitted { member } => {
            if let Err(error) = promote_author(state, workspace, member) {
                eprintln!("parked records for a newly admitted author were not applied: {error}");
            }
            // Dial the new Member promptly instead of waiting for the idle tick.
            let _ = state.connection.execute(
                "UPDATE durable_wakes SET wake_generation = wake_generation + 1
                 WHERE scope_id IN (SELECT scope_id FROM sync_scopes WHERE workspace_id = ?1)",
                [workspace.as_bytes().as_slice()],
            );
        }
    }
    if let Err(error) = super::person_name_sync::initialize(state, workspace) {
        eprintln!("Person name scope activation after membership transition failed: {error}");
    }
    super::person_name_sync::publish_generation(state, workspace);
    // Who the peers are changed, so what every subscription of this
    // Workspace's scopes reports may have changed with it.
    crate::synchronization::record_subscription::bump_workspace_scopes(state, workspace);
    // A membership stream waits on this watch rather than polling.
    if let Some(sender) = state.membership_epochs.get(&workspace) {
        let epoch = transition.epoch().value();
        sender.send_modify(|value| *value = (*value).max(epoch));
    }
    // The peer set changed, so what is due changed; the scheduler reloads
    // rather than waiting for its idle tick.
    state.sync_wake.notify_one();
}

/// Applies quarantined operations that have become applicable and publishes
/// every transition they produce.
pub(super) fn maintain_and_publish(
    state: &mut RuntimeState,
    now_unix_ms: i64,
) -> Result<(), crate::MembershipError> {
    let applied = match crate::membership::projection::maintain_quarantine(
        &mut state.connection,
        &mut state.memberships,
        now_unix_ms,
    ) {
        Ok(applied) => applied,
        Err(error) => {
            if matches!(
                error,
                crate::MembershipError::Storage(StorageError::UncertainOutcome)
            ) {
                state.poisoned = true;
            }
            return Err(error);
        }
    };
    for applied in applied {
        if let Some(transition) = CoreRuntime::transition_from_applied(applied) {
            after_membership_accepted(state, &transition);
        }
    }
    Ok(())
}

/// The Core side of `teamarness/membership-sync/1`.
///
/// Every call takes the runtime lock briefly and runs on a blocking thread, so
/// a replication session never holds the runtime mutex on a Tokio worker.
pub(crate) struct RuntimeMembershipReplica {
    state: std::sync::Weak<std::sync::Mutex<RuntimeState>>,
    clock: std::sync::Arc<dyn crate::runtime::Clock>,
    #[cfg(test)]
    paused: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl std::fmt::Debug for RuntimeMembershipReplica {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeMembershipReplica")
            .finish_non_exhaustive()
    }
}

impl RuntimeMembershipReplica {
    pub(crate) fn new(
        state: std::sync::Weak<std::sync::Mutex<RuntimeState>>,
        clock: std::sync::Arc<dyn crate::runtime::Clock>,
        #[cfg(test)] paused: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self {
            state,
            clock,
            #[cfg(test)]
            paused,
        }
    }

    /// Takes the runtime lock and runs quarantine maintenance first, so every
    /// replica answer reflects the same state a local command would see.
    fn with_state<T>(
        &self,
        call: impl FnOnce(&mut RuntimeState, i64) -> Result<T, ReplicaError>,
    ) -> Result<T, ReplicaError> {
        #[cfg(test)]
        if self.paused.load(std::sync::atomic::Ordering::Acquire) {
            return Err(ReplicaError::Unavailable);
        }
        let state = self.state.upgrade().ok_or(ReplicaError::Unavailable)?;
        let mut state = state.lock().map_err(|_| ReplicaError::Unavailable)?;
        if state.poisoned {
            return Err(ReplicaError::Unavailable);
        }
        let now_unix_ms = self.clock.now_unix_ms();
        if maintain_and_publish(&mut state, now_unix_ms).is_err() {
            return Err(ReplicaError::Storage);
        }
        call(&mut state, now_unix_ms)
    }
}

impl MembershipReplica for RuntimeMembershipReplica {
    fn summary(&self, workspace: WorkspaceId) -> Result<Option<MembershipSummary>, ReplicaError> {
        self.with_state(|state, _| {
            Ok(state
                .memberships
                .get(&workspace)
                .map(|projection| MembershipSummary {
                    version: *projection.snapshot().version().as_bytes(),
                    operation_ids: crate::membership::replication::operation_ids(projection),
                }))
        })
    }

    fn disclosure(
        &self,
        workspace: WorkspaceId,
        peer: PublicIdentity,
        peer_has: std::collections::BTreeSet<crate::MembershipOperationId>,
    ) -> Result<Disclosure, ReplicaError> {
        self.with_state(|state, _| {
            let projection = state
                .memberships
                .get(&workspace)
                .ok_or(ReplicaError::Invalid)?;
            crate::membership::replication::disclosure(projection, peer, &peer_has)
                .map_err(|_| ReplicaError::Storage)
        })
    }

    fn apply(
        &self,
        _peer: PublicIdentity,
        workspace: WorkspaceId,
        canonical_envelope: Vec<u8>,
    ) -> Result<ApplyOutcome, ReplicaError> {
        self.with_state(|state, now_unix_ms| {
            let envelope = crate::membership::authenticate(&canonical_envelope)
                .map_err(|_| ReplicaError::Invalid)?;
            if envelope.workspace_id != workspace {
                return Err(ReplicaError::Invalid);
            }
            // Replication never creates a Workspace: onboarding is the only
            // path that imports one.
            if !state.memberships.contains_key(&workspace) {
                return Err(ReplicaError::Invalid);
            }
            let result = {
                let RuntimeState {
                    connection,
                    memberships,
                    ..
                } = &mut *state;
                crate::membership::projection::apply_canonical(
                    connection,
                    memberships,
                    &canonical_envelope,
                    now_unix_ms,
                )
            };
            match result {
                Ok(applied) if applied.accepted => {
                    if let Some(transition) = CoreRuntime::transition_from_applied(applied) {
                        after_membership_accepted(state, &transition);
                    }
                    Ok(ApplyOutcome::Accepted)
                }
                Ok(applied) if applied.quarantined => Ok(ApplyOutcome::Quarantined),
                Ok(_) => Ok(ApplyOutcome::Duplicate),
                Err(crate::MembershipError::Storage(StorageError::UncertainOutcome)) => {
                    state.poisoned = true;
                    Err(ReplicaError::Storage)
                }
                Err(crate::MembershipError::Storage(_)) => Err(ReplicaError::Storage),
                Err(_) => Err(ReplicaError::Invalid),
            }
        })
    }
}
