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

/// Verifies: TEAM.SYS.TRANS-R06
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grant_with_a_member_minted_origin_is_rejected() {
    let cluster = SyncCluster::start("minted-origin", &["a", "b", "c"]).await;
    let workspace = cluster.node("a").create_workspace("Origin").unwrap();
    let id = workspace.id();
    let a1 = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"a1")
        .unwrap();
    cluster.admit("a", "b", id).await;
    cluster.admit("a", "c", id).await;

    // B invents its own key and signs both halves of a bundle for C. Every
    // signature is genuine; only the creator-origin rule refuses it.
    let b_secret = cluster.node("b").state.lock().unwrap().secret_key.clone();
    let (forged_origin, forged_grant) = crate::synchronization::forge_protection_bundle_for_test(
        &b_secret,
        id,
        PACKAGE,
        cluster.identity("c"),
    );
    assert_eq!(
        cluster
            .node("c")
            .import_protection_grant(id, PACKAGE, &forged_origin, &forged_grant)
            .unwrap_err(),
        crate::SynchronizationError::KeyOriginRejected
    );

    let (origin, grant) = cluster
        .node("a")
        .export_protection_bundle_for_test(id, PACKAGE, cluster.identity("c"))
        .unwrap();
    cluster
        .node("c")
        .import_protection_grant(id, PACKAGE, &origin, &grant)
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
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn non_creator_holder_welcomes_a_new_member() {
    let mut cluster = SyncCluster::start("non-creator-holder", &["a", "b", "c"]).await;
    let workspace = cluster.node("a").create_workspace("Holder").unwrap();
    let id = workspace.id();
    let a1 = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"a1")
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
    cluster.admit("a", "c", id).await;
    cluster.replicate_membership("a", "b", id).await;
    cluster.stop("a").await;

    cluster
        .node("c")
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
            .record_ids("c", id, PACKAGE)
            .contains(&a1.record_id())
    );
    let c1 = cluster
        .node("c")
        .append_record(id, PACKAGE, "message.post", b"c1")
        .unwrap();
    cluster
        .wait_until(
            "the holder receives the newcomer's record",
            DEFAULT_DEADLINE,
            |c| c.record_ids("b", id, PACKAGE).contains(&c1.record_id()),
        )
        .await;
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn member_without_key_gets_content_key_unavailable() {
    let mut cluster = SyncCluster::start("no-key-yet", &["a", "b"]).await;
    let workspace = cluster.node("a").create_workspace("No key").unwrap();
    let id = workspace.id();
    cluster.admit("a", "b", id).await;
    cluster.stop("a").await;

    assert_eq!(
        cluster
            .node("b")
            .append_record(id, PACKAGE, "message.post", b"b1")
            .unwrap_err(),
        crate::SynchronizationError::ContentKeyUnavailable
    );
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R06, TEAM.SYS.MEM-R10
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn records_from_an_unknown_author_are_applied_after_membership_replicates() {
    let mut cluster = SyncCluster::start("parked-author", &["a", "b", "c"]).await;
    let workspace = cluster.node("a").create_workspace("Parking").unwrap();
    let id = workspace.id();
    let a1 = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"a1")
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
    cluster.stop("b").await;

    // C is admitted while B is away, so B has never heard of C.
    cluster.admit("a", "c", id).await;
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
    let c1 = cluster
        .node("c")
        .append_record(id, PACKAGE, "message.post", b"c1")
        .unwrap();
    let c2 = cluster
        .node("c")
        .append_record(id, PACKAGE, "message.post", b"c2")
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
    cluster.stop("c").await;
    cluster.restart("b").await;
    // Hold B's membership replication so the forwarded records arrive before
    // the admission that makes them applicable.
    cluster
        .node("b")
        .pause_membership_replication_for_test(true);

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
    assert_eq!(
        cluster.node("b").parked_count_for_test(id, PACKAGE),
        2,
        "both of the unknown author's records wait instead of aborting the session"
    );
    let seen = cluster.record_ids("b", id, PACKAGE);
    assert!(
        seen.contains(&a1.record_id()),
        "the known author still applies"
    );
    assert!(!seen.contains(&c1.record_id()));

    // Replication delivers the admission, which promotes what was parked.
    cluster
        .node("b")
        .pause_membership_replication_for_test(false);
    cluster
        .wait_until(
            "the parked records are applied after membership replicates",
            DEFAULT_DEADLINE,
            |c| {
                let seen = c.record_ids("b", id, PACKAGE);
                seen.contains(&c1.record_id()) && seen.contains(&c2.record_id())
            },
        )
        .await;
    assert!(cluster.members("b", id).contains(&cluster.identity("c")));
    assert_eq!(cluster.node("b").parked_count_for_test(id, PACKAGE), 0);
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R06
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forged_scope_record_is_rejected_without_ending_the_session() {
    const OTHER: &str = "teamarness.other";
    let cluster = SyncCluster::start("forged-scope", &["a", "m", "b"]).await;
    let workspace = cluster.node("a").create_workspace("Forgery").unwrap();
    let id = workspace.id();
    cluster
        .node("a")
        .append_record(id, OTHER, "message.post", b"a-other")
        .unwrap();
    let a1 = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"a1")
        .unwrap();
    cluster.admit("a", "m", id).await;
    cluster.admit("a", "b", id).await;
    // Each joiner learned the roster as it stood when it joined; one membership
    // session each gives both the manager's current view, so either may dial
    // the other.
    cluster.replicate_membership("a", "m", id).await;
    cluster.replicate_membership("a", "b", id).await;
    for package in [OTHER, PACKAGE] {
        cluster
            .node("m")
            .replicate_records_to_address_for_test(
                id,
                package,
                cluster.address("a"),
                cluster.identity("a"),
            )
            .await
            .unwrap();
    }
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

    // M writes to one scope and re-seals that record into another it also holds.
    let m_other = cluster
        .node("m")
        .append_record(id, OTHER, "message.post", b"m-other")
        .unwrap();
    let forged = {
        let state = cluster.node("m").state.lock().unwrap();
        crate::synchronization::forge_cross_scope_operation_for_test(
            &state.connection,
            &state.secret_key,
            id,
            OTHER,
            PACKAGE,
            m_other.record_id(),
        )
    };

    let applied = cluster
        .node("m")
        .deliver_operations_for_test(
            cluster.address("b"),
            id,
            PACKAGE,
            cluster.identity("b"),
            vec![forged],
        )
        .await
        .expect("one forged record must not end the session");
    assert_eq!(applied, 0, "the forging peer had nothing new to receive");

    let seen = cluster.record_ids("b", id, PACKAGE);
    assert!(
        seen.contains(&a1.record_id()),
        "the genuine record in the same session still applies"
    );
    assert!(
        !seen.contains(&m_other.record_id()),
        "a record signed for another package never opens in this scope"
    );
    assert_eq!(cluster.node("b").parked_count_for_test(id, PACKAGE), 0);
    cluster.finish().await;
}
