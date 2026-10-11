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
    let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
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
    let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
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
    let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
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
    let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
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
async fn migration_0006_adds_strict_immutable_journal_and_append_only_guards() {
    let directory = private_data_directory("migration-immutable-record-log");
    let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
        .await
        .unwrap();
    runtime.create_workspace("Journal").unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();
    for table in [
        "sync_scopes",
        "immutable_operations",
        "immutable_operation_dependencies",
        "immutable_author_heads",
        "immutable_records",
        "immutable_idempotency",
        "sealed_envelopes",
        "protection_state",
        "protection_pre_keys",
        "protection_grant_slots",
        "pending_records",
        "record_delivery",
        "durable_wakes",
    ] {
        let exists: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE type='table' AND name=?1",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "missing migration table {table}");
    }
    let trigger_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_schema WHERE type='trigger' AND name LIKE 'immutable_%_no_%'", [], |row| row.get(0)
    ).unwrap();
    assert!(trigger_count >= 6);
    let consumed_generation: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('durable_wakes')
             WHERE name = 'consumed_generation'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(consumed_generation, 1);
    drop(connection);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn migration_0007_adds_authenticated_p2panda_operation_projection() {
    let directory = private_data_directory("migration-p2panda-projection");
    let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
        .await
        .unwrap();
    let workspace = runtime.create_workspace("Projection").unwrap();
    runtime
        .append_record(workspace.id(), "teamarness.test", "message.post", b"sealed")
        .unwrap();
    runtime
        .append_record(
            workspace.id(),
            "teamarness.test",
            "message.post",
            b"sealed-2",
        )
        .unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 15);
    for column in [
        "scope_id",
        "log_id",
        "operation_id",
        "author_public_identity",
        "author_sequence",
        "backlink",
        "payload_hash",
        "payload",
        "header",
        "true_height",
    ] {
        let exists: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('p2panda_operations') WHERE name = ?1",
                [column],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "missing p2panda projection column {column}");
    }
    let projection: (i64, i64, i64) = connection
        .query_row(
            "SELECT length(header), length(payload), true_height FROM p2panda_operations
             WHERE author_sequence = 0",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert!(projection.0 > 0);
    assert!(projection.1 > 0);
    assert_eq!(projection.2, 0);
    let first_operation: Vec<u8> = connection
        .query_row(
            "SELECT operation_id FROM p2panda_operations WHERE author_sequence = 0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let second_backlink: Vec<u8> = connection
        .query_row(
            "SELECT backlink FROM p2panda_operations WHERE author_sequence = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(second_backlink, first_operation);
    drop(connection);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn ordered_migration_is_idempotent_on_reopen() {
    let directory = private_data_directory("migration-reopen");
    let config = CoreRuntimeConfig::new(&directory);

    let first = CoreRuntime::open_loopback_for_tests(config.clone())
        .await
        .unwrap();
    first.shutdown().await.unwrap();
    drop(first);
    let second = CoreRuntime::open_loopback_for_tests(config).await.unwrap();
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
    assert_eq!(version, 15);
    assert_eq!(installation_count, 1);

    drop(connection);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn migration_0010_adds_membership_wakes_and_parking_and_drops_pending_records() {
    let directory = private_data_directory("migration-membership-replication");
    let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
        .await
        .unwrap();
    let workspace = runtime.create_workspace("Replication").unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();

    let wake: (Vec<u8>, i64, i64) = connection
        .query_row(
            "SELECT workspace_id, wake_generation, consumed_generation FROM membership_sync_wakes",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(wake.0, hex_bytes(&workspace.id().to_string()));
    assert_eq!((wake.1, wake.2), (0, 0));

    for column in [
        "operation_hash",
        "scope_id",
        "workspace_id",
        "package_identity",
        "author_public_identity",
        "author_sequence",
        "reason",
        "received_from",
        "operation_bytes",
        "parked_at_unix_ms",
    ] {
        let exists: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('parked_operations') WHERE name = ?1",
                [column],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "missing parked_operations column {column}");
    }
    let retired: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'table' AND name = 'pending_protection_records'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retired, 0, "the pending record table has no writer left");

    drop(connection);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn accepted_membership_operations_bump_the_membership_wake() {
    let directory = private_data_directory("migration-membership-wake-bump");
    let joiner_directory = private_data_directory("migration-membership-wake-joiner");
    // Real Installations over loopback addressing: the admission is the real
    // one, and nothing off this machine is contacted (SF-002).
    let manager = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
        .await
        .unwrap();
    let workspace = manager.create_workspace("Wakes").unwrap();
    let invitation = manager
        .create_workspace_invitation(workspace.id())
        .await
        .unwrap();
    let joiner = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&joiner_directory))
        .await
        .unwrap();
    manager.remember_loopback_peer_for_tests(&joiner);
    joiner.remember_loopback_peer_for_tests(&manager);
    joiner.join_workspace(&invitation).await.unwrap();
    manager
        .revoke_member(workspace.id(), joiner.public_identity())
        .unwrap();
    manager.shutdown().await.unwrap();
    joiner.shutdown().await.unwrap();
    drop(manager);
    drop(joiner);

    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();
    let generation: i64 = connection
        .query_row(
            "SELECT wake_generation FROM membership_sync_wakes",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(generation, 2, "one admission and one revocation");
    drop(connection);

    let joiner_connection = Connection::open(joiner_directory.join("core.sqlite3")).unwrap();
    let imported: i64 = joiner_connection
        .query_row(
            "SELECT wake_generation FROM membership_sync_wakes",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        imported >= 1,
        "a bootstrap import leaves the joiner due to advertise itself"
    );
    drop(joiner_connection);

    fs::remove_dir_all(directory).unwrap();
    fs::remove_dir_all(joiner_directory).unwrap();
}

#[tokio::test]
async fn membership_wake_row_is_seeded_for_existing_workspaces() {
    let directory = private_data_directory("migration-membership-wake-seed");
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open_loopback_for_tests(config.clone())
        .await
        .unwrap();
    runtime.create_workspace("Seeded").unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);

    // Rewind to the schema before this migration, keeping the workspace row.
    // `pending_protection_records` and `protection_state.state_bytes` are
    // restored only so the forward migrations have the table and the column
    // they drop; neither shape is the subject of this test.
    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();
    connection
        .execute_batch(
            "DROP TABLE workspace_person_name_consumption;
             DROP TABLE workspace_person_name_generations; DROP TABLE workspace_person_name_winners;
             DROP TABLE workspace_person_name_intent_revisions;
             DROP TABLE pending_person_join_names; DROP TABLE pending_person_name_edits;
             DROP TABLE workspace_person_names;
             ALTER TABLE workspace_creation_requests DROP COLUMN initial_person_name;
             DROP TABLE sync_scope_key_demand_peer_progress;
             DROP TABLE sync_scope_key_demands;
             DROP TABLE sync_peer_progress;
             DROP TABLE membership_peer_progress;
             DROP TABLE scope_validation;
             DROP TABLE quarantined_scopes;
             DROP TABLE quarantined_records;
             DROP TABLE membership_sync_wakes;
             DROP TABLE parked_operations;
             CREATE TABLE pending_protection_records (pending_id INTEGER PRIMARY KEY);
             ALTER TABLE protection_state ADD COLUMN state_bytes BLOB;
             PRAGMA user_version = 9;",
        )
        .unwrap();
    drop(connection);

    let runtime = CoreRuntime::open_loopback_for_tests(config).await.unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();
    let seeded: (i64, i64) = connection
        .query_row(
            "SELECT COUNT(*), COALESCE(MAX(wake_generation), -1) FROM membership_sync_wakes",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(seeded, (1, 1), "an existing workspace is seeded as due");
    drop(connection);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn migration_0011_adds_progress_and_quarantine_tables() {
    let directory = private_data_directory("migration-sync-bounds");
    let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
        .await
        .unwrap();
    let workspace = runtime.create_workspace("Bounds").unwrap();
    runtime
        .append_record(workspace.id(), "teamarness.test", "message.post", b"sealed")
        .unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();

    for table in [
        "sync_peer_progress",
        "membership_peer_progress",
        "scope_validation",
        "quarantined_scopes",
        "quarantined_records",
    ] {
        let rows: i64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap_or_else(|error| panic!("missing migration table {table}: {error}"));
        assert_eq!(rows, 0, "{table} starts empty");
    }

    let scope: Vec<u8> = connection
        .query_row("SELECT scope_id FROM sync_scopes", [], |row| row.get(0))
        .unwrap();
    let rejected = connection.execute(
        "INSERT INTO quarantined_records(record_id, scope_id, reason, detected_at_unix_ms)
         VALUES (?1, ?2, 'not_a_reason', 1)",
        params![[9_u8; 32].as_slice(), scope.as_slice()],
    );
    assert!(rejected.is_err(), "an unknown quarantine reason is refused");

    drop(connection);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn migration_0013_adds_bounded_key_demand_and_upgrades_v12() {
    let directory = private_data_directory("migration-key-demand");
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open_loopback_for_tests(config.clone())
        .await
        .unwrap();
    let workspace = runtime.create_workspace("Demand").unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    let workspace_bytes = hex_bytes(&workspace.id().to_string());
    let insert = "INSERT INTO sync_scope_key_demands(scope_id, package_identity, workspace_id, demand_generation) VALUES (?1, ?2, ?3, ?4)";
    let scope = [3_u8; 32];
    connection
        .execute(
            insert,
            params![scope.as_slice(), "teamarness.test", workspace_bytes, 1],
        )
        .unwrap();
    assert!(
        connection
            .execute(
                insert,
                params![[4_u8; 32].as_slice(), "teamarness.test", workspace_bytes, 1]
            )
            .is_err(),
        "Package and Workspace uniquely determine demand"
    );
    assert!(
        connection
            .execute(
                insert,
                params![scope.as_slice(), "other", workspace_bytes, 1]
            )
            .is_err(),
        "scope digest is unique"
    );
    assert!(
        connection
            .execute(
                insert,
                params![[5_u8; 31].as_slice(), "other", workspace_bytes, 1]
            )
            .is_err(),
        "scope digest must be 32 bytes"
    );
    assert!(
        connection
            .execute(
                insert,
                params![[5_u8; 32].as_slice(), "other", workspace_bytes, -1]
            )
            .is_err(),
        "generation cannot be negative"
    );
    assert!(
        connection
            .execute(
                insert,
                params![[5_u8; 32].as_slice(), "other", [8_u8; 16].as_slice(), 1]
            )
            .is_err(),
        "Workspace must exist"
    );
    let progress = "INSERT INTO sync_scope_key_demand_peer_progress(scope_id, peer_public_identity, consumed_generation) VALUES (?1, ?2, ?3)";
    connection
        .execute(
            progress,
            params![scope.as_slice(), [7_u8; 32].as_slice(), 0],
        )
        .unwrap();
    assert!(
        connection
            .execute(
                progress,
                params![scope.as_slice(), [8_u8; 31].as_slice(), 0]
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                progress,
                params![scope.as_slice(), [8_u8; 32].as_slice(), -1]
            )
            .is_err()
    );
    connection
        .execute("DELETE FROM sync_scope_key_demands", [])
        .unwrap();
    let left: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sync_scope_key_demand_peer_progress",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(left, 0, "peer progress cascades when demand is cleared");
    connection.execute_batch("DROP TABLE workspace_person_name_consumption; DROP TABLE workspace_person_name_generations; DROP TABLE workspace_person_name_winners; DROP TABLE workspace_person_name_intent_revisions; DROP TABLE pending_person_join_names; DROP TABLE pending_person_name_edits; DROP TABLE workspace_person_names; ALTER TABLE workspace_creation_requests DROP COLUMN initial_person_name; DROP TABLE sync_scope_key_demand_peer_progress; DROP TABLE sync_scope_key_demands; PRAGMA user_version = 12;").unwrap();
    drop(connection);

    let reopened = CoreRuntime::open_loopback_for_tests(config).await.unwrap();
    reopened.shutdown().await.unwrap();
    drop(reopened);
    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 15);
    connection
        .execute(
            insert,
            params![scope.as_slice(), "teamarness.test", workspace_bytes, 1],
        )
        .unwrap();
    drop(connection);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn migration_0015_preserves_v14_pending_name_and_assigns_intent_revision() {
    let directory = private_data_directory("migration-v14-pending-intent");
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open_loopback_for_tests(config.clone())
        .await
        .unwrap();
    let workspace = runtime
        .create_workspace_with_person_name("Existing", "Chosen")
        .unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();
    connection
        .execute_batch(
            "DROP TRIGGER person_name_intent_insert;
         DROP TRIGGER person_name_intent_name_update;
         DROP TABLE workspace_person_name_winners;
         DROP TABLE workspace_person_name_generations;
         DROP TABLE workspace_person_name_consumption;
         DROP TABLE workspace_person_name_intent_revisions;
         ALTER TABLE pending_person_name_edits DROP COLUMN intent_revision;
         INSERT INTO pending_person_name_edits (workspace_id, person_id, name)
         SELECT workspace_id, person_id, name FROM workspace_person_names;
         PRAGMA user_version = 14;",
        )
        .unwrap();
    let name: String = connection
        .query_row("SELECT name FROM pending_person_name_edits", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(name, "Chosen");
    drop(connection);
    let reopened = CoreRuntime::open_loopback_for_tests(config).await.unwrap();
    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();
    let (pending, revision): (i64, i64) = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM pending_person_name_edits), last_revision FROM workspace_person_name_intent_revisions",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        (pending, revision),
        (0, 1),
        "reopen publishes the migrated pending name"
    );
    drop(connection);
    reopened.set_own_person_name(workspace.id(), "New").unwrap();
    reopened.shutdown().await.unwrap();
    drop(reopened);
    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();
    let (name, next): (String, i64) = connection
        .query_row(
            "SELECT n.name, r.last_revision FROM workspace_person_names n JOIN workspace_person_name_intent_revisions r USING (workspace_id, person_id)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((name.as_str(), next), ("New", 2));
    drop(connection);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn migration_0015_rejects_future_schema_versions() {
    let directory = private_data_directory("migration-future-schema");
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open_loopback_for_tests(config.clone())
        .await
        .unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();
    connection.pragma_update(None, "user_version", 16).unwrap();
    drop(connection);
    assert!(matches!(
        CoreRuntime::open_loopback_for_tests(config).await,
        Err(teamarness_core::CoreError::Storage(
            teamarness_core::StorageError::UnsupportedSchema
        ))
    ));
    fs::remove_dir_all(directory).unwrap();
}

fn hex_bytes(value: &str) -> Vec<u8> {
    (0..value.len() / 2)
        .map(|index| u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).unwrap())
        .collect()
}

#[tokio::test]
// Verifies: TEAM.SYS.ID-R07
async fn migration_0012_removes_duplicate_content_key_copy() {
    let directory = private_data_directory("migration-single-content-key");
    let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
        .await
        .unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();
    let columns: Vec<String> = {
        let mut statement = connection
            .prepare("SELECT name FROM pragma_table_info('protection_state')")
            .unwrap();
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap();
        rows.map(Result::unwrap).collect()
    };
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();

    assert!(
        !columns.iter().any(|column| column == "state_bytes"),
        "the second content-key copy is gone: {columns:?}"
    );
    assert!(columns.iter().any(|column| column == "content_key_id"));
    assert!(
        columns
            .iter()
            .any(|column| column == "creator_public_identity")
    );
    assert_eq!(version, 15);

    drop(connection);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn migration_0014_upgrades_existing_unnamed_workspace_without_inventing_a_name() {
    let directory = private_data_directory("migration-person-names");
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open_loopback_for_tests(config.clone())
        .await
        .unwrap();
    let workspace = runtime.create_workspace("Old workspace").unwrap();
    let identity = runtime.public_identity();
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();
    connection.execute_batch("DROP TABLE workspace_person_name_consumption; DROP TABLE workspace_person_name_generations; DROP TABLE workspace_person_name_winners; DROP TABLE workspace_person_name_intent_revisions; DROP TABLE pending_person_join_names; DROP TABLE pending_person_name_edits; DROP TABLE workspace_person_names; ALTER TABLE workspace_creation_requests DROP COLUMN initial_person_name; PRAGMA user_version = 13;").unwrap();
    drop(connection);
    let reopened = CoreRuntime::open_loopback_for_tests(config).await.unwrap();
    let old = reopened
        .workspace_person(workspace.id(), identity)
        .unwrap()
        .unwrap();
    assert_eq!(old.name(), None);
    reopened
        .set_own_person_name(workspace.id(), "Chosen")
        .unwrap();
    assert_eq!(
        reopened
            .workspace_person(workspace.id(), identity)
            .unwrap()
            .unwrap()
            .id(),
        old.id()
    );
    reopened.shutdown().await.unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}
