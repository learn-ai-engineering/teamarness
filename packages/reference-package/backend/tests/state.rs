use std::time::Duration;

use teamarness_reference_package::{ReferenceState, StateSnapshot};

#[tokio::test]
async fn counter_and_background_marker_survive_reopen() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("state.sqlite3");

    let state = ReferenceState::open(&database).unwrap();
    assert_eq!(
        state.snapshot().unwrap(),
        StateSnapshot {
            counter: 0,
            background_marker: 0
        }
    );
    assert_eq!(state.increment_counter().unwrap().counter, 1);
    assert_eq!(
        state.advance_background_marker().unwrap().background_marker,
        1
    );
    drop(state);

    let reopened = ReferenceState::open(&database).unwrap();
    assert_eq!(
        reopened.snapshot().unwrap(),
        StateSnapshot {
            counter: 1,
            background_marker: 1
        }
    );
}

#[tokio::test]
async fn active_marker_advances_and_suspension_stops_it() {
    let root = tempfile::tempdir().unwrap();
    let state = ReferenceState::open(root.path().join("state.sqlite3")).unwrap();
    let activity = state.start_background_activity(Duration::from_millis(10));

    let expiry = std::time::Instant::now() + Duration::from_secs(5);
    while state.snapshot().unwrap().background_marker < 2 {
        assert!(
            std::time::Instant::now() < expiry,
            "timed out waiting until the background activity advanced its marker"
        );
        // poll-interval: the interval of this loop, which is bounded by its own deadline.
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    activity.stop().await;
    let stopped = state.snapshot().unwrap().background_marker;
    assert!(stopped >= 2);
    // negative-wait: a bounded window proving a stopped activity's marker stays put.
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(state.snapshot().unwrap().background_marker, stopped);
}
