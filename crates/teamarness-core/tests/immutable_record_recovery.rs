use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::Connection;
use teamarness_core::{CoreRuntime, CoreRuntimeConfig, StorageError};

/// Rewrites every stored protection control through a byte-level edit, because
/// SQLite string concatenation would turn the STRICT blob column into text.
fn rewrite_control_bytes(connection: &Connection, edit: impl Fn(&mut Vec<u8>)) {
    let rows: Vec<(Vec<u8>, Vec<u8>)> = {
        let mut statement = connection
            .prepare("SELECT control_id, control_bytes FROM protection_control_operations")
            .unwrap();
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap();
        rows.collect::<Result<_, _>>().unwrap()
    };
    assert!(!rows.is_empty());
    for (id, mut bytes) in rows {
        edit(&mut bytes);
        connection
            .execute(
                "UPDATE protection_control_operations SET control_bytes = ?2 WHERE control_id = ?1",
                rusqlite::params![id, bytes],
            )
            .unwrap();
    }
}

static NEXT: AtomicU64 = AtomicU64::new(0);
fn directory() -> PathBuf {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "teamarness-immutable-recovery-{}-{n}",
        std::process::id()
    ))
}

// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test]
async fn restart_quarantines_a_scope_with_tampered_key_state() {
    let path = directory();
    let config = CoreRuntimeConfig::new(&path);
    let runtime = CoreRuntime::open(config.clone()).await.unwrap();
    let workspace = runtime.create_workspace("Recovery").unwrap();
    runtime
        .append_record(workspace.id(), "teamarness.test", "message.post", b"secret")
        .unwrap();
    runtime
        .append_record(workspace.id(), "teamarness.other", "message.post", b"other")
        .unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let connection = Connection::open(path.join("core.sqlite3")).unwrap();
    let tampered = scope_of(&connection, "teamarness.test");
    connection
        .execute(
            "UPDATE sync_scopes SET content_key_id = zeroblob(32) WHERE scope_id = ?1",
            [tampered.as_slice()],
        )
        .unwrap();
    drop(connection);

    let reopened = CoreRuntime::open(config).await.unwrap();
    assert!(matches!(
        reopened.append_record(workspace.id(), "teamarness.test", "message.post", b"again"),
        Err(teamarness_core::SynchronizationError::ScopeQuarantined)
    ));
    reopened
        .append_record(workspace.id(), "teamarness.other", "message.post", b"again")
        .expect("an unrelated scope keeps working");
    assert_eq!(reopened.list_workspaces().unwrap().len(), 1);
    reopened.shutdown().await.unwrap();
    drop(reopened);

    let connection = Connection::open(path.join("core.sqlite3")).unwrap();
    let reason: String = connection
        .query_row(
            "SELECT reason FROM quarantined_scopes WHERE scope_id = ?1",
            [tampered.as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(reason, "content_key_id");
    drop(connection);
    fs::remove_dir_all(path).unwrap();
}

// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test]
async fn restart_quarantines_a_scope_with_a_tampered_projection() {
    let path = directory();
    let config = CoreRuntimeConfig::new(&path);
    let runtime = CoreRuntime::open(config.clone()).await.unwrap();
    let workspace = runtime.create_workspace("Projection tamper").unwrap();
    runtime
        .append_record(workspace.id(), "teamarness.test", "message.post", b"sealed")
        .unwrap();
    runtime
        .append_record(workspace.id(), "teamarness.other", "message.post", b"other")
        .unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let connection = Connection::open(path.join("core.sqlite3")).unwrap();
    let tampered = scope_of(&connection, "teamarness.test");
    connection
        .execute("DROP TRIGGER p2panda_operations_no_update", [])
        .unwrap();
    connection
        .execute(
            "UPDATE p2panda_operations SET payload_hash = zeroblob(32) WHERE scope_id = ?1",
            [tampered.as_slice()],
        )
        .unwrap();
    drop(connection);

    let reopened = CoreRuntime::open(config).await.unwrap();
    assert!(matches!(
        reopened.append_record(workspace.id(), "teamarness.test", "message.post", b"again"),
        Err(teamarness_core::SynchronizationError::ScopeQuarantined)
    ));
    reopened
        .append_record(workspace.id(), "teamarness.other", "message.post", b"again")
        .expect("an unrelated scope keeps working");
    reopened.shutdown().await.unwrap();
    drop(reopened);

    let connection = Connection::open(path.join("core.sqlite3")).unwrap();
    let reason: String = connection
        .query_row(
            "SELECT reason FROM quarantined_scopes WHERE scope_id = ?1",
            [tampered.as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(reason, "projection");
    drop(connection);
    fs::remove_dir_all(path).unwrap();
}

/// A tampered record below the validation mark is never re-read at restart, so
/// it is caught the next time a subscriber reads it, and skipped from then on.
// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test]
async fn tampered_record_below_the_validation_mark_is_quarantined_on_read() {
    let path = directory();
    let config = CoreRuntimeConfig::new(&path);
    let runtime = CoreRuntime::open(config.clone()).await.unwrap();
    let workspace = runtime.create_workspace("Late tamper").unwrap();
    let mut appended = Vec::new();
    for index in 0..3_u8 {
        appended.push(
            runtime
                .append_record(
                    workspace.id(),
                    "teamarness.test",
                    "message.post",
                    &[index; 8],
                )
                .unwrap(),
        );
    }
    runtime.shutdown().await.unwrap();
    drop(runtime);

    // The first restart marks all three as validated.
    let runtime = CoreRuntime::open(config.clone()).await.unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let second = appended[1].record_id().to_string();
    let connection = Connection::open(path.join("core.sqlite3")).unwrap();
    connection
        .execute("DROP TRIGGER immutable_records_no_update", [])
        .unwrap();
    let mut sealed: Vec<u8> = connection
        .query_row(
            "SELECT sealed_record FROM immutable_records WHERE record_id = ?1",
            [hex_bytes(&second)],
            |row| row.get(0),
        )
        .unwrap();
    let last = sealed.len() - 1;
    sealed[last] ^= 1;
    connection
        .execute(
            "UPDATE immutable_records SET sealed_record = ?2 WHERE record_id = ?1",
            rusqlite::params![hex_bytes(&second), sealed],
        )
        .unwrap();
    drop(connection);

    let reopened = CoreRuntime::open(config).await.unwrap();
    let mut subscription = reopened
        .subscribe_records(workspace.id(), "teamarness.test", None)
        .unwrap();
    let mut delivered = Vec::new();
    while let Some(event) = subscription
        .next_event_timeout(std::time::Duration::from_millis(50))
        .unwrap()
    {
        delivered.push(event.record().record_id().to_string());
    }
    assert_eq!(
        delivered,
        vec![
            appended[0].record_id().to_string(),
            appended[2].record_id().to_string()
        ],
        "the tampered record is skipped, the others are delivered"
    );
    drop(subscription);
    reopened.shutdown().await.unwrap();
    drop(reopened);

    let connection = Connection::open(path.join("core.sqlite3")).unwrap();
    let quarantined: (Vec<u8>, String) = connection
        .query_row(
            "SELECT record_id, reason FROM quarantined_records",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(quarantined.0, hex_bytes(&second));
    assert_eq!(quarantined.1, "open");
    drop(connection);
    fs::remove_dir_all(path).unwrap();
}

fn scope_of(connection: &Connection, package: &str) -> Vec<u8> {
    connection
        .query_row(
            "SELECT scope_id FROM sync_scopes WHERE package_identity = ?1",
            [package],
            |row| row.get(0),
        )
        .unwrap()
}

fn hex_bytes(value: &str) -> Vec<u8> {
    (0..value.len() / 2)
        .map(|index| u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).unwrap())
        .collect()
}

#[tokio::test]
async fn creator_bootstrap_is_recorded_as_authenticated_control() {
    let path = directory();
    let runtime = CoreRuntime::open(CoreRuntimeConfig::new(&path))
        .await
        .unwrap();
    let workspace = runtime.create_workspace("Protection bootstrap").unwrap();
    runtime
        .append_record(
            workspace.id(),
            "teamarness.test",
            "message.post",
            b"bootstrap",
        )
        .unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let connection = Connection::open(path.join("core.sqlite3")).unwrap();
    let (kind, length): (String, i64) = connection
        .query_row(
            "SELECT operation_kind, length(control_bytes) FROM protection_control_operations LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(kind, "create");
    assert_eq!(length, 354);
    fs::remove_dir_all(path).unwrap();
}

// Verifies: TEAM.SYS.ID-R08
#[tokio::test]
async fn restart_refuses_legacy_version_1_protection_controls() {
    let path = directory();
    let config = CoreRuntimeConfig::new(&path);
    let runtime = CoreRuntime::open(config.clone()).await.unwrap();
    let workspace = runtime.create_workspace("Legacy controls").unwrap();
    runtime
        .append_record(workspace.id(), "teamarness.test", "message.post", b"legacy")
        .unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let connection = Connection::open(path.join("core.sqlite3")).unwrap();
    connection
        .execute("DROP TRIGGER protection_control_operations_no_update", [])
        .unwrap();
    rewrite_control_bytes(&connection, |bytes| {
        bytes[..32].copy_from_slice(b"teamarness.protection-control/v1");
    });
    drop(connection);

    assert!(matches!(
        CoreRuntime::open(config).await,
        Err(teamarness_core::CoreError::Storage(
            StorageError::LegacyProtectionState
        ))
    ));
    assert!(path.join("core.sqlite3").exists());
    fs::remove_dir_all(path).unwrap();
}

// Verifies: TEAM.SYS.ID-R08
#[tokio::test]
async fn restart_rejects_tampered_protection_control() {
    let path = directory();
    let config = CoreRuntimeConfig::new(&path);
    let runtime = CoreRuntime::open(config.clone()).await.unwrap();
    let workspace = runtime.create_workspace("Tampered control").unwrap();
    runtime
        .append_record(workspace.id(), "teamarness.test", "message.post", b"tamper")
        .unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let connection = Connection::open(path.join("core.sqlite3")).unwrap();
    connection
        .execute("DROP TRIGGER protection_control_operations_no_update", [])
        .unwrap();
    // Offset 242 is the first ciphertext byte, inside the signed prefix.
    rewrite_control_bytes(&connection, |bytes| bytes[242] ^= 1);
    drop(connection);

    assert!(matches!(
        CoreRuntime::open(config).await,
        Err(teamarness_core::CoreError::Storage(
            StorageError::CorruptWorkspace
        ))
    ));
    fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn core_database_contains_sealed_payload_not_canonical_record_bytes() {
    let path = directory();
    let runtime = CoreRuntime::open(CoreRuntimeConfig::new(&path))
        .await
        .unwrap();
    let workspace = runtime.create_workspace("At Rest").unwrap();
    runtime
        .append_record(
            workspace.id(),
            "teamarness.test",
            "message.post",
            b"plaintext-sentinel",
        )
        .unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let connection = Connection::open(path.join("core.sqlite3")).unwrap();
    let stored: Vec<u8> = connection
        .query_row("SELECT sealed_record FROM immutable_records", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(
        !stored
            .windows(b"plaintext-sentinel".len())
            .any(|window| window == b"plaintext-sentinel")
    );
    let canonical_digest: Vec<u8> = connection
        .query_row(
            "SELECT canonical_digest FROM immutable_records",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(canonical_digest.len(), 32);
    let projected_payload: Vec<u8> = connection
        .query_row("SELECT payload FROM p2panda_operations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(
        !projected_payload
            .windows(b"plaintext-sentinel".len())
            .any(|window| window == b"plaintext-sentinel")
    );
    fs::remove_dir_all(path).unwrap();
}

/// Counts how many BLOB values anywhere in the database contain `needle`.
fn blob_occurrences(connection: &Connection, needle: &[u8]) -> usize {
    let tables: Vec<String> = {
        let mut statement = connection
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
            .unwrap();
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap();
        rows.map(Result::unwrap).collect()
    };
    let mut found = 0;
    for table in tables {
        let columns: Vec<String> = {
            let mut statement = connection
                .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
                .unwrap();
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap();
            rows.map(Result::unwrap).collect()
        };
        for column in columns {
            let mut statement = connection
                .prepare(&format!(
                    "SELECT \"{column}\" FROM \"{table}\" WHERE typeof(\"{column}\") = 'blob'"
                ))
                .unwrap();
            let rows = statement
                .query_map([], |row| row.get::<_, Vec<u8>>(0))
                .unwrap();
            for value in rows.map(Result::unwrap) {
                found += value
                    .windows(needle.len())
                    .filter(|window| *window == needle)
                    .count();
            }
        }
    }
    found
}

// Verifies: TEAM.SYS.ID-R07
#[tokio::test]
async fn content_key_is_stored_exactly_once() {
    let path = directory();
    let runtime = CoreRuntime::open(CoreRuntimeConfig::new(&path))
        .await
        .unwrap();
    let workspace = runtime.create_workspace("Custody").unwrap();
    runtime
        .append_record(
            workspace.id(),
            "teamarness.test",
            "message.post",
            b"one key",
        )
        .unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let database = path.join("core.sqlite3");
    let connection = Connection::open(&database).unwrap();
    let key: Vec<u8> = connection
        .query_row("SELECT content_key FROM sync_scopes", [], |row| row.get(0))
        .unwrap();
    assert_eq!(key.len(), 32);

    assert_eq!(
        blob_occurrences(&connection, &key),
        1,
        "the content key appears in exactly one stored value"
    );
    drop(connection);

    // And exactly once in the file itself, so no freed page still holds it.
    let raw = fs::read(&database).unwrap();
    let on_disk = raw
        .windows(key.len())
        .filter(|window| *window == key.as_slice())
        .count();
    assert_eq!(
        on_disk, 1,
        "the content key appears once in the database file"
    );

    fs::remove_dir_all(path).unwrap();
}
