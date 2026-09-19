#![forbid(unsafe_code)]

mod commands;
mod data_dir;
mod dto;
mod error;
mod package_commands;
mod package_dto;
mod package_error;
mod package_protocol;

use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{Manager, RunEvent};
use teamarness_core::{CoreError, CoreRuntime, CoreRuntimeConfig, StorageError};
use teamarness_package_host::{
    InstallationPublicIdentity, PackageHost, PackageHostCommand, PackageHostConfig,
    PackageInstanceKey, ShutdownHost, UiMountId,
};
use tokio::sync::Mutex;

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
    packages: Arc<PackageHost>,
    protocol_mounts: Arc<Mutex<BTreeMap<String, ProtocolMountLease>>>,
    lifecycle: Arc<DispatchLifecycle>,
}

#[derive(Clone)]
struct ProtocolMountLease {
    mount: UiMountId,
    instance: PackageInstanceKey,
}

impl DesktopState {
    fn new(runtime: Arc<CoreRuntime>, packages: Arc<PackageHost>) -> Self {
        Self {
            runtime,
            packages,
            protocol_mounts: Arc::new(Mutex::new(BTreeMap::new())),
            lifecycle: Arc::new(DispatchLifecycle::running()),
        }
    }

    fn runtime(&self) -> Result<Arc<CoreRuntime>, BridgeError> {
        if !self.lifecycle.accepting.load(Ordering::SeqCst) {
            return Err(BridgeError::shutdown());
        }
        Ok(Arc::clone(&self.runtime))
    }

    #[allow(clippy::result_large_err)]
    fn package_host(&self) -> Result<Arc<PackageHost>, package_error::PackageBridgeError> {
        if !self.lifecycle.accepting.load(Ordering::SeqCst) {
            return Err(package_error::invalid_input(
                teamarness_package_host::PackageErrorCode::ShuttingDown,
                teamarness_package_host::PackageErrorStage::Shutdown,
            ));
        }
        Ok(Arc::clone(&self.packages))
    }

    async fn register_protocol_mount(
        &self,
        token: &str,
        mount: &UiMountId,
        instance: &PackageInstanceKey,
    ) {
        self.protocol_mounts.lock().await.insert(
            token.to_owned(),
            ProtocolMountLease {
                mount: mount.clone(),
                instance: instance.clone(),
            },
        );
    }

    async fn mount_for_token(&self, token: &str) -> Option<UiMountId> {
        self.protocol_mounts
            .lock()
            .await
            .get(token)
            .map(|lease| lease.mount.clone())
    }

    async fn remove_mount_token(&self, mount: &UiMountId) {
        self.protocol_mounts
            .lock()
            .await
            .retain(|_, lease| lease.mount != *mount);
    }

    async fn remove_instance_tokens(&self, instance: &PackageInstanceKey) {
        self.protocol_mounts
            .lock()
            .await
            .retain(|_, lease| lease.instance != *instance);
    }

    async fn shutdown(&self) {
        await_shutdown_handoff(
            &self.lifecycle,
            async { self.protocol_mounts.lock().await.clear() },
            async {
                let _ = self
                    .packages
                    .execute(PackageHostCommand::Shutdown(ShutdownHost))
                    .await;
            },
            async {
                let _ = self.runtime.shutdown().await;
            },
        )
        .await;
    }
}

async fn await_shutdown_handoff<M, P, C>(
    lifecycle: &DispatchLifecycle,
    close_mounts: M,
    shutdown_packages: P,
    shutdown_core: C,
) where
    M: Future<Output = ()>,
    P: Future<Output = ()>,
    C: Future<Output = ()>,
{
    lifecycle.begin_shutdown();
    close_mounts.await;
    shutdown_packages.await;
    shutdown_core.await;
    lifecycle.complete_shutdown();
}

async fn open_runtime_modules(
    data_directory: PathBuf,
    application_root: PathBuf,
    application_index: PathBuf,
) -> Result<(Arc<CoreRuntime>, Arc<PackageHost>), &'static str> {
    let runtime = Arc::new(
        CoreRuntime::open(CoreRuntimeConfig::new(data_directory.clone()))
            .await
            .map_err(|error| match error {
                CoreError::Storage(StorageError::LegacyProtectionState) => {
                    "This data directory was created by an older development build whose key grants are no longer supported. Delete it to start fresh."
                }
                CoreError::Storage(StorageError::InvalidDataDirectory) => {
                    "The data directory is not a private Teamarness directory (symlink, other owner, or unrelated contents)"
                }
                _ => "Core Runtime startup failed",
            })?,
    );
    let installation = InstallationPublicIdentity::new(runtime.public_identity().to_string())
        .map_err(|_| "Installation identity is incompatible with Package Host")?;
    let packages = match PackageHost::open(
        PackageHostConfig::new(
            application_root,
            application_index,
            data_directory,
            installation,
        ),
        Arc::clone(&runtime),
    )
    .await
    {
        Ok(packages) => Arc::new(packages),
        Err(_) => {
            let _ = runtime.shutdown().await;
            return Err("Package Host startup failed");
        }
    };
    Ok((runtime, packages))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    debug_assert_eq!(
        commands::COMMAND_NAMES.len() + package_commands::COMMAND_NAMES.len(),
        19
    );
    let app = tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol(
            "teamarness-package",
            |context, request, responder| {
                let state = context.app_handle().state::<DesktopState>().inner().clone();
                tauri::async_runtime::spawn(async move {
                    responder.respond(package_protocol::serve(state, request).await);
                });
            },
        )
        .invoke_handler(tauri::generate_handler![
            commands::installation_details,
            commands::list_workspaces,
            commands::create_workspace,
            commands::create_workspace_invitation,
            commands::join_workspace,
            commands::current_membership,
            package_commands::package_catalog_snapshot,
            package_commands::package_subscribe_host_state,
            package_commands::package_open_mount,
            package_commands::package_close_mount,
            package_commands::package_request,
            package_commands::package_cancel_request,
            package_commands::package_open_stream,
            package_commands::package_acknowledge_events,
            package_commands::package_close_stream,
            package_commands::package_suspend,
            package_commands::package_recover,
            package_commands::package_dispose,
            package_commands::package_report_ui_fault,
        ])
        .setup(|app| {
            let ordinary_directory = app.path().app_data_dir()?;
            let selected =
                data_dir::resolve_data_directory(std::env::args_os(), &ordinary_directory)
                    .map_err(|_| "invalid --data-dir option")?;
            let application_root = app.path().resource_dir()?;
            let application_index = application_root.join("packages/application-packages.v1.json");
            let (runtime, packages) = tauri::async_runtime::block_on(open_runtime_modules(
                selected,
                application_root,
                application_index,
            ))?;
            app.manage(DesktopState::new(runtime, packages));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build Teamarness desktop application");

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
        for name in commands::COMMAND_NAMES
            .iter()
            .chain(package_commands::COMMAND_NAMES.iter())
        {
            assert!(
                source.contains(&format!("commands::{name}"))
                    || source.contains(&format!("package_commands::{name}"))
            );
            assert!(build.contains(&format!("\"{name}\"")));
            assert!(capability.contains(&format!("allow-{}", name.replace('_', "-"))));
        }
    }

    /// Verifies: TEAM.SYS.SHELL-R05
    #[test]
    fn shell_capability_targets_the_main_webview_not_windows() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
        assert_eq!(capability["webviews"], serde_json::json!(["main"]));
        assert!(
            capability.get("windows").is_none(),
            "a window-scoped capability also reaches child webviews inside that window"
        );
        let permissions = capability["permissions"].as_array().unwrap();
        assert!(permissions.iter().any(|value| value == "core:default"));
        assert_eq!(
            permissions
                .iter()
                .filter(|value| value
                    .as_str()
                    .is_some_and(|name| name.starts_with("allow-")))
                .count(),
            19
        );
    }

    /// Verifies: TEAM.SYS-R01, TEAM.SYS.SHELL-R05
    #[test]
    fn bridge_and_registry_source_deny_internal_and_product_concepts() {
        let reviewed = [
            include_str!("commands.rs")
                .split("#[cfg(test)]")
                .next()
                .unwrap(),
            include_str!("../../src/bridge.ts"),
            include_str!("../../src/contracts.ts"),
            include_str!("package_commands.rs")
                .split("#[cfg(test)]")
                .next()
                .unwrap(),
            include_str!("package_dto.rs")
                .split("#[cfg(test)]")
                .next()
                .unwrap(),
            include_str!("../../src/package-bridge.ts"),
            include_str!("../../src/package-contracts.ts"),
        ]
        .join("\n")
        .to_lowercase();
        for forbidden in [
            "package_manifest",
            "reference.read",
            "reference.changes",
            "conversation",
            "counter",
            "message_command",
            "repository_command",
            "source_path",
            "sourcepath",
            "cache_path",
            "cachepath",
            "data_path",
            "datapath",
            "process_handle",
            "processhandle",
            "transport_handle",
            "transporthandle",
            "core_runtime",
            "canonical_bytes",
            "endpoint_ticket",
            "secret_key",
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
    fn bundle_configuration_uses_all_generated_application_packages() {
        // packages/bundled-packages.json is the one bundled Package list; the Tauri
        // bundle configuration and the application index are derived from it.
        let bundled: serde_json::Value =
            serde_json::from_str(include_str!("../../../../packages/bundled-packages.json"))
                .unwrap();
        let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let mut expected_resources = serde_json::Map::new();
        expected_resources.insert(
            "../../../packages/application-packages.v1.json".into(),
            "packages/application-packages.v1.json".into(),
        );
        let mut expected_external_bin = Vec::new();
        let mut expected_index = Vec::new();
        let entries = bundled["packages"].as_array().unwrap();
        assert!(!entries.is_empty());
        for entry in entries {
            let source = entry["sourceDirectory"].as_str().unwrap();
            let cargo_package = entry["cargoPackage"].as_str().unwrap();
            let manifest_path = repository.join(source).join("teamarness.package.json");
            let manifest: serde_json::Value = serde_json::from_slice(
                &std::fs::read(&manifest_path)
                    .unwrap_or_else(|error| panic!("bundled Package manifest {source}: {error}")),
            )
            .unwrap();
            let identity = manifest["identity"].as_str().unwrap();
            let staged = format!("../../../target/teamarness-packages/{identity}");
            expected_resources.insert(format!("{staged}/"), format!("packages/{identity}/").into());
            expected_external_bin.push(serde_json::Value::String(format!(
                "{staged}/backend/{cargo_package}"
            )));
            expected_index.push(serde_json::json!({ "directory": format!("packages/{identity}") }));
        }

        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(
            config["bundle"]["resources"].as_object().unwrap(),
            &expected_resources,
            "run `node scripts/bundled-packages.mjs --write`"
        );
        assert_eq!(
            config["bundle"]["externalBin"].as_array().unwrap(),
            &expected_external_bin,
            "run `node scripts/bundled-packages.mjs --write`"
        );
        let index: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../packages/application-packages.v1.json"
        ))
        .unwrap();
        assert_eq!(
            index["packages"],
            serde_json::Value::Array(expected_index),
            "run `npm run build:application-packages`"
        );
    }

    #[test]
    fn exit_handoff_closes_dispatch_then_mounts_host_and_core() {
        let lifecycle = DispatchLifecycle::running();
        let order = std::sync::Mutex::new(Vec::new());
        tauri::async_runtime::block_on(await_shutdown_handoff(
            &lifecycle,
            async {
                assert!(!lifecycle.accepting.load(Ordering::SeqCst));
                order.lock().unwrap().push("mounts");
            },
            async { order.lock().unwrap().push("host") },
            async { order.lock().unwrap().push("core") },
        ));
        assert_eq!(*order.lock().unwrap(), ["mounts", "host", "core"]);
        assert!(lifecycle.shutdown_complete.load(Ordering::SeqCst));
    }
}
