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

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_stops_at_the_send_budget_and_reports_truncation() {
    let limits = SessionLimits {
        operations: 10,
        ..SessionLimits::CURRENT
    };
    let cluster =
        SyncCluster::start_with_limits("send-budget", &["a", "b"], &[("a", limits)]).await;
    // Only the explicit sessions below move records, so each report is about a
    // session this test ran.
    cluster.node("a").pause_record_replication_for_test(true);
    cluster.node("b").pause_record_replication_for_test(true);
    let workspace = cluster.node("a").create_workspace("Send budget").unwrap();
    let id = workspace.id();
    for index in 0..25_u8 {
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

    let first = replicate(&cluster, id).await;
    assert!(first.sent_truncated, "the first session stops at its bound");
    assert_eq!(cluster.record_ids("b", id, PACKAGE).len(), 10);

    let second = replicate(&cluster, id).await;
    assert!(second.sent_truncated);
    assert_eq!(cluster.record_ids("b", id, PACKAGE).len(), 20);

    let third = replicate(&cluster, id).await;
    assert!(
        !third.sent_truncated,
        "the last session had nothing left to hold back"
    );
    assert_eq!(cluster.record_ids("b", id, PACKAGE).len(), 25);
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_does_not_load_the_whole_scope() {
    let cluster = SyncCluster::start("no-whole-scope-load", &["a", "b"]).await;
    cluster.node("a").pause_record_replication_for_test(true);
    cluster.node("b").pause_record_replication_for_test(true);
    let workspace = cluster.node("a").create_workspace("Lazy").unwrap();
    let id = workspace.id();
    for index in 0..300_u16 {
        cluster
            .node("a")
            .append_record(id, PACKAGE, "message.post", &index.to_be_bytes())
            .unwrap();
    }
    cluster.admit("a", "b", id).await;
    cluster.replicate_membership("a", "b", id).await;
    cluster
        .wait_until("b holds the roster", DEFAULT_DEADLINE, |c| {
            c.members("b", id).len() == 2
        })
        .await;
    replicate(&cluster, id).await;
    assert_eq!(cluster.record_ids("b", id, PACKAGE).len(), 300);

    // A second session with a peer that already holds everything reads no row
    // of the journal at all.
    let before = crate::synchronization::records_loaded_for_test();
    let report = replicate(&cluster, id).await;
    assert_eq!(report.outcomes.applied, 0);
    assert_eq!(
        crate::synchronization::records_loaded_for_test(),
        before,
        "a peer that is already up to date costs no reads"
    );
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn served_log_stops_before_a_quarantined_record() {
    let cluster = SyncCluster::start("quarantined-served-log", &["a", "b", "c"]).await;
    cluster.node("a").pause_record_replication_for_test(true);
    cluster.node("b").pause_record_replication_for_test(true);
    cluster.node("c").pause_record_replication_for_test(true);
    let workspace = cluster.node("a").create_workspace("Gap").unwrap();
    let id = workspace.id();
    let mut appended = Vec::new();
    for index in 0..6_u8 {
        appended.push(
            cluster
                .node("a")
                .append_record(id, PACKAGE, "message.post", &[index; 8])
                .unwrap(),
        );
    }
    for joiner in ["b", "c"] {
        cluster.admit("a", joiner, id).await;
        cluster.replicate_membership("a", joiner, id).await;
    }
    cluster
        .wait_until("both joiners hold the roster", DEFAULT_DEADLINE, |c| {
            c.members("b", id).len() == 3 && c.members("c", id).len() == 3
        })
        .await;

    // C takes the whole log first, then A isolates its own fourth record.
    cluster
        .node("a")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("c"),
            cluster.identity("c"),
        )
        .await
        .expect("c takes the whole log");
    assert_eq!(cluster.record_ids("c", id, PACKAGE).len(), 6);
    cluster
        .node("a")
        .quarantine_record_for_test(id, PACKAGE, appended[3].record_id());

    cluster
        .node("a")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("b"),
            cluster.identity("b"),
        )
        .await
        .expect("a serves what is still sound");
    assert_eq!(
        cluster.record_ids("b", id, PACKAGE).len(),
        3,
        "the served log stops before the isolated record"
    );

    // Another holder still has the complete log, so B converges through it.
    cluster
        .node("c")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("b"),
            cluster.identity("b"),
        )
        .await
        .expect("c completes b's log");
    assert_eq!(cluster.record_ids("b", id, PACKAGE).len(), 6);
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn corrupt_projection_row_fails_the_range_and_quarantines_the_scope() {
    let cluster = SyncCluster::start("corrupt-served-row", &["a", "b"]).await;
    cluster.node("a").pause_record_replication_for_test(true);
    cluster.node("b").pause_record_replication_for_test(true);
    let workspace = cluster.node("a").create_workspace("Corrupt").unwrap();
    let id = workspace.id();
    for index in 0..3_u8 {
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

    cluster.node("a").tamper_payload_hash_for_test(id, PACKAGE);
    let result = cluster
        .node("a")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("b"),
            cluster.identity("b"),
        )
        .await;
    assert!(result.is_err(), "a corrupt row ends the session");
    assert!(
        cluster.node("a").scope_is_quarantined_for_test(id, PACKAGE),
        "the scope is isolated when its projection no longer authenticates"
    );
    assert!(matches!(
        cluster
            .node("a")
            .append_record(id, PACKAGE, "message.post", b"after"),
        Err(crate::SynchronizationError::ScopeQuarantined)
    ));
    cluster.finish().await;
}

/// Runs one explicit session from A to B and returns its report.
async fn replicate(
    cluster: &SyncCluster,
    workspace: crate::WorkspaceId,
) -> crate::synchronization::ExchangeReport {
    cluster
        .node("a")
        .replicate_records_to_address_for_test(
            workspace,
            PACKAGE,
            cluster.address("b"),
            cluster.identity("b"),
        )
        .await
        .expect("the session completes")
}
