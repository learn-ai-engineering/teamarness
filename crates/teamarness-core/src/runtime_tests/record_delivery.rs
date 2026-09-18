//! Record delivery re-reads the durable journal instead of queueing events.
//!
//! Real components: real [`CoreRuntime`](super::super::CoreRuntime) instances on
//! temporary directories, real SQLite, and — where a peer is involved — real
//! Iroh endpoints over loopback. Nothing that is under test is mocked.

use std::collections::BTreeSet;
use std::time::Duration;

use super::cluster::{DEFAULT_DEADLINE, SyncCluster};
use crate::synchronization::record_subscription::records_decoded_for_test;
use crate::synchronization::scope_digest;
use crate::{Cursor, SynchronizationError};

const PACKAGE: &str = "teamarness.test";
/// Only bounds failure: a wake that works arrives in microseconds.
const WAKE_DEADLINE: Duration = Duration::from_secs(5);

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test]
async fn subscribe_loads_no_backlog() {
    let cluster = SyncCluster::start("subscribe-no-backlog", &["a"]).await;
    let workspace = cluster.node("a").create_workspace("Backlog").unwrap();
    let id = workspace.id();
    let scope = scope_digest(PACKAGE, id);
    for index in 0..500_u32 {
        cluster
            .node("a")
            .append_record(id, PACKAGE, "message.post", &index.to_be_bytes())
            .unwrap();
    }

    let mut subscription = cluster
        .node("a")
        .subscribe_records(id, PACKAGE, None)
        .unwrap();
    assert_eq!(
        records_decoded_for_test(scope),
        0,
        "subscribing decodes nothing"
    );

    let page = subscription.next_page(32).unwrap();
    assert_eq!(page.events.len(), 32);
    assert_eq!(page.quarantined_skipped, 0);
    assert_eq!(
        records_decoded_for_test(scope),
        32,
        "one page decodes exactly one page"
    );
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test]
async fn idle_subscriber_holds_no_buffered_events() {
    let cluster = SyncCluster::start("idle-subscriber", &["a"]).await;
    let workspace = cluster.node("a").create_workspace("Idle").unwrap();
    let id = workspace.id();
    let scope = scope_digest(PACKAGE, id);

    let mut subscription = cluster
        .node("a")
        .subscribe_records(id, PACKAGE, None)
        .unwrap();
    let mut appended = Vec::new();
    for index in 0..2_000_u32 {
        appended.push(
            cluster
                .node("a")
                .append_record(id, PACKAGE, "message.post", &index.to_be_bytes())
                .unwrap()
                .record_id(),
        );
    }
    assert_eq!(
        records_decoded_for_test(scope),
        0,
        "an open subscription that reads nothing costs no decryption"
    );

    let mut delivered = Vec::new();
    loop {
        let page = subscription.next_page(32).unwrap();
        assert!(page.events.len() <= 32);
        if page.events.is_empty() {
            break;
        }
        delivered.extend(page.events.iter().map(crate::RecordEvent::record_id));
    }
    assert_eq!(delivered, appended, "every record, once, in delivery order");
    assert_eq!(delivered.iter().collect::<BTreeSet<_>>().len(), 2_000);
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test]
async fn subscribe_rejects_a_cursor_ahead_of_the_journal() {
    let cluster = SyncCluster::start("cursor-ahead", &["a"]).await;
    let workspace = cluster.node("a").create_workspace("Cursors").unwrap();
    let id = workspace.id();
    for index in 0..3_u8 {
        cluster
            .node("a")
            .append_record(id, PACKAGE, "message.post", &[index; 4])
            .unwrap();
    }
    let mut subscription = cluster
        .node("a")
        .subscribe_records(id, PACKAGE, None)
        .unwrap();
    let page = subscription.next_page(32).unwrap();
    let third = page.events.last().expect("three records were committed");
    let journal: [u8; 16] = third.cursor().as_bytes()[1..17]
        .try_into()
        .expect("cursor width");

    assert!(matches!(
        cluster
            .node("a")
            .subscribe_records(id, PACKAGE, Some(Cursor::new(journal, 4))),
        Err(SynchronizationError::CursorAhead)
    ));
    assert!(
        cluster
            .node("a")
            .subscribe_records(id, PACKAGE, Some(Cursor::new(journal, 3)))
            .is_ok(),
        "the journal head itself is a valid cursor"
    );
    assert!(
        matches!(
            cluster.node("a").subscribe_records(
                id,
                "teamarness.absent",
                Some(Cursor::new(journal, 1))
            ),
            Err(SynchronizationError::CursorAhead)
        ),
        "a cursor for a scope this Installation does not hold is ahead of it"
    );
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test]
async fn subscribe_rejects_a_cursor_from_another_journal() {
    let cluster = SyncCluster::start("cursor-journal", &["a"]).await;
    let workspace = cluster.node("a").create_workspace("Journals").unwrap();
    let id = workspace.id();
    cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"here")
        .unwrap();
    cluster
        .node("a")
        .append_record(id, "teamarness.other", "message.post", b"there")
        .unwrap();

    let mut other = cluster
        .node("a")
        .subscribe_records(id, "teamarness.other", None)
        .unwrap();
    let elsewhere = other
        .next_page(1)
        .unwrap()
        .events
        .first()
        .expect("the other scope committed one record")
        .cursor();

    assert!(matches!(
        cluster
            .node("a")
            .subscribe_records(id, PACKAGE, Some(elsewhere)),
        Err(SynchronizationError::CursorScopeMismatch)
    ));
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test]
async fn subscribe_before_the_scope_exists_waits_for_it() {
    let cluster = SyncCluster::start("subscribe-before-scope", &["a"]).await;
    let workspace = cluster.node("a").create_workspace("Waiting").unwrap();
    let id = workspace.id();

    let mut subscription = cluster
        .node("a")
        .subscribe_records(id, PACKAGE, None)
        .expect("a subscription opens before the scope has a content key");
    assert!(subscription.next_page(32).unwrap().events.is_empty());

    let waiter = tokio::spawn(async move {
        subscription.changed().await.expect("the runtime is alive");
        subscription
    });
    let appended = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"first")
        .unwrap();
    let mut subscription = tokio::time::timeout(WAKE_DEADLINE, waiter)
        .await
        .expect("the first append wakes the waiter")
        .expect("the waiting task did not panic");
    let page = subscription.next_page(32).unwrap();
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].record_id(), appended.record_id());
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test]
async fn quarantined_record_is_skipped_and_position_advances() {
    let cluster = SyncCluster::start("quarantined-skip", &["a"]).await;
    let workspace = cluster.node("a").create_workspace("Quarantine").unwrap();
    let id = workspace.id();
    let mut appended = Vec::new();
    for index in 0..3_u8 {
        appended.push(
            cluster
                .node("a")
                .append_record(id, PACKAGE, "message.post", &[index; 4])
                .unwrap()
                .record_id(),
        );
    }
    // The last record is isolated, so the page that reads it must still
    // advance past it rather than offering the same gap again.
    cluster
        .node("a")
        .quarantine_record_for_test(id, PACKAGE, appended[2]);

    let mut subscription = cluster
        .node("a")
        .subscribe_records(id, PACKAGE, None)
        .unwrap();
    let page = subscription.next_page(32).unwrap();
    assert_eq!(
        page.events
            .iter()
            .map(crate::RecordEvent::record_id)
            .collect::<Vec<_>>(),
        appended[..2].to_vec()
    );
    assert_eq!(page.quarantined_skipped, 1);
    let next = subscription.next_page(32).unwrap();
    assert!(
        next.events.is_empty() && next.quarantined_skipped == 0,
        "the skipped record's sequence is consumed, so the subscription is idle"
    );
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test]
async fn commits_from_every_path_wake_a_waiting_subscriber() {
    let mut cluster = SyncCluster::start("wake-every-path", &["a", "b", "c"]).await;
    let workspace = cluster.node("a").create_workspace("Wakes").unwrap();
    let id = workspace.id();
    cluster
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

    // (a) A local append wakes a waiter on the same Installation.
    let mut local = cluster
        .node("a")
        .subscribe_records(id, PACKAGE, None)
        .unwrap();
    drain(&mut local);
    let waiter = tokio::spawn(async move {
        local.changed().await.expect("the runtime is alive");
    });
    cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"a2")
        .unwrap();
    tokio::time::timeout(WAKE_DEADLINE, waiter)
        .await
        .expect("a local append wakes a waiting subscriber")
        .expect("the waiting task did not panic");

    // (b) A record applied from a peer wakes a waiter on the receiver.
    let mut remote = cluster
        .node("b")
        .subscribe_records(id, PACKAGE, None)
        .unwrap();
    drain(&mut remote);
    let waiter = tokio::spawn(async move {
        remote.changed().await.expect("the runtime is alive");
    });
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
    tokio::time::timeout(WAKE_DEADLINE, waiter)
        .await
        .expect("an applied remote record wakes a waiting subscriber")
        .expect("the waiting task did not panic");

    // (c) A parked operation promoted after an admission wakes a waiter too.
    cluster.node("b").pause_sync_scheduler_for_test(true);
    cluster.stop("b").await;
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
    let parked_record = cluster
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
    cluster.stop("c").await;
    cluster.restart("b").await;
    cluster.node("b").pause_sync_scheduler_for_test(true);
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
        1,
        "the unknown author's record waits for its admission"
    );
    let mut promoted = cluster
        .node("b")
        .subscribe_records(id, PACKAGE, None)
        .unwrap();
    drain(&mut promoted);
    let waiter = tokio::spawn(async move {
        promoted.changed().await.expect("the runtime is alive");
        promoted
    });
    cluster.node("b").pause_sync_scheduler_for_test(false);
    cluster
        .node("b")
        .pause_membership_replication_for_test(false);
    let mut promoted = tokio::time::timeout(DEFAULT_DEADLINE, waiter)
        .await
        .expect("a promoted parked record wakes a waiting subscriber")
        .expect("the waiting task did not panic");
    let mut seen = BTreeSet::new();
    while let Ok(Some(event)) = promoted.next_event_timeout(Duration::from_secs(1)) {
        seen.insert(event.record_id());
        if seen.contains(&parked_record.record_id()) {
            break;
        }
    }
    assert!(
        seen.contains(&parked_record.record_id()),
        "the promoted record is delivered by re-reading the journal"
    );
    cluster.finish().await;
}

/// Reads everything already committed, so a waiter starts at the head.
fn drain(subscription: &mut crate::RecordSubscription) {
    while !subscription.next_page(32).unwrap().events.is_empty() {}
}
