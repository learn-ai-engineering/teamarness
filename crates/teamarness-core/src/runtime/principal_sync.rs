//! Private Workspace Person name scope. Only Core may select this record scope.
use super::*;
use crate::principals::{self, edit};
use crate::synchronization::{self, MembershipStanding};
use std::collections::BTreeSet;

// A scan is triggered by an accepted membership transition or a new reference.
// Unknown references are visited once per scan, in bounded pages, not on every
// scheduler reload; a later claim starts a new scan so no target can starve.
pub(super) struct ReplayScan {
    known: BTreeSet<[u8; 32]>,
    after: Option<i64>,
}

fn restart_scan(
    state: &mut RuntimeState,
    workspace: WorkspaceId,
) -> Result<(), crate::SynchronizationError> {
    let pending: bool = state
        .connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM deferred_principal_names WHERE workspace_id=?1)",
            [workspace.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .map_err(|_| crate::SynchronizationError::Storage(StorageError::Database))?;
    if pending {
        let known = state.memberships[&workspace]
            .principal_history()
            .into_iter()
            .map(|id| *id.as_bytes())
            .collect();
        state.name_replay_scans.insert(
            workspace,
            ReplayScan {
                known,
                after: Some(0),
            },
        );
    } else {
        state.name_replay_scans.remove(&workspace);
    }
    Ok(())
}

const SCOPE: &str = synchronization::reserved_principals_scope();
const PURPOSE: &str = "principal-name.edit";

pub(super) fn initialize(
    state: &mut RuntimeState,
    workspace: WorkspaceId,
) -> Result<(), crate::SynchronizationError> {
    #[cfg(any(test, feature = "test-hooks"))]
    if state.principal_sync_init_faults > 0 {
        state.principal_sync_init_faults -= 1;
        return Err(crate::SynchronizationError::Storage(StorageError::Database));
    }
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
    restart_scan(state, workspace)?;
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
            "SELECT delivery_sequence FROM principal_name_consumption WHERE workspace_id=?1",
            [workspace.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| crate::SynchronizationError::Storage(StorageError::Database))?
        .unwrap_or(0);
    let mut consumed = 0;
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
        consumed = page.len();
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
                        && let Ok(edit_bytes) = edit::PrincipalNameEdit::decode(record.canonical())
                        && edit_bytes.author() == record.author()
                    {
                        match edit::accept(&mut state.connection, &projection, record.canonical()) {
                            Ok(_) => {}
                            Err(edit::EditError::Unauthorized)
                                if edit::is_early_target(&projection, &edit_bytes)
                                    && edit_bytes.workspace() == workspace =>
                            {
                                // The reference and cursor are one durable decision. A failed
                                // insert cannot silently consume the only replay opportunity.
                                let transaction = state.connection.transaction().map_err(|_| {
                                    crate::SynchronizationError::Storage(StorageError::Database)
                                })?;
                                transaction.execute(
                                    "INSERT OR IGNORE INTO deferred_principal_names (workspace_id, delivery_sequence, principal_id) VALUES (?1, ?2, ?3)",
                                    rusqlite::params![workspace.as_bytes().as_slice(), sequence, edit_bytes.target().as_bytes().as_slice()],
                                ).map_err(|_| crate::SynchronizationError::Storage(StorageError::Database))?;
                                advance_cursor(&transaction, workspace, *sequence)?;
                                transaction.commit().map_err(|_| {
                                    crate::SynchronizationError::Storage(
                                        StorageError::UncertainOutcome,
                                    )
                                })?;
                                if let Some(scan) = state.name_replay_scans.get_mut(&workspace) {
                                    scan.after = Some(0);
                                } else {
                                    restart_scan(state, workspace)?;
                                }
                                continue;
                            }
                            Err(edit::EditError::InvalidEdit | edit::EditError::Unauthorized) => {}
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
            advance_cursor(&state.connection, workspace, *sequence)?;
        }
        if page.len() == 32 {
            state.sync_wake.notify_one();
        }
    }
    replay_eligible(state, workspace, &projection, 32 - consumed)?;
    flush_pending(state, workspace)?;
    Ok(())
}

fn advance_cursor(
    connection: &rusqlite::Connection,
    workspace: WorkspaceId,
    sequence: i64,
) -> Result<(), crate::SynchronizationError> {
    connection.execute(
        "INSERT INTO principal_name_consumption (workspace_id, delivery_sequence) VALUES (?1, ?2) ON CONFLICT(workspace_id) DO UPDATE SET delivery_sequence=excluded.delivery_sequence",
        rusqlite::params![workspace.as_bytes().as_slice(), sequence],
    ).map_err(|_| crate::SynchronizationError::Storage(StorageError::Database))?;
    Ok(())
}

struct DeferredDelivery {
    sequence: i64,
    target: Vec<u8>,
    sealed: Vec<u8>,
}

fn eligible_page(
    connection: &rusqlite::Connection,
    workspace: WorkspaceId,
    after: i64,
    limit: usize,
) -> Result<Vec<DeferredDelivery>, crate::SynchronizationError> {
    let mut query = connection.prepare(
        "SELECT d.delivery_sequence, d.principal_id, r.sealed_record FROM deferred_principal_names d JOIN record_delivery delivery ON delivery.delivery_sequence=d.delivery_sequence JOIN immutable_records r ON r.record_id=delivery.record_id WHERE d.workspace_id=?1 AND d.delivery_sequence>?2 ORDER BY d.delivery_sequence LIMIT ?3"
    ).map_err(|_| crate::SynchronizationError::Storage(StorageError::Database))?;
    query
        .query_map(
            rusqlite::params![workspace.as_bytes().as_slice(), after, limit as i64],
            |row| {
                Ok(DeferredDelivery {
                    sequence: row.get(0)?,
                    target: row.get(1)?,
                    sealed: row.get(2)?,
                })
            },
        )
        .map_err(|_| crate::SynchronizationError::Storage(StorageError::Database))?
        .collect::<Result<_, _>>()
        .map_err(|_| crate::SynchronizationError::Storage(StorageError::Database))
}

fn replay_eligible(
    state: &mut RuntimeState,
    workspace: WorkspaceId,
    projection: &crate::membership::WorkspaceProjection,
    limit: usize,
) -> Result<(), crate::SynchronizationError> {
    if limit == 0 {
        return Ok(());
    }
    let Some(scan) = state.name_replay_scans.get(&workspace) else {
        return Ok(());
    };
    let Some(after) = scan.after else {
        return Ok(());
    };
    let page = eligible_page(&state.connection, workspace, after, limit)?;
    for delivery in &page {
        if !<[u8; 32]>::try_from(delivery.target.as_slice())
            .ok()
            .is_some_and(|id| state.name_replay_scans[&workspace].known.contains(&id))
        {
            state.name_replay_scans.get_mut(&workspace).unwrap().after = Some(delivery.sequence);
            continue;
        }
        let opened = synchronization::open_records(
            &state.connection,
            workspace,
            SCOPE,
            std::slice::from_ref(&delivery.sealed),
            state.clock.now_unix_ms(),
        );
        match opened {
            Ok(records) => {
                let record = &records[0];
                if record.purpose() == PURPOSE
                    && let Ok(decoded) = edit::PrincipalNameEdit::decode(record.canonical())
                    && decoded.author() == record.author()
                {
                    match edit::accept(&mut state.connection, projection, record.canonical()) {
                        Ok(_)
                        | Err(edit::EditError::InvalidEdit | edit::EditError::Unauthorized) => {}
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
        state.connection.execute("DELETE FROM deferred_principal_names WHERE workspace_id=?1 AND delivery_sequence=?2", rusqlite::params![workspace.as_bytes().as_slice(), delivery.sequence])
            .map_err(|_| crate::SynchronizationError::Storage(StorageError::Database))?;
        state.name_replay_scans.get_mut(&workspace).unwrap().after = Some(delivery.sequence);
    }
    if page.len() == limit {
        state.sync_wake.notify_one();
    } else {
        state.name_replay_scans.get_mut(&workspace).unwrap().after = None;
    }
    Ok(())
}

/// The LogSync worker calls this on its blocking reload thread, one Workspace
/// and one page at a time. A full page schedules the next reload.
pub(crate) fn retry_pending_initialization(state: &mut RuntimeState) {
    let workspaces: Vec<_> = state.memberships.keys().copied().collect();
    for workspace in workspaces {
        let pending: bool = state
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pending_principal_name_edits WHERE workspace_id=?1)",
                [workspace.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap_or(false);
        if pending && let Err(error) = initialize(state, workspace) {
            eprintln!("private name initialization deferred: {error}");
        }
    }
}

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
            "SELECT EXISTS(SELECT 1 FROM record_delivery d WHERE d.scope_id=?1 AND d.delivery_sequence > COALESCE((SELECT delivery_sequence FROM principal_name_consumption WHERE workspace_id=?2), 0))",
            rusqlite::params![scope.as_slice(), workspace.as_bytes().as_slice()], |row| row.get(0),
        ).unwrap_or(false);
        let eligible = state
            .name_replay_scans
            .get(&workspace)
            .is_some_and(|scan| scan.after.is_some());
        if outstanding || eligible {
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
    for (id, _, kind, steward, _) in principals::registry(&projection) {
        let owned = match kind {
            principals::PrincipalKind::Person => {
                principals::lookup_id(&state.connection, &projection, id)
                    .map_err(|_| crate::SynchronizationError::Storage(StorageError::Database))?
                    .is_some_and(|row| row.keys().contains(&local))
            }
            principals::PrincipalKind::Agent => steward.is_some_and(|person| {
                principals::registry(&projection)
                    .iter()
                    .any(|(candidate, key, _, _, _)| *candidate == person && *key == local)
            }),
        };
        if owned {
            flush_principal(state, &projection, workspace, id, local)?;
        }
    }
    Ok(())
}

fn flush_principal(
    state: &mut RuntimeState,
    projection: &crate::membership::WorkspaceProjection,
    workspace: WorkspaceId,
    person: principals::PrincipalId,
    local: PublicIdentity,
) -> Result<(), crate::SynchronizationError> {
    let Some(intent) =
        edit::pending_intent(&state.connection, workspace, person).map_err(edit_error)?
    else {
        return Ok(());
    };
    let counter = edit::next_counter(&state.connection, workspace, person).map_err(edit_error)?;
    let signed =
        edit::PrincipalNameEdit::sign(&state.secret_key, workspace, person, counter, &intent.name)
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
            edit::accept(&mut state.connection, projection, signed.canonical_bytes())
                .map_err(edit_error)?;
            // A superseding local intent or higher winner remains pending for the next wake.
            edit::complete_pending(&mut state.connection, projection, &signed, intent.revision)
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
