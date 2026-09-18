//! Package synchronization is bounded in concurrency, in time, and in what one
//! session may exchange.
//!
//! Real components: real [`CoreRuntime`](super::super::CoreRuntime)s on
//! temporary directories, real Iroh endpoints in loopback mode with the
//! in-memory address book, real SQLite. Only the numbers are injected.

use std::time::Duration;

use super::cluster::{DEFAULT_DEADLINE, SyncCluster};
use crate::synchronization::LogSessionError;
use crate::synchronization::limits::SessionLimits;

const PACKAGE: &str = "teamarness.test";

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inbound_package_sync_sessions_are_capped() {
    let limits = SessionLimits {
        inbound_sessions: 2,
        ..SessionLimits::CURRENT
    };
    let cluster =
        SyncCluster::start_with_limits("inbound-capped", &["a", "x", "y", "z"], &[("a", limits)])
            .await;
    let address = cluster.address("a");

    // Two peers open a stream and then say nothing, so each holds one of A's
    // two inbound slots until its Hello deadline.
    let held_x = cluster
        .node("x")
        .hold_package_sync_before_hello_for_test(address.clone())
        .await;
    let held_y = cluster
        .node("y")
        .hold_package_sync_before_hello_for_test(address.clone())
        .await;
    cluster
        .wait_until("both inbound slots are held", DEFAULT_DEADLINE, |c| {
            c.node("a").inbound_package_sessions_for_test() == 2
        })
        .await;

    let refused = cluster
        .node("z")
        .package_sync_close_code_for_test(address, Duration::from_secs(5))
        .await;
    assert_eq!(
        refused,
        Some(2),
        "a third inbound session is closed as overloaded before it is read"
    );
    assert_eq!(
        cluster.node("a").inbound_package_sessions_for_test(),
        2,
        "the refused session never took a slot"
    );

    drop(held_x);
    drop(held_y);
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn silent_peer_is_dropped_after_the_hello_timeout() {
    let limits = SessionLimits {
        hello_timeout: Duration::from_millis(200),
        ..SessionLimits::CURRENT
    };
    let cluster =
        SyncCluster::start_with_limits("silent-peer", &["a", "x"], &[("a", limits)]).await;

    let close_code = cluster
        .node("x")
        .stalled_package_sync_close_code_for_test(cluster.address("a"), Duration::from_secs(5))
        .await;
    assert_eq!(
        close_code,
        Some(4),
        "a peer that never sends its opening message is dropped"
    );
    assert_eq!(cluster.node("a").inbound_package_sessions_for_test(), 0);
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_budget_exceeded_closes_with_code_7() {
    let limits = SessionLimits {
        operations: 4,
        ..SessionLimits::CURRENT
    };
    let cluster =
        SyncCluster::start_with_limits("session-budget", &["a", "b"], &[("b", limits)]).await;
    let workspace = cluster.node("a").create_workspace("Budget").unwrap();
    let id = workspace.id();
    for index in 0..5_u8 {
        cluster
            .node("a")
            .append_record(id, PACKAGE, "message.post", &[index; 8])
            .unwrap();
    }
    cluster.admit("a", "b", id).await;
    cluster.replicate_membership("a", "b", id).await;
    cluster
        .wait_until("b holds the roster", DEFAULT_DEADLINE, |c| {
            c.members("b", id).len() == 2
        })
        .await;

    let result = cluster
        .node("a")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("b"),
            cluster.identity("b"),
        )
        .await;
    assert_eq!(
        result.err(),
        Some(LogSessionError::BudgetExceeded),
        "the responder stops the session at its operation bound"
    );
    assert_eq!(
        cluster.record_ids("b", id, PACKAGE).len(),
        0,
        "a peer that announces more than the bound is refused before any \
         operation is exchanged, so nothing is buffered for it"
    );

    // The same peer converges once the session bound covers what it offers.
    let generous = SessionLimits {
        operations: 8,
        ..SessionLimits::CURRENT
    };
    let mut cluster = cluster;
    cluster.set_limits_for_test("b", generous);
    cluster.stop("b").await;
    cluster.restart("b").await;
    cluster
        .wait_until("b receives every record", DEFAULT_DEADLINE, |c| {
            c.record_ids("b", id, PACKAGE).len() == 5
        })
        .await;
    cluster.finish().await;
}
