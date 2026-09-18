//! Package synchronization is gated on current membership.
//!
//! Real components: real `CoreRuntime`s on temporary directories, real Iroh
//! endpoints in loopback mode with the in-memory address book, real SQLite.

use std::collections::BTreeSet;
use std::time::Duration;

use super::super::ProbeOutcome;
use super::cluster::{DEFAULT_DEADLINE, SyncCluster};
use crate::synchronization::{LogSessionError, Refusal};

const PACKAGE: &str = "teamarness.test";

/// Verifies: TEAM.SYS.TRANS-R06
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn non_member_is_refused_on_package_sync() {
    let cluster = SyncCluster::start("non-member-refused", &["a", "x"]).await;
    let workspace = cluster.node("a").create_workspace("Gate").unwrap();
    let seed = cluster
        .node("a")
        .append_record(workspace.id(), PACKAGE, "message.post", b"seed")
        .unwrap();

    let outcome = cluster
        .node("x")
        .probe_package_sync_for_test(
            cluster.address("a"),
            workspace.id(),
            PACKAGE,
            cluster.identity("a"),
        )
        .await;

    assert_eq!(outcome, ProbeOutcome::Refused(Refusal::NotAuthorized));
    assert_eq!(
        cluster.record_ids("a", workspace.id(), PACKAGE),
        BTreeSet::from([seed.record_id()]),
        "a refused session must not change the serving installation's scope"
    );
    assert!(
        cluster.record_ids("x", workspace.id(), PACKAGE).is_empty(),
        "a non-member must receive nothing"
    );
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn batch_alpn_is_no_longer_offered() {
    let cluster = SyncCluster::start("batch-alpn-gone", &["a", "x"]).await;
    assert!(
        !cluster
            .node("x")
            .can_connect_alpn_for_test(cluster.address("a"), b"teamarness/package-sync-batch/1")
            .await,
        "the retired batch ingest ALPN must not be routed"
    );
    assert!(
        cluster
            .node("x")
            .can_connect_alpn_for_test(cluster.address("a"), b"teamarness/package-sync/2")
            .await,
        "the gated package sync ALPN is still offered"
    );
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn member_receives_content_key_in_the_handshake_from_either_side() {
    let cluster = SyncCluster::start("handshake-key", &["a", "b", "c"]).await;
    let workspace = cluster.node("a").create_workspace("Handshake").unwrap();
    let id = workspace.id();
    let a1 = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"a1")
        .unwrap();

    // The joiner dials: the key arrives in the responder's Welcome.
    cluster.admit("a", "b", id).await;
    cluster
        .node("b")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("a"),
            cluster.identity("a"),
        )
        .await
        .unwrap();
    let b1 = cluster
        .node("b")
        .append_record(id, PACKAGE, "message.post", b"b1")
        .unwrap();
    cluster
        .node("a")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("b"),
            cluster.identity("b"),
        )
        .await
        .unwrap();
    assert!(
        cluster
            .record_ids("b", id, PACKAGE)
            .contains(&a1.record_id())
    );
    assert!(
        cluster
            .record_ids("a", id, PACKAGE)
            .contains(&b1.record_id())
    );

    // The holder dials: the key arrives in the initiator's Ready.
    cluster.admit("a", "c", id).await;
    cluster
        .node("a")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("c"),
            cluster.identity("c"),
        )
        .await
        .unwrap();
    let c1 = cluster
        .node("c")
        .append_record(id, PACKAGE, "message.post", b"c1")
        .unwrap();
    cluster
        .node("c")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("a"),
            cluster.identity("a"),
        )
        .await
        .unwrap();
    assert!(
        cluster
            .record_ids("c", id, PACKAGE)
            .contains(&a1.record_id())
    );
    assert!(
        cluster
            .record_ids("a", id, PACKAGE)
            .contains(&c1.record_id())
    );
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R06
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_revocation_cancels_a_live_session() {
    let cluster = SyncCluster::start("revocation-cancels", &["a", "b"]).await;
    let workspace = cluster.node("a").create_workspace("Cancel").unwrap();
    let id = workspace.id();
    cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"seed")
        .unwrap();
    cluster.admit("a", "b", id).await;

    // The responder stalls inside its own gate, so the session is live but
    // cannot finish while the revocation is committed.
    let hold = cluster.node("b").hold_runtime_state_for_test();
    let result = {
        let session = cluster.node("a").replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("b"),
            cluster.identity("b"),
        );
        tokio::pin!(session);
        let settled = tokio::select! {
            result = &mut session => Some(result),
            () = tokio::time::sleep(Duration::from_millis(300)) => None,
        };
        assert!(settled.is_none(), "the session must still be running");

        cluster
            .node("a")
            .revoke_member(id, cluster.identity("b"))
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), session)
            .await
            .expect("a revocation cancels the live session within two seconds")
    };
    assert_eq!(result.unwrap_err(), LogSessionError::Cancelled);
    hold.release();
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_revoked_member_is_refused_by_the_gate() {
    let cluster = SyncCluster::start("revoked-refused", &["a", "b"]).await;
    let workspace = cluster.node("a").create_workspace("Revoked").unwrap();
    let id = workspace.id();
    cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"seed")
        .unwrap();
    cluster.admit("a", "b", id).await;
    cluster
        .node("b")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("a"),
            cluster.identity("a"),
        )
        .await
        .unwrap();

    cluster
        .node("a")
        .revoke_member(id, cluster.identity("b"))
        .unwrap();
    let after = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"after-revocation")
        .unwrap();

    // B has not yet learned of its own revocation, so its local gate still
    // lets it dial. A's gate is the one that must refuse.
    let refused = cluster
        .node("b")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("a"),
            cluster.identity("a"),
        )
        .await;
    assert_eq!(
        refused.unwrap_err(),
        LogSessionError::Refused(Refusal::NotAuthorized)
    );
    assert!(
        !cluster
            .record_ids("b", id, PACKAGE)
            .contains(&after.record_id()),
        "a revoked member must not receive records written after its revocation"
    );
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R06
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_worker_converges_two_members_and_catches_up_after_a_restart() {
    let mut cluster = SyncCluster::start("worker-converges", &["a", "b"]).await;
    let workspace = cluster.node("a").create_workspace("Worker").unwrap();
    let id = workspace.id();
    let a1 = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"a1")
        .unwrap();
    cluster.admit("a", "b", id).await;
    assert_eq!(
        cluster.members("a", id),
        cluster.members("b", id),
        "the bootstrap import gives the joiner the manager's roster"
    );

    cluster
        .wait_until("the joiner has the first record", DEFAULT_DEADLINE, |c| {
            c.record_ids("b", id, PACKAGE).contains(&a1.record_id())
        })
        .await;

    cluster.stop("b").await;
    let a2 = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"a2")
        .unwrap();
    cluster.restart("b").await;
    cluster
        .wait_until("the restarted member catches up", DEFAULT_DEADLINE, |c| {
            c.record_ids("b", id, PACKAGE).contains(&a2.record_id())
        })
        .await;
    cluster.finish().await;
}
