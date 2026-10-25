use super::*;
use crate::{
    lifecycle::{self, Install, InstallFailure, TerminalAdapter},
    package_protocol,
};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Manager;

struct RealShutdown {
    state: DesktopState,
    installed: AtomicBool,
    restarted: AtomicBool,
    process_group: Option<i32>,
}
fn group_alive(group: i32) -> bool {
    std::process::Command::new("/bin/kill")
        .args(["-0", "--", &format!("-{group}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}
impl TerminalAdapter for RealShutdown {
    fn stop_updates(&self) {
        if let Some(updates) = &self.state.updates {
            updates.stop();
        }
    }
    fn cleanup(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = lifecycle::ShutdownErrors> + Send + '_>>
    {
        Box::pin(self.state.cleanup())
    }
    fn install(&self, pending: Install) -> Result<(), InstallFailure> {
        assert!(self.state.protocol_mounts.try_lock().unwrap().is_empty());
        if let Some(group) = self.process_group {
            assert!(!group_alive(group), "old Backend process group survives");
        }
        pending()?;
        assert!(!self.installed.swap(true, Ordering::SeqCst));
        Ok(())
    }
    fn restart(&self) {
        assert!(self.installed.load(Ordering::SeqCst));
        self.restarted.store(true, Ordering::SeqCst);
    }
    fn exit(&self) {}
}
fn adapter(state: DesktopState) -> Arc<RealShutdown> {
    Arc::new(RealShutdown {
        state,
        installed: AtomicBool::new(false),
        restarted: AtomicBool::new(false),
        process_group: None,
    })
}

/// Verifies: TEAM.SYS.SHELL-R01, TEAM.SYS.SHELL-R05
#[test]
fn actual_command_and_resource_adapters_reject_after_terminal_admission() {
    tauri::async_runtime::block_on(async {
        let (_root, core, host) = fixture().await;
        let state = DesktopState::new(core, host);
        let app = tauri::test::mock_builder()
            .manage(state.clone())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let initial = package_catalog_snapshot(app.state()).await.unwrap();
        assert!(
            !serde_json::to_value(initial).unwrap()["instances"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let key = state.packages.snapshot().await.instances[0].key.clone();
        let mount = package_open_mount(PackageInstanceKeyDto::from(key), app.state())
            .await
            .unwrap();
        let lease = state.lifecycle.admit().unwrap();
        let terminal = adapter(state.clone());
        state.lifecycle.quit(terminal);
        assert!(
            crate::commands::installation_details(app.state())
                .await
                .is_err()
        );
        assert!(package_catalog_snapshot(app.state()).await.is_err());
        assert_eq!(state.lifecycle.status().phase, lifecycle::Phase::Stopping);
        let request = tauri::http::Request::builder()
            .uri(format!(
                "teamarness-package://localhost/{}/__teamarness/frame.html",
                mount.loader.token
            ))
            .body(vec![])
            .unwrap();
        let (sent, received) = tokio::sync::oneshot::channel();
        package_protocol::respond(state.clone(), request, move |response| {
            sent.send(response.status()).unwrap();
        })
        .await;
        assert_eq!(
            received.await.unwrap(),
            tauri::http::StatusCode::SERVICE_UNAVAILABLE
        );
        drop(lease);
        state.lifecycle.finished().await;
    });
}

/// Verifies: TEAM.SYS.SHELL-R01
#[test]
fn real_shutdown_kills_process_group_and_preserves_reopened_state_before_fake_restart() {
    tauri::async_runtime::block_on(async {
        let (root, core, host) = fixture_processes(true).await;
        let identity = core.public_identity().to_string();
        let workspaces = core.list_workspaces().unwrap();
        let pids: Vec<i32> =
            serde_json::from_slice(&fs::read(root.path().join("processes.json")).unwrap()).unwrap();
        assert!(group_alive(pids[0]));
        let marker = find_marker(&root.path().join("private"));
        assert_eq!(fs::read_to_string(&marker).unwrap(), "1");
        let state = DesktopState::new(core.clone(), host.clone());
        let terminal = Arc::new(RealShutdown {
            process_group: Some(pids[0]),
            ..Arc::try_unwrap(adapter(state.clone())).ok().unwrap()
        });
        state
            .lifecycle
            .apply("fixture", || Ok(Box::new(|| Ok(()))), terminal.clone())
            .unwrap();
        state.lifecycle.finished().await;
        assert!(terminal.restarted.load(Ordering::SeqCst));
        assert_eq!(state.lifecycle.status().phase, lifecycle::Phase::Restarting);
        assert!(!group_alive(pids[0]));
        let reopened = Arc::new(
            CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(root.path().join("core")))
                .await
                .unwrap(),
        );
        assert_eq!(reopened.public_identity().to_string(), identity);
        assert_eq!(
            reopened.list_workspaces().unwrap()[0].id(),
            workspaces[0].id()
        );
        let reopened_host = PackageHost::open(
            PackageHostConfig::new(
                root.path().join("application"),
                root.path().join("application/application-packages.v1.json"),
                root.path().join("private"),
                InstallationPublicIdentity::new(identity).unwrap(),
            ),
            reopened.clone(),
        )
        .await
        .unwrap();
        assert_eq!(fs::read_to_string(&marker).unwrap(), "2");
        reopened_host
            .execute(PackageHostCommand::Shutdown(ShutdownHost))
            .await
            .unwrap();
        reopened.shutdown().await.unwrap();
    });
}
fn find_marker(path: &Path) -> std::path::PathBuf {
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().unwrap() == "lifecycle-marker" {
            return path;
        }
        if path.is_dir()
            && let Some(found) = find_marker_optional(&path)
        {
            return found;
        }
    }
    panic!("missing durable fixture marker")
}
fn find_marker_optional(path: &Path) -> Option<std::path::PathBuf> {
    for entry in fs::read_dir(path).ok()? {
        let path = entry.ok()?.path();
        if path.file_name()? == "lifecycle-marker" {
            return Some(path);
        }
        if path.is_dir()
            && let Some(found) = find_marker_optional(&path)
        {
            return Some(found);
        }
    }
    None
}

#[test]
fn resource_response_lease_survives_dropped_waiter_through_responder() {
    tauri::async_runtime::block_on(async {
        let (_root, core, host) = fixture().await;
        let state = DesktopState::new(core, host);
        let key = state.packages.snapshot().await.instances[0].key.clone();
        let PackageHostOutcome::UiMountOpened { mount, loader, .. } = state
            .packages
            .execute(PackageHostCommand::OpenUiMount(OpenUiMount {
                instance: key.clone(),
            }))
            .await
            .unwrap()
        else {
            panic!()
        };
        state
            .register_protocol_mount(loader.token.as_str(), &mount, &key)
            .await;
        let request = tauri::http::Request::builder()
            .uri(format!(
                "teamarness-package://localhost/{}/__teamarness/frame.html",
                loader.token.as_str()
            ))
            .body(vec![])
            .unwrap();
        let (entered, received) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let serving = state.clone();
        let waiter = tauri::async_runtime::spawn(async move {
            package_protocol::respond(serving, request, move |response| {
                assert_eq!(response.status(), tauri::http::StatusCode::OK);
                entered.send(()).unwrap();
                released.recv().unwrap();
            })
            .await;
        });
        received.await.unwrap();
        waiter.abort();
        let terminal = adapter(state.clone());
        state
            .lifecycle
            .apply("fixture", || Ok(Box::new(|| Ok(()))), terminal.clone())
            .unwrap();
        assert!(!terminal.installed.load(Ordering::SeqCst));
        assert!(!state.protocol_mounts.lock().await.is_empty());
        release.send(()).unwrap();
        state.lifecycle.finished().await;
        assert!(terminal.restarted.load(Ordering::SeqCst));
    });
}

#[test]
fn idle_subscriptions_cancel_without_holding_drain_open() {
    tauri::async_runtime::block_on(async {
        let (_root, core, host) = fixture().await;
        let workspace = core.list_workspaces().unwrap()[0].id().to_string();
        let cursor = core
            .principals_snapshot(workspace.parse().unwrap())
            .unwrap()
            .0
            .to_string();
        let state = DesktopState::new(core, host);
        let app = tauri::test::mock_builder()
            .manage(state.clone())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        package_subscribe_host_state(0, Channel::new(|_| Ok(())), app.state())
            .await
            .unwrap();
        crate::commands::subscribe_principals(
            workspace,
            cursor,
            Channel::new(|_| Ok(())),
            app.state(),
        )
        .await
        .unwrap();
        let terminal = adapter(state.clone());
        state.lifecycle.quit(terminal);
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            state.lifecycle.finished(),
        )
        .await
        .unwrap();
        assert_eq!(state.lifecycle.status().phase, lifecycle::Phase::Stopped);
    });
}

#[test]
fn in_flight_ui_event_send_is_joined_but_idle_stream_does_not_deadlock_cleanup() {
    tauri::async_runtime::block_on(async {
        let (_root, core, host) = fixture().await;
        let state = DesktopState::new(core, host);
        let app = tauri::test::mock_builder()
            .manage(state.clone())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let key = state.packages.snapshot().await.instances[0].key.clone();
        let mount = package_open_mount(key.into(), app.state()).await.unwrap();
        let (entered, received) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let entered = StdMutex::new(Some(entered));
        let released = StdMutex::new(released);
        let channel = Channel::new(move |_| {
            if let Some(entered) = entered.lock().unwrap().take() {
                entered.send(()).unwrap();
                released.lock().unwrap().recv().unwrap();
            }
            Ok(())
        });
        package_open_stream(
            mount.mount,
            "terminal-stream".into(),
            "fixture.saturation".into(),
            PackageValue::Object(Default::default()),
            channel,
            app.state(),
        )
        .await
        .unwrap();
        received.await.unwrap();
        let terminal = adapter(state.clone());
        state
            .lifecycle
            .apply("fixture", || Ok(Box::new(|| Ok(()))), terminal.clone())
            .unwrap();
        assert!(!terminal.installed.load(Ordering::SeqCst));
        release.send(()).unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            state.lifecycle.finished(),
        )
        .await
        .unwrap();
        assert!(terminal.restarted.load(Ordering::SeqCst));
    });
}
