use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct Fixture {
    order: Mutex<Vec<&'static str>>,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    block: bool,
    host_error: bool,
    install_error: bool,
    installs: AtomicUsize,
    exits: AtomicUsize,
}
impl TerminalAdapter for Fixture {
    fn stop_updates(&self) {}
    fn cleanup(&self) -> BoxFuture<'_, ShutdownErrors> {
        Box::pin(async move {
            self.entered.notify_one();
            if self.block {
                self.release.notified().await;
            }
            self.order
                .lock()
                .unwrap()
                .extend(["mounts", "host", "core"]);
            ShutdownErrors {
                host: self.host_error,
                core: false,
                uncertain: false,
            }
        })
    }
    fn install(&self, pending: Install) -> Result<(), InstallFailure> {
        self.installs.fetch_add(1, Ordering::SeqCst);
        self.order.lock().unwrap().push("install");
        if self.install_error {
            Err(InstallFailure { previous_app: None })
        } else {
            pending()
        }
    }
    fn restart(&self) {
        self.order.lock().unwrap().push("restart");
    }
    fn exit(&self) {
        self.exits.fetch_add(1, Ordering::SeqCst);
    }
}
fn candidate() -> Install {
    Box::new(|| Ok(()))
}

/// Verifies: TEAM.SYS.SHELL-R01, TEAM.SYS.SHELL-R07
#[tokio::test]
async fn admitted_work_drains_and_two_applies_share_one_owned_operation() {
    let lifecycle = Lifecycle::new();
    let adapter = Arc::new(Fixture::default());
    let lease = lifecycle.admit().unwrap();
    lifecycle
        .apply("1", || Ok(candidate()), adapter.clone())
        .unwrap();
    lifecycle
        .apply("1", || panic!("must not acquire twice"), adapter.clone())
        .unwrap();
    assert!(lifecycle.admit().is_err());
    assert_eq!(lifecycle.status().phase, Phase::Stopping);
    assert!(adapter.order.lock().unwrap().is_empty());
    drop(lease);
    lifecycle.finished().await;
    assert_eq!(
        *adapter.order.lock().unwrap(),
        ["mounts", "host", "core", "install", "restart"]
    );
    assert_eq!(adapter.installs.load(Ordering::SeqCst), 1);
    assert_eq!(adapter.exits.load(Ordering::SeqCst), 0);
}

/// Verifies: TEAM.SYS.SHELL-R07
#[tokio::test]
async fn quit_first_never_acquires_install_and_apply_first_ignores_exit_events() {
    let lifecycle = Lifecycle::new();
    let adapter = Arc::new(Fixture::default());
    let lease = lifecycle.admit().unwrap();
    lifecycle.quit(adapter.clone());
    lifecycle.quit(adapter.clone());
    assert!(
        lifecycle
            .apply("1", || panic!("quit wins"), adapter.clone())
            .is_err()
    );
    drop(lease);
    lifecycle.finished().await;
    assert_eq!(adapter.exits.load(Ordering::SeqCst), 1);
    assert_eq!(adapter.installs.load(Ordering::SeqCst), 0);

    let lifecycle = Lifecycle::new();
    let lease = lifecycle.admit().unwrap();
    lifecycle
        .apply("1", || Ok(candidate()), adapter.clone())
        .unwrap();
    lifecycle.quit(adapter.clone());
    lifecycle.quit(adapter.clone());
    drop(lease);
    lifecycle.finished().await;
    assert_eq!(adapter.exits.load(Ordering::SeqCst), 1);
    assert_eq!(adapter.installs.load(Ordering::SeqCst), 1);
}

/// Verifies: TEAM.SYS.SHELL-R07
#[tokio::test(start_paused = true)]
async fn timeout_keeps_cleanup_owned_and_late_success_cannot_install() {
    let lifecycle = Lifecycle::on_runtime(tokio::runtime::Handle::current());
    let adapter = Arc::new(Fixture {
        block: true,
        ..Default::default()
    });
    lifecycle
        .apply("1", || Ok(candidate()), adapter.clone())
        .unwrap();
    adapter.entered.notified().await;
    tokio::time::advance(SHUTDOWN_DEADLINE).await;
    lifecycle.wait_for_phase(Phase::Failed).await;
    assert_eq!(
        lifecycle.status().failure,
        Some(FailureCategory::ShutdownTimeout)
    );
    lifecycle.quit(adapter.clone());
    assert_eq!(adapter.exits.load(Ordering::SeqCst), 0);
    adapter.release.notify_one();
    lifecycle.finished().await;
    assert_eq!(adapter.installs.load(Ordering::SeqCst), 0);
    assert_eq!(adapter.exits.load(Ordering::SeqCst), 1);
    assert!(lifecycle.admit().is_err());
}

/// Verifies: TEAM.SYS.SHELL-R07
#[tokio::test]
async fn stale_admission_and_failures_never_restart_or_reopen_dispatch() {
    let lifecycle = Lifecycle::new();
    let adapter = Arc::new(Fixture::default());
    assert!(
        lifecycle
            .apply("stale", || Err(()), adapter.clone())
            .is_err()
    );
    assert_eq!(lifecycle.status().phase, Phase::Running);
    for (host_error, install_error) in [(true, false), (false, true)] {
        let lifecycle = Lifecycle::new();
        let adapter = Arc::new(Fixture {
            host_error,
            install_error,
            ..Default::default()
        });
        lifecycle
            .apply("1", || Ok(candidate()), adapter.clone())
            .unwrap();
        lifecycle.finished().await;
        assert_eq!(lifecycle.status().phase, Phase::Failed);
        assert_eq!(
            adapter.installs.load(Ordering::SeqCst),
            usize::from(!host_error)
        );
        assert!(!adapter.order.lock().unwrap().contains(&"restart"));
        assert!(
            lifecycle
                .apply("1", || panic!("no retry"), adapter.clone())
                .is_err()
        );
        assert!(lifecycle.admit().is_err());
    }
}

/// Verifies: TEAM.SYS.SHELL-R07
#[tokio::test]
async fn cancelled_waiter_does_not_abandon_ordinary_work() {
    let lifecycle = Lifecycle::new();
    let (entered, received) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let operation = lifecycle.clone();
    let waiter = tokio::spawn(async move {
        operation
            .dispatch(async move {
                entered.send(()).unwrap();
                released.await.unwrap();
                42
            })
            .await
    });
    received.await.unwrap();
    waiter.abort();
    let adapter = Arc::new(Fixture::default());
    lifecycle.quit(adapter.clone());
    assert!(adapter.order.lock().unwrap().is_empty());
    release.send(()).unwrap();
    lifecycle.finished().await;
    assert_eq!(*adapter.order.lock().unwrap(), ["mounts", "host", "core"]);
}

/// Verifies: TEAM.SYS.SHELL-R07
#[tokio::test]
async fn shutdown_handoff_attempts_core_after_host_error_and_aggregates_errors() {
    let order = Mutex::new(Vec::new());
    let errors = crate::shutdown_handoff(
        async {
            order.lock().unwrap().push("mounts");
        },
        async {
            order.lock().unwrap().push("host");
            (true, true)
        },
        async {
            order.lock().unwrap().push("core");
            true
        },
    )
    .await;
    assert_eq!(*order.lock().unwrap(), ["mounts", "host", "core"]);
    assert!(errors.host && errors.core && errors.uncertain);
}

/// Verifies: TEAM.SYS.SHELL-R07
#[tokio::test]
async fn installation_has_no_abandon_deadline_and_recovery_quit_cannot_race_it() {
    let lifecycle = Lifecycle::new();
    let adapter = Arc::new(Fixture::default());
    let (entered, received) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let install: Install = Box::new(move || {
        entered.send(()).unwrap();
        released.recv().unwrap();
        Err(InstallFailure {
            previous_app: Some(PathBuf::from("fixture-old.app")),
        })
    });
    // This adapter executes the actual injected blocking installer.
    struct Execute(Arc<Fixture>);
    impl TerminalAdapter for Execute {
        fn stop_updates(&self) {}
        fn cleanup(&self) -> BoxFuture<'_, ShutdownErrors> {
            self.0.cleanup()
        }
        fn install(&self, pending: Install) -> Result<(), InstallFailure> {
            pending()
        }
        fn restart(&self) {
            panic!("failed installation must not restart")
        }
        fn exit(&self) {
            self.0.exit();
        }
    }
    let terminal = Arc::new(Execute(adapter.clone()));
    lifecycle
        .apply("1", || Ok(install), terminal.clone())
        .unwrap();
    received.await.unwrap();
    lifecycle.quit(terminal.clone());
    assert_eq!(lifecycle.status().phase, Phase::Installing);
    assert_eq!(adapter.exits.load(Ordering::SeqCst), 0);
    release.send(()).unwrap();
    lifecycle.finished().await;
    let status = lifecycle.status();
    assert_eq!(status.phase, Phase::Failed);
    assert_eq!(
        status.previous_app.as_deref(),
        Some(std::path::Path::new("fixture-old.app"))
    );
    assert!(status.guidance.unwrap().contains("reinstall"));
    lifecycle.quit(terminal);
    assert_eq!(adapter.exits.load(Ordering::SeqCst), 1);
}

/// Verifies: TEAM.SYS.SHELL-R07
#[test]
fn native_quit_after_returned_block_on_owns_cleanup_and_exit() {
    let lifecycle = tauri::async_runtime::block_on(async { Lifecycle::new() });
    assert!(tokio::runtime::Handle::try_current().is_err());
    let adapter = Arc::new(Fixture::default());
    let lease = lifecycle.admit().unwrap();
    lifecycle.quit(adapter.clone());
    lifecycle.quit(adapter.clone());
    assert!(
        lifecycle
            .apply("1", || panic!("Quit won"), adapter.clone())
            .is_err()
    );
    assert!(lifecycle.admit().is_err());
    drop(lease);
    tauri::async_runtime::block_on(async {
        tokio::time::timeout(Duration::from_secs(10), lifecycle.finished())
            .await
            .unwrap();
    });
    assert_eq!(*adapter.order.lock().unwrap(), ["mounts", "host", "core"]);
    assert_eq!(adapter.exits.load(Ordering::SeqCst), 1);
    assert_eq!(adapter.installs.load(Ordering::SeqCst), 0);
    assert!(lifecycle.exit_allowed());
}

/// Verifies: TEAM.SYS.SHELL-R07
#[test]
fn native_apply_after_returned_block_on_owns_one_generation_and_restart() {
    let lifecycle = tauri::async_runtime::block_on(async { Lifecycle::new() });
    assert!(tokio::runtime::Handle::try_current().is_err());
    let adapter = Arc::new(Fixture::default());
    let lease = lifecycle.admit().unwrap();
    assert!(
        lifecycle
            .apply("stale", || Err(()), adapter.clone())
            .is_err()
    );
    lifecycle
        .apply("1", || Ok(candidate()), adapter.clone())
        .unwrap();
    lifecycle
        .apply("1", || panic!("already acquired"), adapter.clone())
        .unwrap();
    assert!(
        lifecycle
            .apply("other", || panic!("wrong generation"), adapter.clone())
            .is_err()
    );
    lifecycle.quit(adapter.clone());
    assert!(lifecycle.admit().is_err());
    drop(lease);
    tauri::async_runtime::block_on(async {
        tokio::time::timeout(Duration::from_secs(10), lifecycle.finished())
            .await
            .unwrap();
    });
    assert_eq!(
        *adapter.order.lock().unwrap(),
        ["mounts", "host", "core", "install", "restart"]
    );
    assert_eq!(adapter.installs.load(Ordering::SeqCst), 1);
    assert_eq!(adapter.exits.load(Ordering::SeqCst), 0);
}
