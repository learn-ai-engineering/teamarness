//! Canonical membership reaches every Member, over `teamarness/membership-sync/1`.
//!
//! Real components: real `CoreRuntime`s on temporary directories, real Iroh
//! endpoints in loopback mode with the in-memory address book, real SQLite.
//! Nothing about membership replication is stubbed in these tests.

use std::collections::BTreeSet;
use std::time::Duration;

use super::cluster::{DEFAULT_DEADLINE, SyncCluster};

const PACKAGE: &str = "teamarness.test";

/// Verifies: TEAM.SYS.MEM-R10
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admitted_members_learn_each_other_through_the_manager() {
    let cluster = SyncCluster::start("learn-each-other", &["a", "b", "c"]).await;
    let workspace = cluster.node("a").create_workspace("Roster").unwrap();
    let id = workspace.id();
    cluster.admit("a", "b", id).await;
    cluster.admit("a", "c", id).await;
    let everyone = BTreeSet::from([
        cluster.identity("a"),
        cluster.identity("b"),
        cluster.identity("c"),
    ]);

    cluster
        .wait_until("every member knows every member", DEFAULT_DEADLINE, |c| {
            c.members("b", id) == everyone && c.members("c", id) == everyone
        })
        .await;
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.MEM-R10, TEAM.SYS.TRANS-R06
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn members_converge_on_records_after_the_manager_goes_offline() {
    let mut cluster = SyncCluster::start("manager-offline", &["a", "b", "c"]).await;
    let workspace = cluster.node("a").create_workspace("Offline").unwrap();
    let id = workspace.id();
    let a1 = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"a1")
        .unwrap();
    cluster.admit("a", "b", id).await;
    cluster.admit("a", "c", id).await;
    let everyone = BTreeSet::from([
        cluster.identity("a"),
        cluster.identity("b"),
        cluster.identity("c"),
    ]);
    cluster
        .wait_until(
            "both joiners hold the roster and the first record",
            DEFAULT_DEADLINE,
            |c| {
                c.members("b", id) == everyone
                    && c.members("c", id) == everyone
                    && c.record_ids("b", id, PACKAGE).contains(&a1.record_id())
                    && c.record_ids("c", id, PACKAGE).contains(&a1.record_id())
            },
        )
        .await;

    cluster.stop("a").await;
    let b1 = cluster
        .node("b")
        .append_record(id, PACKAGE, "message.post", b"b1")
        .unwrap();
    let c1 = cluster
        .node("c")
        .append_record(id, PACKAGE, "message.post", b"c1")
        .unwrap();

    let expected = BTreeSet::from([a1.record_id(), b1.record_id(), c1.record_id()]);
    cluster
        .wait_until(
            "the remaining members converge without the manager",
            DEFAULT_DEADLINE,
            |c| {
                c.record_ids("b", id, PACKAGE).is_superset(&expected)
                    && c.record_ids("c", id, PACKAGE).is_superset(&expected)
            },
        )
        .await;
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offline_member_catches_up_on_restart() {
    let mut cluster = SyncCluster::start("offline-catch-up", &["a", "b", "c", "d"]).await;
    let workspace = cluster.node("a").create_workspace("Catch up").unwrap();
    let id = workspace.id();
    cluster.admit("a", "b", id).await;
    cluster.stop("b").await;

    cluster.admit("a", "c", id).await;
    cluster.admit("a", "d", id).await;
    cluster.restart("b").await;

    let everyone = BTreeSet::from([
        cluster.identity("a"),
        cluster.identity("b"),
        cluster.identity("c"),
        cluster.identity("d"),
    ]);
    cluster
        .wait_until(
            "the restarted member learns the admissions it missed",
            DEFAULT_DEADLINE,
            |c| c.members("b", id) == everyone,
        )
        .await;
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.MEM-R10, TEAM.SYS.TRANS-R06
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn new_member_is_not_wedged_by_a_lagging_peer() {
    let mut cluster = SyncCluster::start("not-wedged", &["a", "b", "c"]).await;
    let workspace = cluster.node("a").create_workspace("Wedge").unwrap();
    let id = workspace.id();
    let a1 = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"a1")
        .unwrap();
    cluster.admit("a", "b", id).await;
    cluster
        .wait_until("the first joiner holds the record", DEFAULT_DEADLINE, |c| {
            c.record_ids("b", id, PACKAGE).contains(&a1.record_id())
        })
        .await;
    cluster.stop("b").await;

    // C joins while B is away, so B's view of the roster lags behind C's.
    cluster.admit("a", "c", id).await;
    cluster
        .wait_until(
            "the newcomer holds the content key",
            DEFAULT_DEADLINE,
            |c| c.record_ids("c", id, PACKAGE).contains(&a1.record_id()),
        )
        .await;
    cluster.stop("a").await;
    cluster.restart("b").await;

    let c1 = cluster
        .node("c")
        .append_record(id, PACKAGE, "message.post", b"c1")
        .unwrap();
    cluster
        .wait_until(
            "the lagging peer learns the newcomer",
            DEFAULT_DEADLINE,
            |c| c.members("b", id).contains(&c.identity("c")),
        )
        .await;
    cluster
        .wait_until(
            "the lagging peer takes the newcomer's records",
            DEFAULT_DEADLINE,
            |c| c.record_ids("b", id, PACKAGE).contains(&c1.record_id()),
        )
        .await;
    assert_eq!(
        cluster.node("b").parked_count_for_test(id, PACKAGE),
        0,
        "nothing stays parked once the admission has replicated"
    );
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.MEM-R06, TEAM.SYS.MEM-R10
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revocation_reaches_every_member() {
    let cluster = SyncCluster::start("revocation-reaches", &["a", "b", "c"]).await;
    let workspace = cluster.node("a").create_workspace("Revoked").unwrap();
    let id = workspace.id();
    cluster.admit("a", "b", id).await;
    cluster.admit("a", "c", id).await;
    let everyone = BTreeSet::from([
        cluster.identity("a"),
        cluster.identity("b"),
        cluster.identity("c"),
    ]);
    cluster
        .wait_until("the third member knows everyone", DEFAULT_DEADLINE, |c| {
            c.members("c", id) == everyone
        })
        .await;

    let mut transitions = cluster
        .node("c")
        .subscribe_membership(id, crate::MembershipEpoch::new(0))
        .unwrap();
    cluster
        .node("a")
        .revoke_member(id, cluster.identity("b"))
        .unwrap();

    let without_bob = BTreeSet::from([cluster.identity("a"), cluster.identity("c")]);
    cluster
        .wait_until(
            "the revocation reaches the third member",
            DEFAULT_DEADLINE,
            |c| c.members("c", id) == without_bob,
        )
        .await;

    let mut last = None;
    while let Ok(Some(transition)) = transitions.next_transition_timeout(Duration::ZERO) {
        last = Some(transition);
    }
    assert_eq!(
        last.expect("the third member records the revocation")
            .change(),
        crate::MembershipChange::Revoked {
            member: cluster.identity("b")
        }
    );
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.MEM-R10
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revoked_member_learns_only_its_revocation_closure() {
    let mut cluster = SyncCluster::start("closure-only", &["a", "b", "c", "d"]).await;
    let workspace = cluster.node("a").create_workspace("Closure").unwrap();
    let id = workspace.id();
    cluster.admit("a", "b", id).await;
    cluster.stop("b").await;

    let before_c = cluster.membership_operation_ids("a", id);
    cluster.admit("a", "c", id).await;
    let admit_c = cluster
        .membership_operation_ids("a", id)
        .difference(&before_c)
        .copied()
        .collect::<BTreeSet<_>>();
    let revocation = cluster
        .node("a")
        .revoke_member(id, cluster.identity("b"))
        .unwrap();
    let before_d = cluster.membership_operation_ids("a", id);
    cluster.admit("a", "d", id).await;
    let admit_d = cluster
        .membership_operation_ids("a", id)
        .difference(&before_d)
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(admit_c.len(), 1);
    assert_eq!(admit_d.len(), 1);
    cluster.restart("b").await;

    cluster
        .wait_until(
            "the revoked member learns its own revocation",
            DEFAULT_DEADLINE,
            |c| !c.members("b", id).contains(&c.identity("b")),
        )
        .await;

    let learned = cluster.membership_operation_ids("b", id);
    assert!(
        learned.contains(&revocation.cause()),
        "the revocation itself is disclosed"
    );
    assert!(
        learned.is_superset(&admit_c),
        "so is everything the revocation causally depends on"
    );
    assert!(
        learned.is_disjoint(&admit_d),
        "nothing authored after the revocation is disclosed"
    );

    // Ten further seconds of replication do not widen what it learned.
    let settled = cluster.members("b", id);
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(cluster.members("b", id), settled);
    assert_eq!(cluster.membership_operation_ids("b", id), learned);
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.MEM-R10
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stranger_receives_no_membership_operations() {
    let cluster = SyncCluster::start("stranger", &["a", "x"]).await;
    let workspace = cluster.node("a").create_workspace("Private").unwrap();
    let id = workspace.id();

    let (operations, refused) = cluster
        .node("x")
        .probe_membership_sync_for_test(cluster.address("a"), id, Vec::new())
        .await;

    assert!(operations.is_empty(), "a stranger is served nothing");
    assert!(refused, "a stranger is refused");
    assert!(
        cluster.members("x", id).is_empty(),
        "replication never creates a workspace"
    );
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn membership_replication_is_idempotent() {
    let cluster = SyncCluster::start("idempotent", &["a", "b", "c"]).await;
    let workspace = cluster.node("a").create_workspace("Idempotent").unwrap();
    let id = workspace.id();
    cluster.admit("a", "b", id).await;
    cluster.admit("a", "c", id).await;
    let everyone = BTreeSet::from([
        cluster.identity("a"),
        cluster.identity("b"),
        cluster.identity("c"),
    ]);
    cluster
        .wait_until("all three converge", DEFAULT_DEADLINE, |c| {
            c.members("b", id) == everyone && c.members("c", id) == everyone
        })
        .await;

    let epoch = cluster.node("b").membership_snapshot(id).unwrap().epoch();
    let mut transitions = cluster.node("b").subscribe_membership(id, epoch).unwrap();
    for _ in 0..5 {
        for name in ["a", "b", "c"] {
            cluster.node(name).sync_now_for_test();
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    assert_eq!(
        cluster.node("b").membership_snapshot(id).unwrap().epoch(),
        epoch,
        "re-running sessions applies nothing twice"
    );
    assert!(
        transitions
            .next_transition_timeout(Duration::from_secs(2))
            .unwrap()
            .is_none(),
        "no transition is published for an operation already held"
    );
    cluster.finish().await;
}

/// A peer the responder does not yet know as a Member may still push, because
/// canonical operations authenticate themselves.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_from_unknown_member_is_accepted_and_then_served() {
    let mut cluster = SyncCluster::start("push-unknown", &["a", "b", "c"]).await;
    let workspace = cluster.node("a").create_workspace("Push").unwrap();
    let id = workspace.id();
    cluster.admit("a", "b", id).await;
    cluster.stop("b").await;
    cluster.admit("a", "c", id).await;
    cluster.stop("a").await;
    cluster.restart("b").await;
    assert!(
        !cluster.members("b", id).contains(&cluster.identity("c")),
        "the restarted member has not heard of the newcomer yet"
    );

    let report = cluster
        .node("c")
        .membership_session_for_test(cluster.address("b"), id, cluster.identity("b"))
        .await
        .expect("the push session completes");

    assert!(report.sent >= 1, "the newcomer pushed its own admission");
    assert!(!report.refused, "a pushing member is served in return");
    assert!(cluster.members("b", id).contains(&cluster.identity("c")));
    cluster.finish().await;
}
