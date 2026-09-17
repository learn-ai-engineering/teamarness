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

#[tokio::test]
async fn restart_authenticates_sealed_record_and_rejects_tampered_key_state() {
    let path = directory();
    let config = CoreRuntimeConfig::new(&path);
    let runtime = CoreRuntime::open(config.clone()).await.unwrap();
    let workspace = runtime.create_workspace("Recovery").unwrap();
    runtime
        .append_record(workspace.id(), "teamarness.test", "message.post", b"secret")
        .unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let connection = Connection::open(path.join("core.sqlite3")).unwrap();
    connection
        .execute("UPDATE sync_scopes SET content_key_id = zeroblob(32)", [])
        .unwrap();
    drop(connection);
    let reopened = CoreRuntime::open(config);
    assert!(matches!(
        reopened.await,
        Err(teamarness_core::CoreError::Storage(
            StorageError::CorruptWorkspace
        ))
    ));
    fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn restart_rejects_tampered_p2panda_projection() {
    let path = directory();
    let config = CoreRuntimeConfig::new(&path);
    let runtime = CoreRuntime::open(config.clone()).await.unwrap();
    let workspace = runtime.create_workspace("Projection tamper").unwrap();
    runtime
        .append_record(workspace.id(), "teamarness.test", "message.post", b"sealed")
        .unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let connection = Connection::open(path.join("core.sqlite3")).unwrap();
    connection
        .execute("DROP TRIGGER p2panda_operations_no_update", [])
        .unwrap();
    connection
        .execute(
            "UPDATE p2panda_operations SET payload_hash = zeroblob(32)",
            [],
        )
        .unwrap();
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
