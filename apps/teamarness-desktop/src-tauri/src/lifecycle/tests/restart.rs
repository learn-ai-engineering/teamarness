use super::*;
use crate::test_temp_dir as tempfile;

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
    assert_eq!(adapter.retirements.load(Ordering::SeqCst), 0);
    assert!(lifecycle.admit().is_err());
}

/// Verifies: TEAM.SYS.ID-R07, TEAM.SYS.SHELL-R07
#[tokio::test]
async fn replacement_acquires_only_after_drain_successful_cleanup_and_explicit_retirement() {
    use teamarness_core::{CoreRuntime, CoreRuntimeConfig};
    struct Owned {
        runtime: Arc<CoreRuntime>,
        directory: PathBuf,
        errors: ShutdownErrors,
        replaced: AtomicUsize,
    }
    impl TerminalAdapter for Owned {
        fn stop_updates(&self) {}
        fn cleanup(&self) -> BoxFuture<'_, ShutdownErrors> {
            Box::pin(async {
                self.runtime.shutdown().await.unwrap();
                self.errors.clone()
            })
        }
        fn install(&self, pending: Install) -> Result<(), InstallFailure> {
            pending()
        }
        fn retire_ownership(&self) -> Result<(), ()> {
            self.runtime.retire_application_ownership().map_err(|_| ())
        }
        fn restart(&self) {
            // Tauri starts the replacement while the old Arc still exists.
            let directory = self.directory.clone();
            // The synchronous adapter cannot block this executor: inspect the
            // real kernel lock, then the parent opens Core below.
            let lock = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(directory.join("application.lock"))
                .unwrap();
            lock.try_lock().expect("replacement owns the retired lock");
            self.replaced.fetch_add(1, Ordering::SeqCst);
        }
        fn exit(&self) {}
    }
    for update in [false, true] {
        for errors in [
            ShutdownErrors::default(),
            ShutdownErrors {
                host: true,
                ..Default::default()
            },
            ShutdownErrors {
                core: true,
                ..Default::default()
            },
            ShutdownErrors {
                uncertain: true,
                ..Default::default()
            },
        ] {
            let fail = errors.failed();
            let root = tempfile::tempdir().unwrap();
            let directory = root.path().join("private");
            let config = CoreRuntimeConfig::new(&directory).with_exclusive_application_ownership();
            let runtime = Arc::new(
                CoreRuntime::open_loopback_for_tests(config.clone())
                    .await
                    .unwrap(),
            );
            let adapter = Arc::new(Owned {
                runtime,
                directory,
                errors,
                replaced: AtomicUsize::new(0),
            });
            let lifecycle = Lifecycle::new();
            let admitted = lifecycle.admit().unwrap();
            if update {
                lifecycle
                    .apply("1", || Ok(candidate()), adapter.clone())
                    .unwrap();
            } else {
                lifecycle.restart(adapter.clone()).unwrap();
            }
            assert!(lifecycle.admit().is_err());
            assert!(
                CoreRuntime::open_loopback_for_tests(config.clone())
                    .await
                    .is_err()
            );
            drop(admitted);
            lifecycle.finished().await;
            assert_eq!(adapter.replaced.load(Ordering::SeqCst), usize::from(!fail));
            let replacement = CoreRuntime::open_loopback_for_tests(config).await;
            if fail {
                assert!(replacement.is_err());
            } else {
                let replacement = replacement.unwrap();
                assert_eq!(
                    replacement.public_identity(),
                    adapter.runtime.public_identity()
                );
                assert!(adapter.runtime.set_display_name("retired").is_err());
                replacement.shutdown().await.unwrap();
            }
        }
    }
}
