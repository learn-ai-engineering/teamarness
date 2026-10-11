//! Private Workspace Person name scope. Only Core may select this record scope.
use super::*;
use crate::person_names::{self, edit};
use crate::synchronization::{self, MembershipStanding};

const SCOPE: &str = synchronization::reserved_person_name_scope();
const PURPOSE: &str = "person-name.edit";

pub(super) fn initialize(
    state: &mut RuntimeState,
    workspace: WorkspaceId,
) -> Result<(), crate::SynchronizationError> {
    let Some(projection) = state.memberships.get(&workspace) else {
        return Ok(());
    };
    let local = PublicIdentity(state.secret_key.public());
    if !projection.contains_member(local) {
        return Ok(());
    }
    if projection.creator() == local {
        let secret = state.secret_key.clone();
        if synchronization::declare_creator_scope(&mut state.connection, &secret, workspace, SCOPE)?
        {
            synchronization::record_subscription::publish_scope_exists(state, workspace, SCOPE);
        }
    } else {
        synchronization::declare_member_key_demand(&state.connection, workspace, SCOPE)?;
    }
    state.sync_wake.notify_one();
    reconcile(state, workspace)
}

/// Replays at most one bounded page per lock hold; the existing LogSync worker
/// reload continues any backlog. The immutable journal, not receipt order, is authority.
pub(super) fn reconcile(
    state: &mut RuntimeState,
    workspace: WorkspaceId,
) -> Result<(), crate::SynchronizationError> {
    let result = reconcile_page(state, workspace);
    // Earlier rows in a page may have committed before a later storage error.
    publish_generation(state, workspace);
    result
}

fn reconcile_page(
    state: &mut RuntimeState,
    workspace: WorkspaceId,
) -> Result<(), crate::SynchronizationError> {
    let Some(projection) = state.memberships.get(&workspace).cloned() else {
        return Ok(());
    };
    let scope = synchronization::scope_digest(SCOPE, workspace);
    // No key means no record can be authenticated or consumed yet.
    if synchronization::record_subscription::journal_of(&state.connection, scope)?.is_none() {
        return Ok(());
    }
    let position: i64 = state
        .connection
        .query_row(
            "SELECT delivery_sequence FROM workspace_person_name_consumption WHERE workspace_id=?1",
            [workspace.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| crate::SynchronizationError::Storage(StorageError::Database))?
        .unwrap_or(0);
    'page: {
        let page: Vec<(i64, Vec<u8>)> = {
            let mut query = state.connection.prepare(
                "SELECT d.delivery_sequence, r.sealed_record FROM record_delivery d JOIN immutable_records r ON r.record_id=d.record_id WHERE d.scope_id=?1 AND d.delivery_sequence>?2 ORDER BY d.delivery_sequence LIMIT 32"
            ).map_err(|_| crate::SynchronizationError::Storage(StorageError::Database))?;
            query
                .query_map(rusqlite::params![scope.as_slice(), position], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .map_err(|_| crate::SynchronizationError::Storage(StorageError::Database))?
                .collect::<Result<_, _>>()
                .map_err(|_| crate::SynchronizationError::Storage(StorageError::Database))?
        };
        if page.is_empty() {
            break 'page;
        }
        for (sequence, sealed) in &page {
            #[cfg(test)]
            let injected_error = std::mem::take(&mut state.person_name_open_error_once);
            #[cfg(not(test))]
            let injected_error = false;
            let opened = if injected_error {
                Err(crate::SynchronizationError::Storage(StorageError::Database))
            } else {
                synchronization::open_records(
                    &state.connection,
                    workspace,
                    SCOPE,
                    std::slice::from_ref(sealed),
                    state.clock.now_unix_ms(),
                )
            };
            match opened {
                Ok(opened) => {
                    let record = &opened[0];
                    if record.purpose() == PURPOSE
                        && let Ok(edit_bytes) = edit::PersonNameEdit::decode(record.canonical())
                        && edit_bytes.author() == record.author()
                    {
                        match edit::accept(&mut state.connection, &projection, record.canonical()) {
                            Ok(_)
                            | Err(edit::EditError::InvalidEdit | edit::EditError::Unauthorized) => {
                            }
                            Err(edit::EditError::Storage(error)) => {
                                return Err(crate::SynchronizationError::Storage(error));
                            }
                        }
                    }
                }
                Err(
                    crate::SynchronizationError::CorruptRecord
                    | crate::SynchronizationError::WrongScope
                    | crate::SynchronizationError::InvalidInput,
                ) => {}
                Err(error) => return Err(error),
            }
            state.connection.execute(
                "INSERT INTO workspace_person_name_consumption (workspace_id, delivery_sequence) VALUES (?1, ?2) ON CONFLICT(workspace_id) DO UPDATE SET delivery_sequence=excluded.delivery_sequence",
                rusqlite::params![workspace.as_bytes().as_slice(), sequence],
            ).map_err(|_| crate::SynchronizationError::Storage(StorageError::Database))?;
        }
        if page.len() == 32 {
            state.sync_wake.notify_one();
        }
    }
    flush_pending(state, workspace)?;
    Ok(())
}

/// The LogSync worker calls this on its blocking reload thread, one Workspace
/// and one page at a time. A full page schedules the next reload.
pub(crate) fn continue_replay(state: &mut RuntimeState) {
    let local = PublicIdentity(state.secret_key.public());
    let workspaces: Vec<_> = state
        .memberships
        .iter()
        .filter(|(_, projection)| projection.contains_member(local))
        .map(|(workspace, _)| *workspace)
        .collect();
    for workspace in workspaces {
        let scope = synchronization::scope_digest(SCOPE, workspace);
        let outstanding: bool = state.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM record_delivery d WHERE d.scope_id=?1 AND d.delivery_sequence > COALESCE((SELECT delivery_sequence FROM workspace_person_name_consumption WHERE workspace_id=?2), 0))",
            rusqlite::params![scope.as_slice(), workspace.as_bytes().as_slice()], |row| row.get(0),
        ).unwrap_or(false);
        if outstanding {
            match reconcile(state, workspace) {
                Ok(()) => state.sync_wake.notify_one(), // another Workspace may have a backlog
                Err(error) => eprintln!("private name replay deferred after error: {error}"),
            }
            break;
        }
    }
}

fn flush_pending(
    state: &mut RuntimeState,
    workspace: WorkspaceId,
) -> Result<(), crate::SynchronizationError> {
    let Some(projection) = state.memberships.get(&workspace).cloned() else {
        return Ok(());
    };
    let local = PublicIdentity(state.secret_key.public());
    let Some(anchor) = person_names::anchor(&projection, local) else {
        return Ok(());
    };
    let person = person_names::derive(workspace, anchor);
    let Some(intent) =
        edit::pending_intent(&state.connection, workspace, person).map_err(edit_error)?
    else {
        return Ok(());
    };
    let counter = edit::next_counter(&state.connection, workspace, person).map_err(edit_error)?;
    let signed =
        edit::PersonNameEdit::sign(&state.secret_key, workspace, person, counter, &intent.name)
            .map_err(edit_error)?;
    let secret = state.secret_key.clone();
    match synchronization::append(
        &mut state.connection,
        &secret,
        workspace,
        SCOPE,
        PURPOSE,
        signed.canonical_bytes(),
        MembershipStanding::Current,
        projection.creator() == local,
        state.clock.now_unix_ms(),
    ) {
        Ok((_, sequence)) => {
            synchronization::record_subscription::publish_commit(state, workspace, SCOPE, sequence);
            edit::accept(&mut state.connection, &projection, signed.canonical_bytes())
                .map_err(edit_error)?;
            // A superseding local intent or higher winner remains pending for the next wake.
            edit::complete_pending(&mut state.connection, &projection, &signed, intent.revision)
                .map_err(edit_error)?;
            state.sync_wake.notify_one();
        }
        Err(crate::SynchronizationError::ContentKeyUnavailable) => {
            synchronization::declare_member_key_demand(&state.connection, workspace, SCOPE)?;
            state.sync_wake.notify_one();
        }
        Err(error) => return Err(error),
    }
    Ok(())
}

fn edit_error(error: edit::EditError) -> crate::SynchronizationError {
    match error {
        edit::EditError::Storage(storage) => crate::SynchronizationError::Storage(storage),
        _ => crate::SynchronizationError::InvalidInput,
    }
}

pub(super) fn publish_generation(state: &RuntimeState, workspace: WorkspaceId) {
    if let Some(sender) = state.profile_generations.get(&workspace)
        && let Ok(generation) = edit::profile_generation(&state.connection, workspace)
    {
        sender.send_modify(|value| *value = generation);
    }
}
