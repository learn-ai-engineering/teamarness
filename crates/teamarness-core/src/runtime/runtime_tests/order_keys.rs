//! Order keys are bound to their record, bounded in height, and commit times
//! come from the runtime clock.
//!
//! Real components: real [`CoreRuntime`](super::super::CoreRuntime) instances on
//! temporary directories with real SQLite. The only injected pieces are a
//! forged order key and, for commit times, a fixed clock.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use super::super::{Clock, CoreRuntime, CoreRuntimeConfig, SystemClock};
use super::cluster::cluster_root;
use crate::synchronization::limits::MAX_HEIGHT_ADVANCE;
use crate::synchronization::{RemoteInsert, RemoteInsertError, forge_order_key_operation_for_test};

const PACKAGE: &str = "teamarness.test";

/// A clock a test sets by hand, so a commit time is an assertion, not a guess.
struct FixedClock(Arc<AtomicI64>);

impl Clock for FixedClock {
    fn now_unix_ms(&self) -> i64 {
        self.0.load(Ordering::Acquire)
    }
}

async fn runtime(test: &str) -> (CoreRuntime, std::path::PathBuf) {
    let root = cluster_root(test);
    let _ = std::fs::remove_dir_all(&root);
    let runtime = CoreRuntime::open_with_clock_and_readiness(
        CoreRuntimeConfig::new(root.join("node")),
        Box::new(SystemClock),
        true,
    )
    .await
    .expect("the Installation opens");
    (runtime, root)
}

/// Verifies: TEAM.SYS.TRANS-R06, TEAM.SYS.TRANS-R07
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_height_beyond_the_advance_window_is_rejected() {
    let (runtime, root) = runtime("height-window").await;
    let workspace = runtime.create_workspace("Height window").unwrap();
    let id = workspace.id();
    let seed = runtime
        .append_record(id, PACKAGE, "message.post", b"seed")
        .unwrap();

    let outcome = {
        let mut state = runtime.state.lock().unwrap();
        let secret = state.secret_key.clone();
        let now = state.clock.now_unix_ms();
        let operation = forge_order_key_operation_for_test(
            &state.connection,
            &secret,
            id,
            PACKAGE,
            b"too far ahead",
            |record| {
                crate::synchronization::OrderKey::from_height_and_id(MAX_HEIGHT_ADVANCE + 1, record)
                    .as_bytes()
            },
        );
        crate::synchronization::insert_remote_operation(
            &mut state.connection,
            id,
            PACKAGE,
            &operation,
            now,
        )
    };

    assert_eq!(outcome.err(), Some(RemoteInsertError::HeightOutOfRange));
    let mut delivered = runtime.subscribe_records(id, PACKAGE, None).unwrap();
    let mut seen = Vec::new();
    while let Ok(Some(event)) = delivered.next_event_timeout(std::time::Duration::ZERO) {
        seen.push(event.record_id());
    }
    assert_eq!(seen, vec![seed.record_id()], "the journal is unchanged");

    runtime.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_append_after_a_maximum_window_height_does_not_saturate() {
    let (runtime, root) = runtime("height-no-saturation").await;
    let workspace = runtime.create_workspace("Window edge").unwrap();
    let id = workspace.id();
    runtime
        .append_record(id, PACKAGE, "message.post", b"seed")
        .unwrap();

    // A record right at the edge of the window is accepted.
    {
        let mut state = runtime.state.lock().unwrap();
        let secret = state.secret_key.clone();
        let now = state.clock.now_unix_ms();
        let operation = forge_order_key_operation_for_test(
            &state.connection,
            &secret,
            id,
            PACKAGE,
            b"at the window edge",
            |record| {
                crate::synchronization::OrderKey::from_height_and_id(MAX_HEIGHT_ADVANCE, record)
                    .as_bytes()
            },
        );
        let applied = crate::synchronization::insert_remote_operation(
            &mut state.connection,
            id,
            PACKAGE,
            &operation,
            now,
        )
        .expect("a height inside the window applies");
        assert!(matches!(applied, RemoteInsert::Applied { .. }));
    }

    let first = runtime
        .append_record(id, PACKAGE, "message.post", b"after the edge")
        .unwrap();
    let second = runtime
        .append_record(id, PACKAGE, "message.post", b"after that")
        .unwrap();

    assert_eq!(first.order_key().height(), MAX_HEIGHT_ADVANCE + 1);
    assert_eq!(second.order_key().height(), MAX_HEIGHT_ADVANCE + 2);
    assert_ne!(first.order_key(), second.order_key());

    runtime.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delivery_rows_record_commit_time() {
    let root = cluster_root("delivery-commit-time");
    let _ = std::fs::remove_dir_all(&root);
    let now = Arc::new(AtomicI64::new(1_700_000_000_000));
    let runtime = CoreRuntime::open_with_clock_and_readiness(
        CoreRuntimeConfig::new(root.join("node")),
        Box::new(FixedClock(Arc::clone(&now))),
        true,
    )
    .await
    .expect("the Installation opens");
    let workspace = runtime.create_workspace("Commit time").unwrap();
    let id = workspace.id();

    let first = runtime
        .append_record(id, PACKAGE, "message.post", b"one")
        .unwrap();
    now.store(1_700_000_060_000, Ordering::Release);
    let second = runtime
        .append_record(id, PACKAGE, "message.post", b"two")
        .unwrap();

    let committed = |record: &crate::RecordId| -> i64 {
        runtime
            .state
            .lock()
            .unwrap()
            .connection
            .query_row(
                "SELECT created_at_unix_ms FROM record_delivery WHERE record_id=?1",
                [record.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .expect("a delivery row per committed record")
    };
    assert_eq!(committed(&first.record_id()), 1_700_000_000_000);
    assert_eq!(committed(&second.record_id()), 1_700_000_060_000);

    runtime.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

/// Verifies: TEAM.SYS.MEM-R06, TEAM.SYS.TRANS-R07
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn append_by_revoked_member_reports_revoked_member() {
    let cluster = super::cluster::SyncCluster::start("revoked-append", &["a", "b"]).await;
    let workspace = cluster.node("a").create_workspace("Revocation").unwrap();
    let id = workspace.id();
    cluster.admit("a", "b", id).await;
    // While B is a Member, an append is refused only for want of the scope's
    // content key, never for membership.
    assert_eq!(
        cluster
            .node("b")
            .append_record(id, PACKAGE, "message.post", b"while a member")
            .expect_err("B does not hold the scope's content key yet"),
        crate::SynchronizationError::ContentKeyUnavailable
    );

    cluster
        .node("a")
        .revoke_member(id, cluster.identity("b"))
        .expect("the manager revokes");
    cluster.replicate_membership("a", "b", id).await;
    cluster
        .wait_until(
            "the revoked member accepted its revocation",
            DEADLINE,
            |c| !c.members("b", id).contains(&c.identity("b")),
        )
        .await;

    let refused = cluster
        .node("b")
        .append_record(id, PACKAGE, "message.post", b"after revocation")
        .expect_err("a revoked Installation may not append");

    assert_eq!(refused, crate::SynchronizationError::RevokedMember);
    cluster.finish().await;
}

const DEADLINE: std::time::Duration = super::cluster::DEFAULT_DEADLINE;
