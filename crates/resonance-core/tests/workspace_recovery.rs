use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use resonance_core::{CoreRuntime, CoreRuntimeConfig, MembershipRole, WorkspaceError};
use rusqlite::{Connection, OptionalExtension, params};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn private_data_directory(test_name: &str) -> PathBuf {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "resonance-core-{test_name}-{}-{sequence}",
        std::process::id()
    ))
}

fn database(directory: &Path) -> Connection {
    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    connection
}

#[tokio::test]
async fn create_workspace_persists_one_immutable_manager_genesis() {
    let directory = private_data_directory("workspace-create");
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open(config.clone()).await.unwrap();
    let creator = runtime.public_identity();

    let created = runtime.create_workspace("Field Notes").unwrap();
    assert_eq!(created.name(), "Field Notes");
    assert_eq!(created.created_by(), creator);
    assert!(created.created_at_unix_ms() > 0);
    assert_eq!(runtime.list_workspaces().unwrap(), vec![created.clone()]);
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let reopened = CoreRuntime::open(config).await.unwrap();
    assert_eq!(reopened.list_workspaces().unwrap(), vec![created.clone()]);

    let connection = database(&directory);
    let durable: (i64, i64, i64, i64, String) = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM workspaces),
                (SELECT COUNT(*) FROM membership_operations),
                (SELECT epoch FROM workspace_membership_state WHERE workspace_id = ?1),
                (SELECT COUNT(*) FROM membership_snapshot_members
                 WHERE workspace_id = ?1 AND member_identity = ?2),
                (SELECT status FROM workspace_creation_requests WHERE workspace_id = ?1)",
            params![created.id().to_string_bytes(), creator.to_string_bytes()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(durable, (1, 1, 0, 1, "Complete".to_owned()));
    let role: i64 = connection
        .query_row(
            "SELECT role FROM membership_snapshot_members WHERE workspace_id = ?1",
            [created.id().to_string_bytes()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(role, MembershipRole::Manager as i64);

    drop(connection);
    reopened.shutdown().await.unwrap();
    drop(reopened);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn pending_creation_rolls_forward_without_changing_genesis_bytes() {
    let directory = private_data_directory("workspace-pending-recovery");
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open(config.clone()).await.unwrap();
    let workspace = runtime.create_workspace("Recovered").unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let connection = database(&directory);
    let original: Vec<u8> = connection
        .query_row(
            "SELECT genesis_envelope FROM workspace_creation_requests WHERE workspace_id = ?1",
            [workspace.id().to_string_bytes()],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute_batch("DROP TRIGGER workspaces_no_delete;")
        .unwrap();
    connection
        .execute(
            "DELETE FROM membership_snapshot_members WHERE workspace_id = ?1",
            [workspace.id().to_string_bytes()],
        )
        .unwrap();
    connection
        .execute(
            "DELETE FROM workspace_membership_state WHERE workspace_id = ?1",
            [workspace.id().to_string_bytes()],
        )
        .unwrap();
    connection
        .execute_batch("DROP TRIGGER membership_operations_no_delete;")
        .unwrap();
    connection
        .execute(
            "DELETE FROM membership_operations WHERE workspace_id = ?1",
            [workspace.id().to_string_bytes()],
        )
        .unwrap();
    connection
        .execute(
            "DELETE FROM workspaces WHERE workspace_id = ?1",
            [workspace.id().to_string_bytes()],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE workspace_creation_requests SET status = 'Pending' WHERE workspace_id = ?1",
            [workspace.id().to_string_bytes()],
        )
        .unwrap();
    drop(connection);

    let recovered = CoreRuntime::open(config).await.unwrap();
    assert_eq!(
        recovered.list_workspaces().unwrap(),
        vec![workspace.clone()]
    );
    let connection = database(&directory);
    let recovered_bytes: Vec<u8> = connection
        .query_row(
            "SELECT canonical_envelope FROM membership_operations WHERE workspace_id = ?1",
            [workspace.id().to_string_bytes()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(recovered_bytes, original);
    let counts: (i64, i64, i64) = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM workspaces WHERE workspace_id = ?1),
                (SELECT COUNT(*) FROM membership_operations WHERE workspace_id = ?1),
                (SELECT COUNT(*) FROM workspace_creation_requests
                 WHERE workspace_id = ?1 AND status = 'Complete')",
            [workspace.id().to_string_bytes()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(counts, (1, 1, 1));

    drop(connection);
    recovered.shutdown().await.unwrap();
    drop(recovered);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn workspace_names_are_validated_before_any_request_is_written() {
    let directory = private_data_directory("workspace-name");
    let runtime = CoreRuntime::open(CoreRuntimeConfig::new(&directory))
        .await
        .unwrap();

    for invalid in [
        "",
        " leading",
        "trailing ",
        "line\nbreak",
        "name/with/slash",
        &"x".repeat(65),
    ] {
        assert_eq!(
            runtime.create_workspace(invalid).unwrap_err(),
            WorkspaceError::InvalidName
        );
    }
    assert!(runtime.list_workspaces().unwrap().is_empty());
    let connection = database(&directory);
    let pending: Option<i64> = connection
        .query_row("SELECT 1 FROM workspace_creation_requests", [], |row| {
            row.get(0)
        })
        .optional()
        .unwrap();
    assert_eq!(pending, None);

    drop(connection);
    runtime.shutdown().await.unwrap();
    drop(runtime);
    fs::remove_dir_all(directory).unwrap();
}

trait SqlBytes {
    fn to_string_bytes(self) -> Vec<u8>;
}

impl SqlBytes for resonance_core::WorkspaceId {
    fn to_string_bytes(self) -> Vec<u8> {
        hex_bytes(&self.to_string())
    }
}

impl SqlBytes for resonance_core::PublicIdentity {
    fn to_string_bytes(self) -> Vec<u8> {
        hex_bytes(&self.to_string())
    }
}

fn hex_bytes(value: &str) -> Vec<u8> {
    (0..value.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&value[offset..offset + 2], 16).unwrap())
        .collect()
}
