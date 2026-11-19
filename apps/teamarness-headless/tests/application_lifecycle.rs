mod support;

use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use teamarness_core::CoreRuntime;
use teamarness_headless::application::{Application, Readiness};
use teamarness_headless::control::{self, ControlServer};
use tokio::sync::{Notify, oneshot};
use tokio::time::{Instant, timeout};

#[tokio::test]
async fn first_start_persists_identity_and_optional_package_free_status_is_truthful() {
    let fixture = support::Fixture::new();
    let app = fixture.open().await;
    let status = app.status().await;
    assert_eq!(status.readiness, Readiness::Ready);
    assert!(status.host.packages.is_empty());
    assert!(status.host.instances.is_empty());
    assert!(status.host.diagnostics.is_empty());
    app.shutdown_by(Instant::now() + control::SHUTDOWN_TIMEOUT)
        .await;
    assert!(
        Application::open_with(fixture.config.clone(), CoreRuntime::open_loopback_for_tests)
            .await
            .is_err(),
        "cleanup alone must not retire ownership"
    );
    drop(app);
    let reopened = fixture.open().await;
    assert_eq!(reopened.status().await.installation, status.installation);
    reopened
        .shutdown_by(Instant::now() + control::SHUTDOWN_TIMEOUT)
        .await;
}

#[tokio::test]
async fn host_open_failure_rolls_back_core_and_allows_same_identity_recovery() {
    let fixture = support::Fixture::new();
    let app = fixture.open().await;
    let identity = app.status().await.installation;
    app.shutdown_by(Instant::now() + control::SHUTDOWN_TIMEOUT)
        .await;
    drop(app);
    // Host store creation fails after exclusive Core has already opened.
    std::fs::remove_dir_all(fixture.config.data_directory.join("packages")).unwrap();
    std::fs::write(
        fixture.config.data_directory.join("packages"),
        b"not a directory",
    )
    .unwrap();
    let failure =
        Application::open_with(fixture.config.clone(), CoreRuntime::open_loopback_for_tests).await;
    assert!(failure.err().unwrap().starts_with("host_open:"));
    std::fs::remove_file(fixture.config.data_directory.join("packages")).unwrap();
    let recovered = fixture.open().await;
    assert_eq!(recovered.status().await.installation, identity);
    recovered
        .shutdown_by(Instant::now() + control::SHUTDOWN_TIMEOUT)
        .await;
}

#[tokio::test]
async fn signal_already_received_before_endpoint_binding_enters_cleanup_without_admission() {
    let fixture = support::Fixture::new();
    let app = fixture.open().await;
    std::fs::write(fixture.socket(), b"unrelated occupant").unwrap();
    app.run_until(async { Instant::now() }).await.unwrap();
    assert_eq!(
        std::fs::read(fixture.socket()).unwrap(),
        b"unrelated occupant"
    );
    assert_eq!(
        app.status().await.host.state,
        teamarness_package_host::PackageHostState::Stopped
    );
}

#[tokio::test]
async fn owned_operation_drain_precedes_host_shutdown_and_retains_lock() {
    let fixture = support::Fixture::new();
    let app = fixture.open().await;
    let server = ControlServer::bind(&app).await.unwrap();
    let status = app.status().await;
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (stop, stopped) = oneshot::channel();
    let dispatch = {
        let (entered, release) = (entered.clone(), release.clone());
        move |_| {
            let (entered, release, status) = (entered.clone(), release.clone(), status.clone());
            async move {
                entered.notify_one();
                release.notified().await;
                Ok(status)
            }
        }
    };
    let accepting = server.accept_until(dispatch, async { stopped.await.unwrap() });
    let clients = async {
        let mut client = tokio::net::UnixStream::connect(fixture.socket())
            .await
            .unwrap();
        control::write_frame(&mut client, &support::request(1))
            .await
            .unwrap();
        entered.notified().await;
        drop(client);
        stop.send(Instant::now()).unwrap();
    };
    let (mut drain, ()) = tokio::join!(accepting, clients);
    let finishing = app.finish_shutdown(&mut drain);
    tokio::pin!(finishing);
    tokio::select! { biased; _ = &mut finishing => panic!("cleanup overtook owned work"), _ = tokio::task::yield_now() => {} }
    assert_eq!(
        app.status().await.host.state,
        teamarness_package_host::PackageHostState::Running
    );
    assert!(
        Application::open_with(fixture.config.clone(), CoreRuntime::open_loopback_for_tests)
            .await
            .is_err()
    );
    release.notify_one();
    finishing.await;
    assert_eq!(
        app.status().await.host.state,
        teamarness_package_host::PackageHostState::Stopped
    );
}

struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn child(fixture: &support::Fixture, mode: &str) -> Child {
    Child(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "headless_process_entry", "--nocapture"])
            .env("TEAMARNESS_TEST_CHILD", mode)
            .env("TEAMARNESS_TEST_ROOT", fixture.root.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    )
}
async fn wait_status(
    fixture: &support::Fixture,
    child: &mut Child,
) -> teamarness_headless::application::Status {
    timeout(Duration::from_secs(15), async {
        loop {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "foreground process exited before readiness"
            );
            if let Ok(response) =
                control::request(&fixture.config.data_directory, support::request(1)).await
            {
                return response.result.unwrap();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("foreground socket becomes ready")
}
async fn wait_exit(child: &mut Child) -> std::process::ExitStatus {
    timeout(Duration::from_secs(15), async {
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("foreground process terminates")
}

#[tokio::test]
async fn sigint_and_sigterm_stop_actual_cli_composition_and_restart_preserves_identity() {
    let fixture = support::Fixture::new();
    let mut identity = None;
    for signal in [Signal::SIGINT, Signal::SIGTERM] {
        let mut server = child(&fixture, "serve");
        let status = wait_status(&fixture, &mut server).await;
        let client = tokio::process::Command::new(env!("CARGO_BIN_EXE_teamarness-headless"))
            .arg("--data-dir")
            .arg(&fixture.config.data_directory)
            .arg("status")
            .output()
            .await
            .unwrap();
        assert!(client.status.success(), "{:?}", client.stderr);
        let cli_status: teamarness_headless::application::Status =
            serde_json::from_slice(&client.stdout).unwrap();
        assert_eq!(cli_status.installation, status.installation);
        assert_eq!(cli_status.readiness, Readiness::Ready);
        if let Some(identity) = &identity {
            assert_eq!(&status.installation, identity);
        }
        identity = Some(status.installation);
        let mut duplicate = child(&fixture, "serve");
        assert!(!wait_exit(&mut duplicate).await.success());
        // A losing duplicate must not unlink the original live endpoint.
        assert!(
            control::request(&fixture.config.data_directory, support::request(2))
                .await
                .unwrap()
                .result
                .is_ok()
        );
        kill(Pid::from_raw(server.0.id() as i32), signal).unwrap();
        assert!(wait_exit(&mut server).await.success());
        assert!(!fixture.socket().exists());
    }
}

#[tokio::test]
async fn deadline_failure_is_nonzero_uncertain_and_does_not_continue_unlocked() {
    use std::io::Read;
    let fixture = support::Fixture::new();
    let mut server = child(&fixture, "deadline");
    assert_eq!(wait_exit(&mut server).await.code(), Some(1));
    let mut stderr = String::new();
    server
        .0
        .stderr
        .as_mut()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(
        stderr.contains("shutdown_failed stage=operation_drain outcomeUncertain=true"),
        "{stderr}"
    );
    assert!(!fixture.root.path().join("continued-after-timeout").exists());
    // Kernel ownership is released by process death, not a background task/drop handoff.
    let reopened = fixture.open().await;
    reopened
        .shutdown_by(Instant::now() + control::SHUTDOWN_TIMEOUT)
        .await;
}

/// Test-only executable: the production CLI has no environment, relay, profile, or deadline bypass.
#[test]
fn headless_process_entry() {
    let Ok(mode) = std::env::var("TEAMARNESS_TEST_CHILD") else {
        return;
    };
    let root = std::path::PathBuf::from(std::env::var_os("TEAMARNESS_TEST_ROOT").unwrap());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let config = teamarness_headless::application::ApplicationConfig { data_directory: root.join("data"), application_root: root.join("app") };
        if mode == "serve" || mode == "serve-package" {
            let args = vec!["serve".into(), "--data-dir".into(), config.data_directory.into_os_string(), "--application-root".into(), config.application_root.into_os_string()];
            std::process::exit(teamarness_headless::cli::run_with(args, |config| async {
                let core = CoreRuntime::open_loopback_for_tests(config).await?;
                if mode == "serve-package" && core.list_workspaces().unwrap().is_empty() { core.create_workspace("Headless lifecycle fixture").unwrap(); }
                Ok(core)
            }).await);
        }
        let panic_cleanup = mode == "panic-cleanup";
        let app = Application::open_with(config, |config| async {
            let core = CoreRuntime::open_loopback_for_tests(config).await?;
            if panic_cleanup { core.create_workspace("Cleanup failure fixture").unwrap(); }
            Ok(core)
        }).await.unwrap();
        let server = ControlServer::bind(&app).await.unwrap();
        let entered = Arc::new(Notify::new());
        let (stop, stopped) = oneshot::channel();
        let dispatch = { let entered = entered.clone(); move |_| { let entered = entered.clone(); async move { entered.notify_one(); assert!(!panic_cleanup, "fixture dispatch panic"); std::future::pending().await } } };
        let accepting = server.accept_until(dispatch, async { stopped.await.unwrap() });
        let client = async {
            let mut stream = tokio::net::UnixStream::connect(root.join("data/control.sock")).await.unwrap();
            control::write_frame(&mut stream, &support::request(1)).await.unwrap();
            entered.notified().await;
            stop.send(Instant::now()).unwrap();
        };
        let (mut drain, ()) = tokio::join!(accepting, client);
        if panic_cleanup { app.finish_shutdown(&mut drain).await; panic!("uncertain drain returned success"); }
        // Advance only the clock, not the production shutdown budget.
        tokio::time::pause();
        let finishing = app.finish_shutdown(&mut drain);
        tokio::pin!(finishing);
        tokio::select! { biased; _ = &mut finishing => panic!("blocked operation finished"), _ = tokio::task::yield_now() => {} }
        tokio::time::advance(control::SHUTDOWN_TIMEOUT).await;
        finishing.await;
        std::fs::write(root.join("continued-after-timeout"), b"unsafe").unwrap();
    });
}

#[path = "../../../crates/teamarness-package-host/tests/common/mod.rs"]
mod host_fixture;

fn stage_lifecycle_package(fixture: &support::Fixture) -> (std::path::PathBuf, std::path::PathBuf) {
    let package = fixture.config.application_root.join("fixture");
    host_fixture::write_node_package(&package, "dev.teamarness.headless-lifecycle");
    let pid_file = fixture.root.path().join("backend.pid");
    let shutdown_file = fixture.root.path().join("backend-shutdown");
    let source = std::fs::read_to_string(package.join("backend/package.mjs")).unwrap();
    let source = source
        .replacen(
            "#!/usr/bin/env node",
            "#!/usr/bin/env node\nimport {writeFileSync} from 'node:fs';",
            1,
        )
        .replace(
            "if (frame.kind === \"initialize\") {",
            &format!(
                "if (frame.kind === \"initialize\") {{ writeFileSync({}, String(process.pid));",
                serde_json::to_string(&pid_file).unwrap()
            ),
        )
        .replace(
            "send({ kind: \"shutdown_ack\" });",
            &format!(
                "writeFileSync({}, 'shutdown-before-exit'); send({{ kind: \"shutdown_ack\" }});",
                serde_json::to_string(&shutdown_file).unwrap()
            ),
        );
    std::fs::write(package.join("backend/package.mjs"), &source).unwrap();
    let manifest_path = package.join("teamarness.package.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["backends"][0]["sha256"] = host_fixture::digest(source.as_bytes()).into();
    manifest["backends"][0]["size"] = source.len().into();
    std::fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    std::fs::write(
        fixture
            .config
            .application_root
            .join("packages/application-packages.v1.json"),
        br#"{"schemaVersion":1,"packages":[{"directory":"fixture"}]}"#,
    )
    .unwrap();
    (pid_file, shutdown_file)
}

#[tokio::test]
async fn foreground_shutdown_stops_real_package_process_before_successful_exit() {
    let fixture = support::Fixture::new();
    let (pid_file, shutdown_file) = stage_lifecycle_package(&fixture);
    let mut server = child(&fixture, "serve-package");
    let status = wait_status(&fixture, &mut server).await;
    assert_eq!(
        status.host.instances.len(),
        1,
        "{:?}",
        status.host.diagnostics
    );
    assert_eq!(
        status.host.instances[0].lifecycle,
        teamarness_package_host::LifecycleState::Active,
        "{:?}",
        status.host.diagnostics
    );
    let pid: i32 = std::fs::read_to_string(pid_file).unwrap().parse().unwrap();
    assert!(kill(Pid::from_raw(pid), None).is_ok());
    kill(Pid::from_raw(server.0.id() as i32), Signal::SIGTERM).unwrap();
    assert!(wait_exit(&mut server).await.success());
    assert_eq!(
        std::fs::read_to_string(shutdown_file).unwrap(),
        "shutdown-before-exit"
    );
    assert_eq!(
        kill(Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH),
        "Backend must be reaped before foreground success"
    );
}

#[tokio::test]
async fn uncertain_completed_operation_still_attempts_host_cleanup_before_nonzero_exit() {
    let fixture = support::Fixture::new();
    let (_pid_file, shutdown_file) = stage_lifecycle_package(&fixture);
    let mut server = child(&fixture, "panic-cleanup");
    assert_eq!(wait_exit(&mut server).await.code(), Some(1));
    assert!(
        shutdown_file.exists(),
        "a completed uncertain operation must not skip Host cleanup"
    );
}

#[tokio::test]
async fn admission_diagnostics_are_degraded_not_global_synchronization_success() {
    let fixture = support::Fixture::new();
    std::fs::write(
        fixture
            .config
            .application_root
            .join("packages/application-packages.v1.json"),
        b"malformed",
    )
    .unwrap();
    let app = fixture.open().await;
    let status = app.status().await;
    assert_eq!(status.readiness, Readiness::Degraded);
    assert!(!status.host.diagnostics.is_empty());
    app.shutdown_by(Instant::now() + control::SHUTDOWN_TIMEOUT)
        .await;
}
