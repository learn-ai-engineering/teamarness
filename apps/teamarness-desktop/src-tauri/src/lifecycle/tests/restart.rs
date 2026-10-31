use super::*;

/// Verifies: TEAM.SYS.SHELL-R01, TEAM.SYS.SHELL-R07
#[test]
fn ordinary_restart_from_native_thread_drains_once_without_installing() {
    let lifecycle = tauri::async_runtime::block_on(async { Lifecycle::new() });
    assert!(tokio::runtime::Handle::try_current().is_err());
    let adapter = Arc::new(Fixture::default());
    let lease = lifecycle.admit().unwrap();
    lifecycle.restart(adapter.clone()).unwrap();
    lifecycle.restart(adapter.clone()).unwrap();
    lifecycle.quit(adapter.clone());
    assert!(lifecycle.admit().is_err());
    assert!(
        lifecycle
            .apply(
                "update",
                || panic!("restart is not update consent"),
                adapter.clone()
            )
            .is_err()
    );
    assert!(adapter.order.lock().unwrap().is_empty());
    drop(lease);
    tauri::async_runtime::block_on(async {
        tokio::time::timeout(Duration::from_secs(10), lifecycle.finished())
            .await
            .unwrap();
    });
    assert_eq!(
        *adapter.order.lock().unwrap(),
        ["mounts", "host", "core", "restart"]
    );
    assert_eq!(adapter.installs.load(Ordering::SeqCst), 0);
    assert_eq!(adapter.exits.load(Ordering::SeqCst), 0);
    assert_eq!(lifecycle.status().phase, Phase::Restarting);
}

/// Verifies: TEAM.SYS.SHELL-R07
#[tokio::test]
async fn ordinary_restart_cannot_override_quit_or_apply() {
    for apply in [false, true] {
        let lifecycle = Lifecycle::new();
        let adapter = Arc::new(Fixture::default());
        let lease = lifecycle.admit().unwrap();
        if apply {
            lifecycle
                .apply("update", || Ok(candidate()), adapter.clone())
                .unwrap();
        } else {
            lifecycle.quit(adapter.clone());
        }
        assert!(lifecycle.restart(adapter.clone()).is_err());
        drop(lease);
        lifecycle.finished().await;
        assert_eq!(adapter.installs.load(Ordering::SeqCst), usize::from(apply));
        assert_eq!(adapter.exits.load(Ordering::SeqCst), usize::from(!apply));
    }
}

/// Verifies: TEAM.SYS.SHELL-R07
#[tokio::test]
async fn failed_ordinary_restart_is_terminal_and_never_restarts() {
    let lifecycle = Lifecycle::new();
    let adapter = Arc::new(Fixture {
        host_error: true,
        ..Default::default()
    });
    lifecycle.restart(adapter.clone()).unwrap();
    lifecycle.finished().await;
    assert_eq!(lifecycle.status().phase, Phase::Failed);
    assert_eq!(lifecycle.status().failure, Some(FailureCategory::Shutdown));
    assert!(!adapter.order.lock().unwrap().contains(&"restart"));
    assert!(lifecycle.restart(adapter.clone()).is_err());
    assert!(lifecycle.admit().is_err());
    assert_eq!(adapter.installs.load(Ordering::SeqCst), 0);
    lifecycle.quit(adapter.clone());
    assert_eq!(adapter.exits.load(Ordering::SeqCst), 1);
}

/// Verifies: TEAM.SYS.SHELL-R07
#[tokio::test(start_paused = true)]
async fn timed_out_ordinary_restart_joins_late_cleanup_without_restarting() {
    let lifecycle = Lifecycle::on_runtime(tokio::runtime::Handle::current());
    let adapter = Arc::new(Fixture {
        block: true,
        ..Default::default()
    });
    lifecycle.restart(adapter.clone()).unwrap();
    adapter.entered.notified().await;
    tokio::time::advance(SHUTDOWN_DEADLINE).await;
    lifecycle.wait_for_phase(Phase::Failed).await;
    assert_eq!(
        lifecycle.status().failure,
        Some(FailureCategory::ShutdownTimeout)
    );
    adapter.release.notify_one();
    lifecycle.finished().await;
    assert!(!adapter.order.lock().unwrap().contains(&"restart"));
    assert_eq!(adapter.installs.load(Ordering::SeqCst), 0);
    assert!(lifecycle.admit().is_err());
}
