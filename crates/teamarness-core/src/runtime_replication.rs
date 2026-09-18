//! What the runtime does once a membership operation has been accepted.
//!
//! Admitting an Installation makes its parked records applicable; revoking one
//! must end its live sessions. Both reactions live here, with the promotion of
//! parked operations, so `runtime.rs` keeps only the capability surface.

use super::{CoreRuntime, MembershipChange, MembershipTransition, PublicIdentity, RuntimeState};
use crate::WorkspaceId;
use crate::error::StorageError;
use crate::synchronization::SynchronizationError;
use crate::synchronization::parking::{self, ParkedOperation};
use crate::synchronization::{ImmutableRecord, RemoteInsert, RemoteInsertError};

/// Applies everything parked for `author` now that its admission is known.
pub(super) fn promote_author(
    state: &mut RuntimeState,
    workspace: WorkspaceId,
    author: PublicIdentity,
) -> Result<usize, SynchronizationError> {
    let parked = parking::parked_for_author(&state.connection, workspace, author)?;
    promote(state, workspace, parked)
}

/// Applies everything parked for one scope now that its content key is held.
pub(super) fn promote_scope(
    state: &mut RuntimeState,
    workspace: WorkspaceId,
    package: &str,
) -> Result<usize, SynchronizationError> {
    let parked = parking::parked_for_scope(&state.connection, workspace, package)?;
    promote(state, workspace, parked)
}

/// Each promotion is its own transaction, so a crash between the insert and the
/// delete is repaired on the next pass: the re-insert reports a duplicate and
/// the parked row is dropped then.
fn promote(
    state: &mut RuntimeState,
    workspace: WorkspaceId,
    parked: Vec<ParkedOperation>,
) -> Result<usize, SynchronizationError> {
    let mut applied = 0;
    for item in parked {
        match crate::synchronization::insert_remote_operation(
            &mut state.connection,
            workspace,
            &item.package,
            &item.operation,
        ) {
            Ok(RemoteInsert::Applied(record)) => {
                parking::remove(&state.connection, item.operation_hash)?;
                publish_record(state, workspace, &item.package, record);
                applied += 1;
            }
            Ok(RemoteInsert::Duplicate(_)) => {
                parking::remove(&state.connection, item.operation_hash)?;
            }
            // Still unapplicable: the scope's key has not arrived yet.
            Err(RemoteInsertError::MissingScope) => {}
            Err(RemoteInsertError::Storage) => {
                return Err(SynchronizationError::Storage(StorageError::Database));
            }
            Err(_) => parking::remove(&state.connection, item.operation_hash)?,
        }
    }
    Ok(applied)
}

fn publish_record(
    state: &mut RuntimeState,
    workspace: WorkspaceId,
    package: &str,
    record: ImmutableRecord,
) {
    let Some(event) =
        crate::synchronization::event_for_record(&state.connection, workspace, package, record)
    else {
        return;
    };
    if let Some(listeners) = state
        .record_subscribers
        .get_mut(&(workspace, package.to_owned()))
    {
        listeners.retain(|sender| sender.send(event.clone()).is_ok());
    }
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
        MembershipChange::Revoked { member } => state.peer_sessions.cancel(member),
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
}
