//! Erasing one Workspace from this Installation (ADR 0048).
//!
//! Erasure is a single transaction. It writes the Workspace's deletion marker, which is what
//! lets the append-only triggers accept deletes for this Workspace and no other, deletes every
//! row the Workspace owns children-first, verifies that nothing of it is left, then deletes the
//! marker. Either the whole Workspace is gone when it commits, or none of it is.

use rusqlite::{Connection, TransactionBehavior, params};

use crate::WorkspaceId;
use crate::diag::fault;
use crate::error::{StorageError, WorkspaceError};

/// Rows owned through one of the Workspace's synchronization scopes, deleted children-first.
/// `?1` is the Workspace.
const SCOPE_ROWS: &[&str] = &[
    "DELETE FROM p2panda_projection_recovery WHERE teamarness_operation_id IN
        (SELECT o.operation_id FROM immutable_operations o
         JOIN sync_scopes s ON s.scope_id = o.scope_id WHERE s.workspace_id = ?1)",
    "DELETE FROM pending_records WHERE operation_id IN
        (SELECT o.operation_id FROM immutable_operations o
         JOIN sync_scopes s ON s.scope_id = o.scope_id WHERE s.workspace_id = ?1)",
    "DELETE FROM p2panda_operations WHERE scope_id IN (SELECT scope_id FROM sync_scopes WHERE workspace_id = ?1)",
    "DELETE FROM immutable_author_heads WHERE scope_id IN (SELECT scope_id FROM sync_scopes WHERE workspace_id = ?1)",
    "DELETE FROM immutable_idempotency WHERE scope_id IN (SELECT scope_id FROM sync_scopes WHERE workspace_id = ?1)",
    "DELETE FROM deferred_principal_names WHERE workspace_id = ?1",
    "DELETE FROM principal_name_consumption WHERE workspace_id = ?1",
    "DELETE FROM record_delivery WHERE scope_id IN (SELECT scope_id FROM sync_scopes WHERE workspace_id = ?1)",
    "DELETE FROM sealed_envelopes WHERE scope_id IN (SELECT scope_id FROM sync_scopes WHERE workspace_id = ?1)",
    "DELETE FROM quarantined_records WHERE scope_id IN (SELECT scope_id FROM sync_scopes WHERE workspace_id = ?1)",
    "DELETE FROM immutable_records WHERE scope_id IN (SELECT scope_id FROM sync_scopes WHERE workspace_id = ?1)",
    "DELETE FROM immutable_operation_dependencies WHERE operation_id IN
        (SELECT o.operation_id FROM immutable_operations o
         JOIN sync_scopes s ON s.scope_id = o.scope_id WHERE s.workspace_id = ?1)",
    "DELETE FROM immutable_operations WHERE scope_id IN (SELECT scope_id FROM sync_scopes WHERE workspace_id = ?1)",
    "DELETE FROM protection_control_operations WHERE scope_id IN (SELECT scope_id FROM sync_scopes WHERE workspace_id = ?1)",
    "DELETE FROM protection_grant_slots WHERE scope_id IN (SELECT scope_id FROM sync_scopes WHERE workspace_id = ?1)",
    "DELETE FROM protection_pre_keys WHERE scope_id IN (SELECT scope_id FROM sync_scopes WHERE workspace_id = ?1)",
    "DELETE FROM protection_state WHERE scope_id IN (SELECT scope_id FROM sync_scopes WHERE workspace_id = ?1)",
    "DELETE FROM quarantined_scopes WHERE scope_id IN (SELECT scope_id FROM sync_scopes WHERE workspace_id = ?1)",
    "DELETE FROM scope_validation WHERE scope_id IN (SELECT scope_id FROM sync_scopes WHERE workspace_id = ?1)",
    "DELETE FROM sync_peer_progress WHERE scope_id IN (SELECT scope_id FROM sync_scopes WHERE workspace_id = ?1)",
    "DELETE FROM durable_wakes WHERE scope_id IN (SELECT scope_id FROM sync_scopes WHERE workspace_id = ?1)",
    "DELETE FROM sync_scope_key_demand_peer_progress WHERE scope_id IN
        (SELECT scope_id FROM sync_scope_key_demands WHERE workspace_id = ?1)",
    "DELETE FROM sync_scope_key_demands WHERE workspace_id = ?1",
    "DELETE FROM sync_scopes WHERE workspace_id = ?1",
];

/// Rows owned directly by the Workspace, deleted children-first. `?1` is the Workspace.
const WORKSPACE_ROWS: &[&str] = &[
    "DELETE FROM parked_operations WHERE workspace_id = ?1",
    "DELETE FROM future_membership_dependencies WHERE operation_id IN
        (SELECT operation_id FROM future_membership_operations WHERE workspace_id = ?1)",
    "DELETE FROM future_membership_operations WHERE workspace_id = ?1",
    "DELETE FROM agent_package_bindings WHERE workspace_id = ?1",
    "DELETE FROM agent_keys WHERE workspace_id = ?1",
    "DELETE FROM pending_principal_name_edits WHERE workspace_id = ?1",
    "DELETE FROM principal_name_intent_revisions WHERE workspace_id = ?1",
    "DELETE FROM principal_name_winners WHERE workspace_id = ?1",
    "DELETE FROM principal_name_generations WHERE workspace_id = ?1",
    "DELETE FROM pending_principal_join_names WHERE workspace_id = ?1",
    "DELETE FROM principal_names WHERE workspace_id = ?1",
    "DELETE FROM workspace_principals WHERE workspace_id = ?1",
    "DELETE FROM membership_peer_progress WHERE workspace_id = ?1",
    "DELETE FROM membership_snapshot_members WHERE workspace_id = ?1",
    "DELETE FROM membership_sync_wakes WHERE workspace_id = ?1",
    "DELETE FROM workspace_membership_state WHERE workspace_id = ?1",
    "DELETE FROM used_invitation_nonces WHERE workspace_id = ?1",
    "DELETE FROM membership_transitions WHERE workspace_id = ?1",
    "DELETE FROM membership_dependencies WHERE operation_id IN
        (SELECT operation_id FROM membership_operations WHERE workspace_id = ?1)",
    "DELETE FROM membership_operations WHERE workspace_id = ?1",
    "DELETE FROM workspace_creation_requests WHERE workspace_id = ?1",
    "DELETE FROM workspaces WHERE workspace_id = ?1",
];

/// Deletes every stored row of `workspace_id`. The caller has already checked, under the
/// runtime lock it still holds, that this Installation may delete the Workspace.
pub(crate) fn erase(
    connection: &mut Connection,
    workspace_id: WorkspaceId,
    now_unix_ms: i64,
) -> Result<(), WorkspaceError> {
    let workspace = workspace_id.as_bytes().as_slice();
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(fault!(WorkspaceError::Storage(StorageError::Database)))?;
    let marked = transaction
        .execute(
            "INSERT INTO workspace_deletions (workspace_id, requested_at_unix_ms)
             SELECT workspace_id, ?2 FROM workspaces WHERE workspace_id = ?1",
            params![workspace, now_unix_ms],
        )
        .map_err(fault!(WorkspaceError::Storage(StorageError::Database)))?;
    if marked != 1 {
        return Err(WorkspaceError::UnknownWorkspace);
    }
    for statement in SCOPE_ROWS.iter().chain(WORKSPACE_ROWS) {
        transaction
            .execute(statement, [workspace])
            .map_err(fault!(WorkspaceError::Storage(StorageError::Database)))?;
    }
    verify_nothing_left(&transaction, workspace)?;
    transaction
        .execute(
            "DELETE FROM workspace_deletions WHERE workspace_id = ?1",
            [workspace],
        )
        .map_err(fault!(WorkspaceError::Storage(StorageError::Database)))?;
    transaction.commit().map_err(fault!(WorkspaceError::Storage(
        StorageError::UncertainOutcome
    )))
}

/// Refuses to commit if any table with a `workspace_id` column still holds a row of the
/// Workspace, so a table added later without an erase step fails loudly instead of leaving data.
fn verify_nothing_left(connection: &Connection, workspace: &[u8]) -> Result<(), WorkspaceError> {
    let mut tables = connection
        .prepare(
            "SELECT m.name FROM sqlite_master m
             WHERE m.type = 'table' AND m.name <> 'workspace_deletions'
               AND EXISTS (SELECT 1 FROM pragma_table_info(m.name) WHERE name = 'workspace_id')",
        )
        .map_err(fault!(WorkspaceError::Storage(StorageError::Database)))?;
    let names = tables
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(fault!(WorkspaceError::Storage(StorageError::Database)))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(fault!(WorkspaceError::Storage(StorageError::Database)))?;
    for name in names {
        let left: i64 = connection
            .query_row(
                &format!("SELECT COUNT(*) FROM \"{name}\" WHERE workspace_id = ?1"),
                [workspace],
                |row| row.get(0),
            )
            .map_err(fault!(WorkspaceError::Storage(StorageError::Database)))?;
        if left != 0 {
            return Err(WorkspaceError::Storage(StorageError::CorruptWorkspace));
        }
    }
    Ok(())
}
