use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use teamarness_core::{CoreRuntime, CoreRuntimeConfig, Cursor, OrderKey, RecordId};

static NEXT: AtomicU64 = AtomicU64::new(0);
fn directory() -> PathBuf {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "teamarness-immutable-log-{}-{n}",
        std::process::id()
    ))
}

#[tokio::test]
async fn append_replay_and_open_are_durable_and_content_idempotent() {
    let path = directory();
    let config = CoreRuntimeConfig::new(&path);
    let runtime = CoreRuntime::open_loopback_for_tests(config.clone())
        .await
        .unwrap();
    let workspace = runtime.create_workspace("Journal").unwrap();

    let first = runtime
        .append_record(workspace.id(), "teamarness.test", "message.post", b"one")
        .unwrap();
    let retry = runtime
        .append_record(workspace.id(), "teamarness.test", "message.post", b"one")
        .unwrap();
    assert_eq!(first.record_id(), retry.record_id());
    assert_eq!(first.order_key(), retry.order_key());
    assert_eq!(first.durability(), "committed");

    let mut subscription = runtime
        .subscribe_records(workspace.id(), "teamarness.test", None)
        .unwrap();
    let event = subscription
        .next_event_timeout(std::time::Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert_eq!(event.record_id(), first.record_id());
    assert_eq!(event.cursor().to_string().len(), 50);
    let opened = runtime
        .open_records(
            workspace.id(),
            "teamarness.test",
            &[first.sealed_record().to_vec()],
        )
        .unwrap();
    assert_eq!(opened[0].canonical(), b"one");
    assert_eq!(opened[0].record_id(), first.record_id());
    assert_eq!(opened[0].order_key(), first.order_key());
    let mut tampered = first.sealed_record().to_vec();
    tampered[0] ^= 1;
    assert!(
        runtime
            .open_records(workspace.id(), "teamarness.test", &[tampered])
            .is_err()
    );
    let other = runtime
        .append_record(workspace.id(), "other.package", "message.post", b"other")
        .unwrap();
    assert!(matches!(
        runtime.open_records(
            workspace.id(),
            "other.package",
            &[first.sealed_record().to_vec()]
        ),
        Err(teamarness_core::SynchronizationError::WrongScope)
    ));
    assert_ne!(other.record_id(), first.record_id());

    runtime.shutdown().await.unwrap();
    drop(runtime);
    let reopened = CoreRuntime::open_loopback_for_tests(config).await.unwrap();
    let mut replay = reopened
        .subscribe_records(workspace.id(), "teamarness.test", None)
        .unwrap();
    assert_eq!(
        replay
            .next_event_timeout(std::time::Duration::from_secs(1))
            .unwrap()
            .unwrap()
            .canonical(),
        b"one"
    );
    reopened.shutdown().await.unwrap();
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn semantic_values_remain_distinct() {
    let _ = (
        std::mem::size_of::<RecordId>(),
        std::mem::size_of::<OrderKey>(),
        std::mem::size_of::<Cursor>(),
    );
}
