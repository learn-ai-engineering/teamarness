use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::Connection;
use teamarness_core::{CoreRuntime, CoreRuntimeConfig, StorageError};

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
    let kind: String = connection
        .query_row(
            "SELECT operation_kind FROM protection_control_operations LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kind, "create");
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
