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
    let workspace = cluster.node("a").create_workspace("Send budget").unwrap();
    let id = workspace.id();
    cluster.admit("a", "b", id).await;
    cluster
        .wait_until("b holds the roster", DEFAULT_DEADLINE, |c| {
            c.members("b", id).len() == 2
        })
        .await;
    // From here only the explicit sessions below move records, so each report
    // is about a session this test ran.
    pause_scheduling(&cluster, &["a", "b"]);
    for index in 0..25_u8 {
        cluster
            .node("a")
            .append_record(id, PACKAGE, "message.post", &[index; 8])
            .unwrap();
    }

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
    let workspace = cluster.node("a").create_workspace("Lazy").unwrap();
    let id = workspace.id();
    cluster.admit("a", "b", id).await;
    cluster
        .wait_until("b holds the roster", DEFAULT_DEADLINE, |c| {
            c.members("b", id).len() == 2
        })
        .await;
    pause_scheduling(&cluster, &["a", "b"]);
    for index in 0..300_u16 {
        cluster
            .node("a")
            .append_record(id, PACKAGE, "message.post", &index.to_be_bytes())
            .unwrap();
    }
    replicate(&cluster, id).await;
    assert_eq!(cluster.record_ids("b", id, PACKAGE).len(), 300);

    // A second session with a peer that already holds everything reads no row
    // of the journal at all. The count is scoped to this test's Workspace, so a
    // test running beside it in the same process cannot move it.
    let scope = crate::synchronization::scope_digest(PACKAGE, id);
    let before = crate::synchronization::records_loaded_for_test(scope);
    let report = replicate(&cluster, id).await;
    assert_eq!(report.outcomes.applied, 0);
    assert_eq!(
        crate::synchronization::records_loaded_for_test(scope),
        before,
        "a peer that is already up to date costs no reads"
    );
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn served_log_stops_before_a_quarantined_record() {
    let cluster = SyncCluster::start("quarantined-served-log", &["a", "b", "c"]).await;
    let workspace = cluster.node("a").create_workspace("Gap").unwrap();
    let id = workspace.id();
    for joiner in ["b", "c"] {
        cluster.admit("a", joiner, id).await;
    }
    cluster
        .wait_until("both joiners hold the roster", DEFAULT_DEADLINE, |c| {
            c.members("b", id).len() == 3 && c.members("c", id).len() == 3
        })
        .await;
    pause_scheduling(&cluster, &["a", "b", "c"]);
    let mut appended = Vec::new();
    for index in 0..6_u8 {
        appended.push(
            cluster
                .node("a")
                .append_record(id, PACKAGE, "message.post", &[index; 8])
                .unwrap(),
        );
    }

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
    let workspace = cluster.node("a").create_workspace("Corrupt").unwrap();
    let id = workspace.id();
    cluster.admit("a", "b", id).await;
    cluster
        .wait_until("b holds the roster", DEFAULT_DEADLINE, |c| {
            c.members("b", id).len() == 2
        })
        .await;
    pause_scheduling(&cluster, &["a", "b"]);
    for index in 0..3_u8 {
        cluster
            .node("a")
            .append_record(id, PACKAGE, "message.post", &[index; 8])
            .unwrap();
    }

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

/// A 64-byte record body that is unique for each index.
///
/// Appends are content-idempotent, so identical bytes would commit one record.
fn body(index: u16) -> Vec<u8> {
    let mut bytes = index.to_be_bytes().to_vec();
    bytes.resize(64, 7);
    bytes
}

/// Stops scheduling on the named nodes, so only explicit sessions run.
fn pause_scheduling(cluster: &SyncCluster, names: &[&str]) {
    for name in names {
        cluster.node(name).pause_sync_scheduler_for_test(true);
    }
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

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offline_peer_does_not_delay_other_peers() {
    let mut cluster = SyncCluster::start("offline-peer", &["a", "b", "c"]).await;
    let workspace = cluster.node("a").create_workspace("Offline").unwrap();
    let id = workspace.id();
    for joiner in ["b", "c"] {
        cluster.admit("a", joiner, id).await;
    }
    cluster
        .wait_until("both joiners hold the roster", DEFAULT_DEADLINE, |c| {
            c.members("b", id).len() == 3 && c.members("c", id).len() == 3
        })
        .await;

    // C goes away; A keeps its own schedule for B.
    let absent = cluster.identity("c");
    cluster.stop("c").await;
    let record = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"while c is away")
        .unwrap();

    cluster
        .wait_until("b receives the record", DEFAULT_DEADLINE, |c| {
            c.record_ids("b", id, PACKAGE).contains(&record.record_id())
        })
        .await;
    cluster
        .wait_until("a records c as failing", DEFAULT_DEADLINE, |c| {
            c.node("a")
                .scheduler_snapshot_for_test()
                .iter()
                .any(|peer| peer.peer == absent && peer.failures >= 1)
        })
        .await;
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R07, TEAM.SYS.PKG-R10
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catching_up_is_reported_between_truncated_sessions() {
    let limits = SessionLimits {
        operations: 100,
        ..SessionLimits::CURRENT
    };
    let cluster =
        SyncCluster::start_with_limits("catching-up-states", &["a", "b"], &[("a", limits)]).await;
    let workspace = cluster.node("a").create_workspace("Catch up").unwrap();
    let id = workspace.id();
    cluster.admit("a", "b", id).await;
    cluster
        .wait_until("b holds the roster", DEFAULT_DEADLINE, |c| {
            c.members("b", id).len() == 2
        })
        .await;
    for index in 0..1_500_u16 {
        cluster
            .node("a")
            .append_record(id, PACKAGE, "message.post", &index.to_be_bytes())
            .unwrap();
    }

    let mut subscription = cluster
        .node("b")
        .subscribe_records(id, PACKAGE, None)
        .unwrap();
    let mut delivered = 0_usize;
    let mut states: Vec<crate::SyncAvailability> = Vec::new();
    let mut last = None;
    // The deadline only bounds failure.
    let expiry = std::time::Instant::now() + Duration::from_secs(90);
    loop {
        delivered += subscription.next_page(32).unwrap().events.len();
        let reported = subscription.availability().unwrap();
        if last != Some(reported.availability) {
            states.push(reported.availability);
            last = Some(reported.availability);
        }
        if delivered == 1_500
            && reported.availability == crate::SyncAvailability::Current
            && reported.pending_records == 0
        {
            break;
        }
        assert!(
            std::time::Instant::now() < expiry,
            "b catches up with a: delivered {delivered}, states {states:?}"
        );
        // B's own sessions are what give it a view of reaching A; A's pushes
        // are what fill the journal.
        cluster.node("a").sync_now_for_test();
        cluster.node("b").sync_now_for_test();
        // poll-interval: the interval of this loop, which is bounded by its own deadline.
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert!(
        states.contains(&crate::SyncAvailability::CatchingUp),
        "truncated sessions leave more to send, which is reported: {states:?}"
    );
    assert_eq!(states.last(), Some(&crate::SyncAvailability::Current));
    assert!(
        !states.contains(&crate::SyncAvailability::Offline),
        "the only peer answered every session: {states:?}"
    );
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catch_up_of_5000_records_converges_across_bounded_sessions() {
    let cluster = SyncCluster::start("catch-up-5000", &["a", "b"]).await;
    let workspace = cluster.node("a").create_workspace("Large").unwrap();
    let id = workspace.id();
    let started = std::time::Instant::now();
    for index in 0..5_000_u16 {
        // Appends are content-idempotent, so every record's bytes differ.
        cluster
            .node("a")
            .append_record(id, PACKAGE, "message.post", &body(index))
            .unwrap();
    }
    let appended = started.elapsed();
    assert_eq!(cluster.record_count("a", id, PACKAGE), 5_000);
    cluster.admit("a", "b", id).await;

    let converged = std::time::Instant::now();
    cluster
        .wait_until("b holds every record", Duration::from_secs(90), |c| {
            c.record_count("b", id, PACKAGE) == 5_000
        })
        .await;
    println!(
        "5000-record catch-up: appended in {appended:?}, converged in {:?}",
        converged.elapsed()
    );

    let a_reports = cluster.node("a").session_reports_for_test();
    let b_reports = cluster.node("b").session_reports_for_test();
    let a = cluster.identity("a");
    let b = cluster.identity("b");
    // Either peer can initiate the bounded exchange. A push records its own
    // sent_truncated flag; a B-initiated pull records the peer_truncated flag.
    let truncated = a_reports
        .iter()
        .filter(|(peer, report)| *peer == b && report.sent_truncated)
        .count()
        + b_reports
            .iter()
            .filter(|(peer, report)| *peer == a && report.peer_truncated)
            .count();
    assert!(
        truncated >= 2,
        "a backlog above one session's bound continues in later sessions: outbound={a_reports:?}, inbound={b_reports:?}"
    );
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_large_backlogs_converge_in_both_directions() {
    let mut cluster = SyncCluster::start("both-directions", &["a", "b"]).await;
    let workspace = cluster.node("a").create_workspace("Both ways").unwrap();
    let id = workspace.id();
    // The scope must exist before B is admitted, or B never receives a content
    // key and its own appends have nowhere to land.
    cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"seed")
        .unwrap();
    cluster.admit("a", "b", id).await;
    cluster
        .wait_until("b holds the seed record", DEFAULT_DEADLINE, |c| {
            c.members("b", id).len() == 2 && c.record_count("b", id, PACKAGE) == 1
        })
        .await;
    cluster.stop("b").await;
    for index in 0..3_000_u16 {
        cluster
            .node("a")
            .append_record(id, PACKAGE, "message.post", &body(index))
            .unwrap();
    }
    cluster.stop("a").await;
    cluster.restart("b").await;
    for index in 0..3_000_u16 {
        cluster
            .node("b")
            .append_record(id, PACKAGE, "message.post", &body(index))
            .unwrap();
    }
    cluster.restart("a").await;

    cluster
        .wait_until(
            "both installations hold every record",
            Duration::from_secs(90),
            |c| {
                c.record_count("a", id, PACKAGE) == 6_001
                    && c.record_count("b", id, PACKAGE) == 6_001
            },
        )
        .await;
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn non_member_connection_flood_is_refused_without_reading() {
    let cluster = SyncCluster::start("connection-flood", &["a", "x"]).await;
    let workspace = cluster.node("a").create_workspace("Flood").unwrap();
    let id = workspace.id();
    cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"before")
        .unwrap();

    let address = cluster.address("a");
    let mut held = Vec::new();
    for _ in 0..20 {
        // A refused stream is the point: the responder bounds concurrency
        // before it reads anything, and X is not even a Member.
        if let Some(connection) = cluster
            .node("x")
            .try_hold_package_sync_before_hello_for_test(address.clone())
            .await
        {
            held.push(connection);
        }
    }
    // The connections exist on X's side either way; what matters is how many
    // the responder let into a session.
    assert!(
        cluster.node("a").inbound_package_sessions_for_test() <= 8,
        "the responder never holds more than its inbound bound"
    );
    let accepted = cluster
        .node("a")
        .inbound_sessions_from_for_test(cluster.identity("x"));
    assert!(
        accepted <= 2,
        "one peer never gets more than its per-peer bound: {accepted} accepted"
    );

    // A local append is not delayed by the flood. The bound is failure only.
    let started = std::time::Instant::now();
    cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"during")
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the flood did not delay a local append"
    );

    // Every accepted session is closed by the hello timeout, so the slots free
    // even though X never says anything.
    let expiry = std::time::Instant::now() + Duration::from_secs(10);
    while cluster.node("a").inbound_package_sessions_for_test() > 0 {
        assert!(
            cluster.node("a").inbound_package_sessions_for_test() <= 8,
            "the inbound bound holds for as long as the flood does"
        );
        assert!(
            std::time::Instant::now() < expiry,
            "a silent peer holds a slot for at most the hello timeout"
        );
        // poll-interval: the interval of this loop, which is bounded by its own deadline.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    drop(held);
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R07, TEAM.SYS.MEM-R06
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revocation_during_backoff_prevents_redial() {
    let mut cluster = SyncCluster::start("revoke-in-backoff", &["a", "b"]).await;
    let workspace = cluster.node("a").create_workspace("Revoked").unwrap();
    let id = workspace.id();
    cluster.admit("a", "b", id).await;
    cluster
        .wait_until("b holds the roster", DEFAULT_DEADLINE, |c| {
            c.members("b", id).len() == 2
        })
        .await;
    cluster.stop("b").await;
    cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"while away")
        .unwrap();
    // A's jobs to B now fail and back off.
    cluster
        .wait_until("a has failed to reach b", DEFAULT_DEADLINE, |c| {
            c.node("a")
                .scheduler_snapshot_for_test()
                .iter()
                .any(|peer| peer.peer == c.identity("b") && peer.failures >= 1)
        })
        .await;

    cluster
        .node("a")
        .revoke_member(id, cluster.identity("b"))
        .unwrap();
    cluster.restart("b").await;
    cluster
        .node("b")
        .pause_membership_replication_for_test(true);
    let b = cluster.identity("b");
    let before = cluster
        .node("b")
        .inbound_sessions_from_for_test(cluster.identity("a"));

    let until = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < until {
        cluster.node("a").sync_now_for_test();
        assert!(
            cluster
                .node("a")
                .scheduler_snapshot_for_test()
                .iter()
                .all(|peer| peer.peer != b),
            "a revoked peer is not scheduled"
        );
        assert_eq!(
            cluster
                .node("b")
                .inbound_sessions_from_for_test(cluster.identity("a")),
            before,
            "a revoked peer is never dialled out of backoff"
        );
        // negative-wait: a bounded window proving a revoked peer is never dialled.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_is_prompt_with_pending_backoff_and_an_inbound_session() {
    let mut cluster = SyncCluster::start("prompt-shutdown", &["a", "b", "c"]).await;
    let workspace = cluster.node("a").create_workspace("Shutdown").unwrap();
    let id = workspace.id();
    cluster.admit("a", "b", id).await;
    cluster.admit("a", "c", id).await;
    cluster
        .wait_until("both members hold the roster", DEFAULT_DEADLINE, |c| {
            c.members("b", id).len() == 3 && c.members("c", id).len() == 3
        })
        .await;
    // C goes away, so A's jobs to C fail and back off.
    cluster.stop("c").await;
    cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"pending")
        .unwrap();
    cluster
        .wait_until("a has failed to reach c", DEFAULT_DEADLINE, |c| {
            c.node("a")
                .scheduler_snapshot_for_test()
                .iter()
                .any(|peer| peer.peer == c.identity("c") && peer.failures >= 1)
        })
        .await;

    // B holds an inbound session on A open while the shutdown starts.
    let held = cluster
        .node("b")
        .hold_package_sync_before_hello_for_test(cluster.address("a"))
        .await;

    let started = std::time::Instant::now();
    cluster.stop("a").await;
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(5),
        "shutdown does not wait out backoff or an idle inbound session: {elapsed:?}"
    );
    drop(held);
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R07, TEAM.SYS.PKG-R10
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn peer_saturation_preserves_reachable_unknown_and_failed_health() {
    use crate::synchronization::PeerOutcome;
    for (name, prior, expected) in [
        (
            "busy-reachable",
            Some(PeerOutcome::Reachable),
            crate::SyncAvailability::Current,
        ),
        ("busy-unknown", None, crate::SyncAvailability::CatchingUp),
        (
            "busy-failed",
            Some(PeerOutcome::Failed),
            crate::SyncAvailability::Offline,
        ),
    ] {
        let cluster = SyncCluster::start(name, &["a", "b"]).await;
        let workspace = cluster
            .node("a")
            .create_workspace("Busy peer")
            .unwrap()
            .id();
        cluster.admit("a", "b", workspace).await;
        cluster
            .wait_until("both nodes have the roster", DEFAULT_DEADLINE, |c| {
                c.members("b", workspace).len() == 2
            })
            .await;
        pause_scheduling(&cluster, &["a", "b"]);
        cluster
            .wait_until("outbound sessions are idle", DEFAULT_DEADLINE, |c| {
                ["a", "b"].iter().all(|name| {
                    c.node(name)
                        .scheduler_snapshot_for_test()
                        .iter()
                        .all(|peer| !peer.in_flight)
                })
            })
            .await;
        let _origin = cluster
            .node("a")
            .subscribe_records(workspace, PACKAGE, None)
            .unwrap();
        let mut subscription = cluster
            .node("b")
            .subscribe_records(workspace, PACKAGE, None)
            .unwrap();
        cluster
            .node("b")
            .replicate_records_to_address_for_test(
                workspace,
                PACKAGE,
                cluster.address("a"),
                cluster.identity("a"),
            )
            .await
            .unwrap();
        cluster
            .node("b")
            .append_record(workspace, PACKAGE, "message.post", b"due work")
            .unwrap();
        let initial = subscription.next_page(32).unwrap();
        let live = subscription.next_page(32).unwrap();
        assert_eq!(
            initial.events.len() + live.events.len(),
            1,
            "drain the captured empty prefix and the later local record"
        );
        cluster.node("b").health.record_outcome(
            crate::synchronization::scope_digest(PACKAGE, workspace),
            cluster.identity("a"),
            prior,
            None,
        );
        assert_eq!(subscription.availability().unwrap().availability, expected);
        // Reading the application acknowledgement completes the initiator before
        // the responder's send.stopped() necessarily releases its inbound slot.
        cluster
            .wait_until(
                "setup responder slots are released",
                DEFAULT_DEADLINE,
                |c| {
                    c.node("a").inbound_package_sessions_for_test() == 0
                        && c.node("b").inbound_package_sessions_for_test() == 0
                },
            )
            .await;
        // Hold the real responder's two per-peer slots before Hello. The third
        // connection is refused as busy while the peer stays alive and reachable.
        let first = cluster
            .node("b")
            .hold_package_sync_before_hello_for_test(cluster.address("a"))
            .await;
        let second = cluster
            .node("b")
            .hold_package_sync_before_hello_for_test(cluster.address("a"))
            .await;
        cluster
            .wait_until("both inbound slots are held", DEFAULT_DEADLINE, |c| {
                c.node("a").inbound_package_sessions_for_test() == 2
            })
            .await;
        cluster.node("b").pause_sync_scheduler_for_test(false);
        cluster.node("b").sync_now_for_test();
        cluster
            .wait_until(
                "the busy session enters bounded retry backoff",
                DEFAULT_DEADLINE,
                |c| {
                    c.node("b")
                        .scheduler_snapshot_for_test()
                        .iter()
                        .any(|p| p.peer == c.identity("a") && p.failures > 0 && !p.in_flight)
                },
            )
            .await;
        assert_eq!(
            subscription.availability().unwrap().availability,
            expected,
            "busy must preserve {name}"
        );
        drop((first, second));
        cluster.finish().await;
    }
}
