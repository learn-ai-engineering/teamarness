//! Main-webview controls bypass ordinary operation admission, including after shutdown.
use crate::{DesktopState, DesktopTerminal, lifecycle, update};
use serde::Serialize;
use std::sync::Arc;
use tauri::{Emitter, Manager};

pub(crate) const COMMAND_NAMES: &[&str] = &[
    "application_update_status",
    "application_update_check",
    "application_update_restart",
    "application_update_quit",
];

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    revision: u64,
    update: update::Snapshot,
    lifecycle: lifecycle::Status,
}
fn snapshot(lifecycle: &lifecycle::Lifecycle, updates: &update::UpdateService) -> Snapshot {
    let update = updates.status();
    let lifecycle = lifecycle.status();
    // Both counters are monotonic for the Installation lifetime. A mixed read
    // cannot hide terminal admission: lifecycle is read last and UI latches it.
    Snapshot {
        revision: update.revision + lifecycle.revision,
        update,
        lifecycle,
    }
}
fn current(state: &DesktopState) -> Snapshot {
    snapshot(
        &state.lifecycle,
        state
            .updates
            .as_ref()
            .expect("updates initialized before dispatch"),
    )
}
fn recovery_quit_allowed(status: &lifecycle::Status) -> bool {
    status.phase == lifecycle::Phase::Failed
}

#[tauri::command]
pub(crate) fn application_update_status(state: tauri::State<'_, DesktopState>) -> Snapshot {
    probe_entry("status");
    current(&state)
}
#[tauri::command]
pub(crate) fn application_update_check(state: tauri::State<'_, DesktopState>) -> Snapshot {
    probe_entry("check");
    if state.lifecycle.status().phase == lifecycle::Phase::Running {
        state
            .updates
            .as_ref()
            .expect("updates initialized")
            .request_check();
    }
    current(&state)
}
#[tauri::command]
pub(crate) fn application_update_restart(
    state: tauri::State<'_, DesktopState>,
    handle: tauri::AppHandle,
    generation: String,
) -> Result<Snapshot, &'static str> {
    probe_entry("restart");
    request_restart(
        &state.lifecycle,
        state.updates.as_ref().expect("updates initialized"),
        &generation,
        Arc::new(DesktopTerminal {
            state: state.inner().clone(),
            handle,
        }),
    )?;
    Ok(current(&state))
}
#[tauri::command]
pub(crate) fn application_update_quit(
    state: tauri::State<'_, DesktopState>,
    handle: tauri::AppHandle,
) -> Result<(), &'static str> {
    probe_entry("quit");
    request_recovery_quit(
        &state.lifecycle,
        Arc::new(DesktopTerminal {
            state: state.inner().clone(),
            handle,
        }),
    )
}

fn request_restart(
    lifecycle: &lifecycle::Lifecycle,
    updates: &update::UpdateService,
    generation: &str,
    adapter: Arc<dyn lifecycle::TerminalAdapter>,
) -> Result<(), &'static str> {
    lifecycle
        .apply(
            generation,
            || {
                let pending = updates.admit_apply(generation).map_err(|_| ())?;
                Ok(update::installer(pending))
            },
            adapter,
        )
        .map_err(|()| "stale_candidate")
}
fn request_recovery_quit(
    lifecycle: &lifecycle::Lifecycle,
    adapter: Arc<dyn lifecycle::TerminalAdapter>,
) -> Result<(), &'static str> {
    if !recovery_quit_allowed(&lifecycle.status()) {
        return Err("recovery_only");
    }
    lifecycle.quit(adapter);
    Ok(())
}

pub(crate) fn start_events(handle: &tauri::AppHandle) {
    let state = handle.state::<DesktopState>().inner().clone();
    let mut updates = state
        .updates
        .as_ref()
        .expect("updates initialized")
        .subscribe();
    let mut lifecycle = state.lifecycle.subscribe();
    let handle = handle.clone();
    // Status forwarding intentionally survives ordinary dispatch closure. It
    // uses no Core/Host resources and is owned by the application runtime.
    tauri::async_runtime::spawn(async move {
        loop {
            let _ = handle.emit_to(
                tauri::EventTarget::webview("main"),
                "application-update",
                current(&state),
            );
            tokio::select! {
                result = updates.changed() => if result.is_err() { return; },
                result = lifecycle.changed() => if result.is_err() { return; },
            }
        }
    });
}

fn probe_entry(command: &str) {
    #[cfg(feature = "ui-isolation-probe")]
    eprintln!("application-update-probe handler={command}");
    #[cfg(not(feature = "ui-isolation-probe"))]
    let _ = command;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::{Lifecycle, Phase};

    /// Verifies: TEAM.SYS.SHELL-R07
    #[test]
    fn application_update_permissions_are_exact_main_only_and_no_plugin_authority() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
        assert_eq!(capability["webviews"], serde_json::json!(["main"]));
        assert!(capability.get("windows").is_none());
        let permissions = capability["permissions"].as_array().unwrap();
        for command in COMMAND_NAMES {
            assert!(permissions.contains(&serde_json::json!(format!(
                "allow-{}",
                command.replace('_', "-")
            ))));
        }
        assert!(
            !permissions
                .iter()
                .any(|value| value.as_str().unwrap().starts_with("updater:")
                    || value.as_str().unwrap().starts_with("process:"))
        );
        assert!(
            !include_str!("../../src/package-frame/protocol.ts").contains("application_update")
        );
    }

    /// Verifies: TEAM.SYS.SHELL-R07
    #[test]
    fn application_update_recovery_quit_is_denied_while_running() {
        assert!(!recovery_quit_allowed(&Lifecycle::new().status()));
    }

    /// Verifies: TEAM.SYS.SHELL-R07
    #[test]
    fn application_update_snapshot_has_no_archive_or_credentials_and_status_bypasses_admission() {
        let lifecycle = Lifecycle::new();
        let service = crate::update::UpdateService::disabled_for_commands();
        let snapshot = snapshot(&lifecycle, &service);
        let json = serde_json::to_value(snapshot).unwrap();
        assert_eq!(json["revision"], 0);
        assert_eq!(json["lifecycle"]["phase"], "running");
        assert_eq!(json["update"]["state"], "disabled");
        for forbidden in ["url", "signature", "bytes", "credentials"] {
            assert!(json["update"].get(forbidden).is_none());
        }
        assert_eq!(lifecycle.status().phase, Phase::Running);
    }
    #[derive(Default)]
    struct Terminal {
        installs: std::sync::atomic::AtomicUsize,
        exits: std::sync::atomic::AtomicUsize,
    }
    impl lifecycle::TerminalAdapter for Terminal {
        fn stop_updates(&self) {}
        fn cleanup(
            &self,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = lifecycle::ShutdownErrors> + Send + '_>,
        > {
            Box::pin(async { lifecycle::ShutdownErrors::default() })
        }
        fn install(&self, _pending: lifecycle::Install) -> Result<(), lifecycle::InstallFailure> {
            self.installs
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(lifecycle::InstallFailure { previous_app: None })
        }
        fn restart(&self) {
            panic!("no restart on failure");
        }
        fn exit(&self) {
            self.exits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Verifies: TEAM.SYS.SHELL-R07
    #[test]
    fn native_application_update_command_adapters_validate_generation_join_duplicates_and_allow_only_recovery_quit()
     {
        let updates = tauri::async_runtime::block_on(update::UpdateService::ready_for_commands());
        assert!(tokio::runtime::Handle::try_current().is_err());
        let lifecycle = Lifecycle::new();
        let terminal = Arc::new(Terminal::default());
        assert_eq!(
            request_recovery_quit(&lifecycle, terminal.clone()),
            Err("recovery_only")
        );
        assert_eq!(
            request_restart(&lifecycle, &updates, "fake", terminal.clone()),
            Err("stale_candidate")
        );
        assert_eq!(lifecycle.status().phase, Phase::Running);
        let generation = updates.status().pending_generation.unwrap();
        let before = snapshot(&lifecycle, &updates).revision;
        request_restart(&lifecycle, &updates, &generation, terminal.clone()).unwrap();
        request_restart(&lifecycle, &updates, &generation, terminal.clone()).unwrap();
        assert_eq!(
            request_restart(&lifecycle, &updates, "other", terminal.clone()),
            Err("stale_candidate")
        );
        assert!(lifecycle.admit().is_err());
        assert!(snapshot(&lifecycle, &updates).revision > before);
        tauri::async_runtime::block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(10), lifecycle.finished())
                .await
                .unwrap();
        });
        assert!(recovery_quit_allowed(&lifecycle.status()));
        assert_eq!(
            terminal.installs.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            request_restart(&lifecycle, &updates, &generation, terminal.clone()),
            Err("stale_candidate")
        );
        request_recovery_quit(&lifecycle, terminal.clone()).unwrap();
        assert_eq!(terminal.exits.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[cfg(feature = "ui-isolation-probe")]
    /// Verifies: TEAM.SYS.SHELL-R07
    #[test]
    fn application_update_probe_real_fixture_is_ready_without_transport_and_never_production_selectable()
     {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.probe.conf.json")).unwrap();
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.package_info_mut().version = semver::Version::new(0, 1, 0);
        context
            .config_mut()
            .plugins
            .0
            .insert("updater".into(), config["plugins"]["updater"].clone());
        let app = tauri::test::mock_builder()
            .plugin(tauri_plugin_updater::Builder::new().build())
            .build(context)
            .unwrap();
        let service = update::UpdateService::probe(app.handle());
        assert_eq!(service.status().state, update::State::Ready);
        assert!(!service.request_check());
        assert!(service.admit_apply("fake").is_err());
        assert!(service.admit_apply("1").is_ok());
        assert!(service.admit_apply("1").is_err());
        assert!(include_str!("../build.rs").contains(
            "UI isolation probe instrumentation cannot be selected in a production release"
        ));
    }
}
