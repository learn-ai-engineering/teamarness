use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use resonance_core::{CoreError, CoreRuntime, CoreRuntimeConfig, StorageError};
use rusqlite::{Connection, params};

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
async fn append_only_guards_reject_canonical_and_trust_anchor_changes() {
    let directory = private_data_directory("append-only");
    let runtime = CoreRuntime::open(CoreRuntimeConfig::new(&directory))
        .await
        .unwrap();
    runtime.create_workspace("Guarded").unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let connection = database(&directory);

    assert!(
        connection
            .execute(
                "UPDATE membership_operations SET authored_at_unix_ms = 0",
                []
            )
            .is_err()
    );
    assert!(
        connection
            .execute("DELETE FROM membership_operations", [])
            .is_err()
    );
    assert!(
        connection
            .execute("UPDATE workspaces SET name = 'Changed'", [])
            .is_err()
    );
    assert!(connection.execute("DELETE FROM workspaces", []).is_err());

    drop(connection);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn trust_anchor_mismatch_refuses_reopen() {
    let directory = private_data_directory("trust-anchor-corrupt");
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open(config.clone()).await.unwrap();
    runtime.create_workspace("Authentic").unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let connection = database(&directory);
    connection
        .execute_batch("DROP TRIGGER workspaces_no_update;")
        .unwrap();
    connection
        .execute("UPDATE workspaces SET name = 'Substituted'", [])
        .unwrap();
    drop(connection);

    assert!(matches!(
        CoreRuntime::open(config).await,
        Err(CoreError::Storage(StorageError::CorruptWorkspace))
    ));

    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn forged_or_wrong_workspace_genesis_refuses_reopen() {
    for (suffix, corruption) in ["forged-bytes", "wrong-workspace"].into_iter().enumerate() {
        let directory = private_data_directory(&format!("genesis-{suffix}"));
        let config = CoreRuntimeConfig::new(&directory);
        let runtime = CoreRuntime::open(config.clone()).await.unwrap();
        let first = runtime.create_workspace("First").unwrap();
        let second = runtime.create_workspace("Second").unwrap();
        runtime.shutdown().await.unwrap();
        drop(runtime);
        let connection = database(&directory);
        connection
            .execute_batch("DROP TRIGGER membership_operations_no_update;")
            .unwrap();
        let first_id = hex_bytes(&first.id().to_string());
        match corruption {
            "forged-bytes" => {
                connection
                    .execute(
                        "UPDATE membership_operations
                         SET canonical_envelope = X'00'
                         WHERE workspace_id = ?1",
                        [first_id],
                    )
                    .unwrap();
            }
            "wrong-workspace" => {
                let second_id = hex_bytes(&second.id().to_string());
                connection
                    .execute(
                        "UPDATE membership_operations SET workspace_id = ?1
                         WHERE workspace_id = ?2",
                        params![second_id, first_id],
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }
        drop(connection);

        assert!(matches!(
            CoreRuntime::open(config).await,
            Err(CoreError::Storage(StorageError::CorruptWorkspace))
        ));
        fs::remove_dir_all(directory).unwrap();
    }
}

#[tokio::test]
async fn duplicate_or_conflicting_workspace_and_genesis_ids_are_rejected() {
    let directory = private_data_directory("duplicate-ids");
    let runtime = CoreRuntime::open(CoreRuntimeConfig::new(&directory))
        .await
        .unwrap();
    let workspace = runtime.create_workspace("Unique").unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let connection = database(&directory);
    let workspace_id = hex_bytes(&workspace.id().to_string());
    let request: (Vec<u8>, String, i64, Vec<u8>, Vec<u8>) = connection
        .query_row(
            "SELECT creator_public_identity, name, authored_at_unix_ms,
                    genesis_operation_id, genesis_envelope
             FROM workspace_creation_requests WHERE workspace_id = ?1",
            [workspace_id.as_slice()],
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

    assert!(
        connection
            .execute(
                "INSERT INTO workspace_creation_requests (
                    workspace_id, creator_public_identity, name, authored_at_unix_ms,
                    genesis_operation_id, genesis_envelope, status
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'Pending')",
                params![
                    workspace_id,
                    request.0,
                    request.1,
                    request.2,
                    request.3,
                    request.4
                ],
            )
            .is_err()
    );

    drop(connection);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn durable_future_quarantine_is_inert_across_reopen() {
    let directory = private_data_directory("future-inert");
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open(config.clone()).await.unwrap();
    let workspace = runtime.create_workspace("Future").unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let connection = database(&directory);
    let workspace_id = hex_bytes(&workspace.id().to_string());
    let creator: Vec<u8> = connection
        .query_row(
            "SELECT creator_public_identity FROM workspaces WHERE workspace_id = ?1",
            [workspace_id.as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO future_membership_operations (
                operation_id, workspace_id, author_public_identity,
                authored_at_unix_ms, canonical_envelope, rejection_reason
             ) VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
            params![
                [8_u8; 32].as_slice(),
                workspace_id,
                creator,
                i64::MAX,
                b"authenticated fixture"
            ],
        )
        .unwrap();
    let before: (i64, i64, i64) = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM membership_operations),
                (SELECT epoch FROM workspace_membership_state),
                (SELECT COUNT(*) FROM membership_snapshot_members)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    drop(connection);

    let reopened = CoreRuntime::open(config).await.unwrap();
    assert_eq!(reopened.list_workspaces().unwrap(), vec![workspace]);
    let connection = database(&directory);
    let after: (i64, i64, i64, i64) = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM membership_operations),
                (SELECT epoch FROM workspace_membership_state),
                (SELECT COUNT(*) FROM membership_snapshot_members),
                (SELECT COUNT(*) FROM future_membership_operations)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!((after.0, after.1, after.2), before);
    assert_eq!(after.3, 1);

    drop(connection);
    reopened.shutdown().await.unwrap();
    drop(reopened);
    fs::remove_dir_all(directory).unwrap();
}

fn hex_bytes(value: &str) -> Vec<u8> {
    (0..value.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&value[offset..offset + 2], 16).unwrap())
        .collect()
}
