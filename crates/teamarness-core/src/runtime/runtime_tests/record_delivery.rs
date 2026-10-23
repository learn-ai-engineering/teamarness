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

/// Verifies: TEAM.SYS.TRANS-R06, TEAM.SYS.TRANS-R08 (creator origin and subscription bootstrap).
#[tokio::test]
async fn creator_subscription_bootstraps_scope_and_signed_origin_without_records() {
    use crate::synchronization::protection_control::{self, KIND_CREATE};

    let cluster = SyncCluster::start("creator-subscription", &["a"]).await;
    let workspace = cluster.node("a").create_workspace("Empty scope").unwrap();
    let id = workspace.id();
    let scope = scope_digest(PACKAGE, id);
    let inspect = || {
        let state = cluster.node("a").state.lock().unwrap();
        let connection = &state.connection;
        let scopes: Vec<(Vec<u8>, Vec<u8>, Vec<u8>)> = {
            let mut statement = connection.prepare(
                "SELECT scope_id, journal_id, content_key_id FROM sync_scopes WHERE scope_id=?1",
            ).unwrap();
            statement
                .query_map([scope.as_slice()], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        let controls: Vec<Vec<u8>> = {
            let mut statement = connection
                .prepare(
                    "SELECT control_bytes FROM protection_control_operations WHERE scope_id=?1",
                )
                .unwrap();
            statement
                .query_map([scope.as_slice()], |row| row.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        let count = |table: &str| -> i64 {
            connection
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE scope_id=?1"),
                    [scope.as_slice()],
                    |row| row.get(0),
                )
                .unwrap()
        };
        assert_eq!(count("immutable_records"), count("immutable_operations"));
        assert_eq!(count("protection_state"), scopes.len() as i64);
        if let Some((_, _, key_id)) = scopes.first() {
            let stored: (Vec<u8>, Vec<u8>) = connection.query_row(
                "SELECT content_key_id, creator_public_identity FROM protection_state WHERE scope_id=?1",
                [scope.as_slice()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).unwrap();
            assert_eq!(&stored.0, key_id);
            assert_eq!(stored.1.as_slice(), cluster.identity("a").as_bytes());
        }
        (
            scopes,
            controls,
            count("immutable_operations"),
            count("record_delivery"),
            count("durable_wakes"),
        )
    };

    let mut subscription = cluster
        .node("a")
        .subscribe_records(id, PACKAGE, None)
        .unwrap();
    assert!(subscription.next_page(1).unwrap().events.is_empty());
    let (scopes, controls, operations, deliveries, wakes) = inspect();
    assert_eq!(scopes.len(), 1, "the creator owns a real scope");
    assert_eq!(scopes[0].0, scope);
    assert_eq!(scopes[0].1.len(), 16);
    assert_eq!(scopes[0].2.len(), 32);
    assert_eq!(controls.len(), 1, "the first key has exactly one origin");
    let origin = protection_control::decode(&controls[0]).expect("signed origin verifies");
    assert_eq!(origin.kind, KIND_CREATE);
    assert_eq!(origin.scope, scope);
    assert_eq!(origin.workspace, id);
    assert_eq!(origin.author, cluster.identity("a"));
    assert_eq!(origin.recipient, cluster.identity("a"));
    assert_eq!(origin.content_key_id.as_slice(), scopes[0].2);
    assert_eq!((operations, deliveries, wakes), (0, 0, 1));

    cluster
        .node("a")
        .subscribe_records(id, PACKAGE, None)
        .unwrap();
    assert_eq!(
        inspect(),
        (scopes.clone(), controls.clone(), 0, 0, 1),
        "reopening must not mint another key or origin"
    );

    cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"first")
        .unwrap();
    let (after_scopes, after_controls, operations, deliveries, wakes) = inspect();
    assert_eq!(
        after_scopes, scopes,
        "append reuses the declared key and journal"
    );
    assert_eq!(after_controls, controls, "append reuses the creator origin");
    assert_eq!((operations, deliveries, wakes), (1, 1, 1));
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R07: an empty scope has a finite replay boundary.
#[tokio::test]
async fn empty_creator_scope_qualifies_initial_replay_once() {
    let cluster = SyncCluster::start("empty-replay-boundary", &["a"]).await;
    let id = cluster
        .node("a")
        .create_workspace("Empty replay")
        .unwrap()
        .id();
    let mut subscription = cluster
        .node("a")
        .subscribe_records(id, PACKAGE, None)
        .unwrap();
    let first = subscription.next_page(1).unwrap();
    let boundary = first.initial_replay.expect("empty scope has a boundary");
    assert!(boundary.gap_free);
    assert_eq!(boundary.cursor, subscription.cursor());
    assert!(subscription.next_page(1).unwrap().initial_replay.is_none());
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R07: new appends cannot postpone the initial prefix.
#[tokio::test]
async fn initial_replay_stops_at_subscription_high_water_despite_new_appends() {
    let cluster = SyncCluster::start("fixed-replay-boundary", &["a"]).await;
    let id = cluster
        .node("a")
        .create_workspace("Fixed replay")
        .unwrap()
        .id();
    for index in 0..3_u8 {
        cluster
            .node("a")
            .append_record(id, PACKAGE, "message.post", &[index])
            .unwrap();
    }
    let mut subscription = cluster
        .node("a")
        .subscribe_records(id, PACKAGE, None)
        .unwrap();
    cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"later")
        .unwrap();
    assert!(subscription.next_page(2).unwrap().initial_replay.is_none());
    let boundary_page = subscription.next_page(32).unwrap();
    assert_eq!(boundary_page.events.len(), 1);
    let boundary = boundary_page
        .initial_replay
        .expect("fixed high-water reached");
    assert!(boundary.gap_free);
    assert_eq!(boundary.cursor, Some(boundary_page.events[0].cursor()));
    let live = subscription.next_page(32).unwrap();
    assert_eq!(live.events.len(), 1);
    assert!(live.initial_replay.is_none());
    cluster.finish().await;
}

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
        if page.initial_replay.is_some() {
            assert!(
                page.events.is_empty(),
                "the fixed prefix was empty at subscribe"
            );
            continue;
        }
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
    let cluster = SyncCluster::start("subscribe-before-scope", &["a", "b"]).await;
    cluster.node("a").pause_sync_scheduler_for_test(true);
    cluster.node("b").pause_sync_scheduler_for_test(true);
    let workspace = cluster.node("a").create_workspace("Waiting").unwrap();
    let id = workspace.id();
    cluster.admit("a", "b", id).await;

    let mut subscription = cluster
        .node("b")
        .subscribe_records(id, PACKAGE, None)
        .expect("a Member subscription opens before it has a content key");
    let waiting = subscription.next_page(32).unwrap();
    assert!(waiting.events.is_empty());
    assert!(
        waiting.initial_replay.is_none(),
        "unavailable history has no absence proof"
    );

    let waiter = tokio::spawn(async move {
        subscription.changed().await.expect("the runtime is alive");
        subscription
    });
    let appended = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"first")
        .unwrap();
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
    let mut subscription = tokio::time::timeout(WAKE_DEADLINE, waiter)
        .await
        .expect("the imported key wakes the Member waiter")
        .expect("the waiting task did not panic");
    let boundary_page = subscription.next_page(32).unwrap();
    assert!(
        boundary_page
            .initial_replay
            .is_some_and(|boundary| !boundary.gap_free)
    );
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
    assert!(
        page.initial_replay
            .is_some_and(|boundary| !boundary.gap_free)
    );
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
    // Every session in parts (a) and (b) is driven explicitly, so the schedulers
    // must not run one of their own with the same peer at the same time: the
    // loser of that race is cancelled, and it would be this test's session.
    // Part (c) resumes b's scheduler, which is what triggers the promotion it
    // waits for.
    for name in ["a", "b", "c"] {
        cluster.node(name).pause_sync_scheduler_for_test(true);
    }
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
    let (health_observed, health_wake) = tokio::sync::oneshot::channel();
    let waiter = tokio::spawn(async move {
        // A health wake is not a delivery. Observe one while admission is
        // paused, so this ordering is covered regardless of scheduler timing.
        promoted.changed().await.expect("the runtime is alive");
        assert!(promoted.next_page(32).unwrap().events.is_empty());
        health_observed.send(()).expect("the test is still waiting");
        loop {
            // Never block this single-threaded executor on a condvar: the
            // membership session that promotes the record must run here too.
            promoted.changed().await.expect("the runtime is alive");
            let page = promoted.next_page(32).unwrap();
            if page
                .events
                .iter()
                .any(|event| event.record_id() == parked_record.record_id())
            {
                return;
            }
        }
    });
    cluster
        .node("b")
        .health
        .scope_changed(scope_digest(PACKAGE, id));
    tokio::time::timeout(WAKE_DEADLINE, health_wake)
        .await
        .expect("the subscriber observes health without a delivery")
        .expect("the waiting task did not panic");
    cluster.node("b").pause_sync_scheduler_for_test(false);
    cluster
        .node("b")
        .pause_membership_replication_for_test(false);
    tokio::time::timeout(DEFAULT_DEADLINE, waiter)
        .await
        .expect("the promoted record is delivered by re-reading the journal")
        .expect("the waiting task did not panic");
    cluster.finish().await;
}

/// Reads everything already committed, so a waiter starts at the head.
fn drain(subscription: &mut crate::RecordSubscription) {
    while !subscription.next_page(32).unwrap().events.is_empty() {}
}
