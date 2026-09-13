use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{Connection, OptionalExtension, params};
use teamarness_core::{CoreRuntime, CoreRuntimeConfig};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn private_data_directory(test_name: &str) -> PathBuf {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "teamarness-core-{test_name}-{}-{sequence}",
        std::process::id()
    ))
}

#[tokio::test]
async fn migration_0001_reserves_nullable_human_identity_subjects() {
    let directory = private_data_directory("migration-human-identity");
    let runtime = CoreRuntime::open(CoreRuntimeConfig::new(&directory))
        .await
        .unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();

    let human_column: Option<(i64, String)> = connection
        .query_row(
            "SELECT [notnull], COALESCE(dflt_value, 'NULL')
             FROM pragma_table_info('identity_subjects')
             WHERE name = 'human_identity_id'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .unwrap();
    assert_eq!(human_column, Some((0, "NULL".to_owned())));

    let local_subject: (i64, Option<Vec<u8>>) = connection
        .query_row(
            "SELECT COUNT(*), human_identity_id FROM identity_subjects",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(local_subject, (1, None));

    connection
        .execute(
            "INSERT INTO identity_subjects (
                installation_public_identity,
                human_identity_id
             ) VALUES (?1, NULL)",
            params![[7_u8; 32].as_slice()],
        )
        .unwrap();
    let ungrouped_subjects: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM identity_subjects WHERE human_identity_id IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(ungrouped_subjects, 2);

    drop(connection);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn migration_0002_adds_workspace_membership_state_and_append_only_guards() {
    let directory = private_data_directory("migration-workspaces");
    let runtime = CoreRuntime::open(CoreRuntimeConfig::new(&directory))
        .await
        .unwrap();
    runtime.create_workspace("Migration").unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();

    for table in [
        "workspace_creation_requests",
        "workspaces",
        "membership_operations",
        "membership_dependencies",
        "future_membership_operations",
        "future_membership_dependencies",
        "workspace_membership_state",
        "membership_snapshot_members",
    ] {
        let exists: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "missing migration table {table}");
    }
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

    drop(connection);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn migration_0003_adds_the_unique_invitation_nonce_ledger() {
    let directory = private_data_directory("migration-invitations");
    let runtime = CoreRuntime::open(CoreRuntimeConfig::new(&directory))
        .await
        .unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();

    let table_exists: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'table' AND name = 'used_invitation_nonces'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let nonce_primary_key: i64 = connection
        .query_row(
            "SELECT pk FROM pragma_table_info('used_invitation_nonces') WHERE name = 'nonce'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(table_exists, 1);
    assert_eq!(nonce_primary_key, 1);

    drop(connection);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn migration_0004_adds_the_append_only_transition_ledger() {
    let directory = private_data_directory("migration-transitions");
    let runtime = CoreRuntime::open(CoreRuntimeConfig::new(&directory))
        .await
        .unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();

    let table_exists: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'table' AND name = 'membership_transitions'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let composite_primary_key: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('membership_transitions') WHERE pk > 0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(table_exists, 1);
    assert_eq!(composite_primary_key, 2);

    drop(connection);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn ordered_migration_is_idempotent_on_reopen() {
    let directory = private_data_directory("migration-reopen");
    let config = CoreRuntimeConfig::new(&directory);

    let first = CoreRuntime::open(config.clone()).await.unwrap();
    first.shutdown().await.unwrap();
    drop(first);
    let second = CoreRuntime::open(config).await.unwrap();
    second.shutdown().await.unwrap();
    drop(second);

    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    let installation_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM installation_identity", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(version, 5);
    assert_eq!(installation_count, 1);

    drop(connection);
    fs::remove_dir_all(directory).unwrap();
}
