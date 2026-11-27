use std::time::{Duration, Instant};

use teamarness_core::{CoreRuntime, CoreRuntimeConfig, MembershipEpoch};

#[tokio::test]
async fn bounded_membership_subscription_releases_an_idle_receiver_after_timeout() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("membership-bounds");
    let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&root))
        .await
        .unwrap();
    let workspace = runtime.create_workspace("Bounded receiver").unwrap();
    let mut subscription = runtime
        .subscribe_membership_bounded(workspace.id(), MembershipEpoch::new(0), 1)
        .unwrap();

    let started = Instant::now();
    assert_eq!(
        subscription
            .next_transition_timeout(Duration::from_millis(20))
            .unwrap(),
        None
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    runtime.shutdown().await.unwrap();
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}
