#![forbid(unsafe_code)]

mod commands;
mod data_dir;
mod dto;
mod error;

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use resonance_core::{CoreRuntime, CoreRuntimeConfig};
use tauri::{Manager, RunEvent};

use crate::error::BridgeError;

#[derive(Default)]
struct DispatchLifecycle {
    accepting: AtomicBool,
    shutdown_started: AtomicBool,
    shutdown_complete: AtomicBool,
}

impl DispatchLifecycle {
    fn running() -> Self {
        Self {
            accepting: AtomicBool::new(true),
            shutdown_started: AtomicBool::new(false),
            shutdown_complete: AtomicBool::new(false),
        }
    }

    fn begin_shutdown(&self) -> bool {
        self.accepting.store(false, Ordering::SeqCst);
        !self.shutdown_started.swap(true, Ordering::SeqCst)
    }

    fn complete_shutdown(&self) {
        self.shutdown_complete.store(true, Ordering::SeqCst);
    }
}

#[derive(Clone)]
pub struct DesktopState {
    runtime: Arc<CoreRuntime>,
    lifecycle: Arc<DispatchLifecycle>,
}

impl DesktopState {
    fn new(runtime: CoreRuntime) -> Self {
        Self {
            runtime: Arc::new(runtime),
            lifecycle: Arc::new(DispatchLifecycle::running()),
        }
    }

    fn runtime(&self) -> Result<Arc<CoreRuntime>, BridgeError> {
        if !self.lifecycle.accepting.load(Ordering::SeqCst) {
            return Err(BridgeError::shutdown());
        }
        Ok(Arc::clone(&self.runtime))
    }

    async fn shutdown(&self) {
        await_shutdown_handoff(&self.lifecycle, async {
            let _ = self.runtime.shutdown().await;
        })
        .await;
    }
}

async fn await_shutdown_handoff<F>(lifecycle: &DispatchLifecycle, shutdown: F)
where
    F: Future<Output = ()>,
{
    lifecycle.begin_shutdown();
    shutdown.await;
    lifecycle.complete_shutdown();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    debug_assert_eq!(commands::COMMAND_NAMES.len(), 6);
    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::installation_details,
            commands::list_workspaces,
            commands::create_workspace,
            commands::create_workspace_invitation,
            commands::join_workspace,
            commands::current_membership,
        ])
        .setup(|app| {
            let ordinary_directory = app.path().app_data_dir()?;
            let selected =
                data_dir::resolve_data_directory(std::env::args_os(), &ordinary_directory)
                    .map_err(|_| "invalid --resonance-data-dir option")?;
            let runtime = tauri::async_runtime::block_on(async move {
                tauri::async_runtime::spawn(async move {
                    CoreRuntime::open(CoreRuntimeConfig::new(selected)).await
                })
                .await
            })
            .map_err(|_| "Core Runtime startup task failed")?
            .map_err(|_| "Core Runtime startup failed")?;
            app.manage(DesktopState::new(runtime));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build Resonance desktop application");

    app.run(|handle, event| {
        if let RunEvent::ExitRequested { api, .. } = event {
            let state = handle.state::<DesktopState>().inner().clone();
            if state.lifecycle.shutdown_complete.load(Ordering::SeqCst) {
                return;
            }
            api.prevent_exit();
            if state.lifecycle.begin_shutdown() {
                let handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    state.shutdown().await;
                    handle.exit(0);
                });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_managed_state_bounds<T: Send + Sync + 'static>() {}

    #[test]
    fn managed_state_meets_tauri_bounds() {
        assert_managed_state_bounds::<DesktopState>();
    }

    #[test]
    fn source_registry_matches_reviewed_command_names() {
        let source = include_str!("lib.rs");
        let build = include_str!("../build.rs");
        let capability = include_str!("../capabilities/default.json");
        for name in commands::COMMAND_NAMES {
            assert!(source.contains(&format!("commands::{name}")));
            assert!(build.contains(&format!("\"{name}\"")));
            assert!(capability.contains(&format!("allow-{}", name.replace('_', "-"))));
        }
    }

    #[test]
    fn bridge_and_registry_source_deny_internal_and_product_concepts() {
        let reviewed = [
            include_str!("commands.rs")
                .split("#[cfg(test)]")
                .next()
                .unwrap(),
            include_str!("../../src/bridge.ts"),
            include_str!("../../src/contracts.ts"),
        ]
        .join("\n")
        .to_lowercase();
        for forbidden in [
            "package_manifest",
            "package_mount",
            "conversation",
            "message_command",
            "repository_command",
            "raw_storage_path",
            "canonical_bytes",
            "endpoint_ticket",
            "iroh::",
            "rusqlite",
        ] {
            assert!(
                !reviewed.contains(forbidden),
                "forbidden bridge concept: {forbidden}"
            );
        }
    }

    #[test]
    fn exit_handoff_closes_dispatch_before_awaiting_core_shutdown() {
        let lifecycle = DispatchLifecycle::running();
        tauri::async_runtime::block_on(await_shutdown_handoff(&lifecycle, async {
            assert!(!lifecycle.accepting.load(Ordering::SeqCst));
            assert!(lifecycle.shutdown_started.load(Ordering::SeqCst));
            assert!(!lifecycle.shutdown_complete.load(Ordering::SeqCst));
        }));
        assert!(lifecycle.shutdown_complete.load(Ordering::SeqCst));
    }
}
