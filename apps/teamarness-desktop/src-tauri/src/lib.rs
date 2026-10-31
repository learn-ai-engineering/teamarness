#![forbid(unsafe_code)]

mod commands;
mod content_protocol;
mod data_dir;
mod dto;
mod error;
mod lifecycle;
mod package_commands;
mod package_dto;
mod package_error;
mod package_protocol;
mod update;
mod update_commands;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use tauri::{Manager, RunEvent};
use teamarness_core::{CoreError, CoreRuntime, CoreRuntimeConfig, StorageError};
use teamarness_package_host::{
    InstallationPublicIdentity, PackageHost, PackageHostCommand, PackageHostConfig,
    PackageInstanceKey, ShutdownHost, UiMountId,
};
use tokio::sync::Mutex;

use crate::error::BridgeError;

#[derive(Clone)]
pub struct DesktopState {
    runtime: Arc<CoreRuntime>,
    packages: Arc<PackageHost>,
    protocol_mounts: Arc<Mutex<BTreeMap<String, ProtocolMountLease>>>,
    lifecycle: lifecycle::Lifecycle,
    updates: Option<update::UpdateService>,
}

#[derive(Clone)]
struct ProtocolMountLease {
    mount: UiMountId,
    instance: PackageInstanceKey,
    /// The mount's Package declares `teamarness.content-views`, so its frame
    /// policy admits `teamarness-content:` frames.
    nested_content: bool,
}

impl DesktopState {
    fn new(runtime: Arc<CoreRuntime>, packages: Arc<PackageHost>) -> Self {
        Self {
            runtime,
            packages,
            protocol_mounts: Arc::new(Mutex::new(BTreeMap::new())),
            lifecycle: lifecycle::Lifecycle::new(),
            updates: None,
        }
    }

    // Module accessors are private to admitted dispatch/resource work; cloning
    // these Arcs is not admission and must never stand in for an operation lease.
    fn runtime(&self) -> Arc<CoreRuntime> {
        Arc::clone(&self.runtime)
    }
    fn package_host(&self) -> Arc<PackageHost> {
        Arc::clone(&self.packages)
    }

    async fn register_protocol_mount(
        &self,
        token: &str,
        mount: &UiMountId,
        instance: &PackageInstanceKey,
        nested_content: bool,
    ) {
        self.protocol_mounts.lock().await.insert(
            token.to_owned(),
            ProtocolMountLease {
                mount: mount.clone(),
                instance: instance.clone(),
                nested_content,
            },
        );
    }

    /// The mount a resource token belongs to, and whether it may nest content frames.
    async fn mount_for_token(&self, token: &str) -> Option<(UiMountId, bool)> {
        self.protocol_mounts
            .lock()
            .await
            .get(token)
            .map(|lease| (lease.mount.clone(), lease.nested_content))
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

    async fn cleanup(&self) -> lifecycle::ShutdownErrors {
        shutdown_handoff(
            async {
                self.protocol_mounts.lock().await.clear();
            },
            async {
                let result = self
                    .packages
                    .execute(PackageHostCommand::Shutdown(ShutdownHost))
                    .await;
                let uncertain = result.as_ref().is_err_and(|error| {
                    error.code == teamarness_package_host::PackageErrorCode::OutcomeUncertain
                });
                (result.is_err(), uncertain)
            },
            async { self.runtime.shutdown().await.is_err() },
        )
        .await
    }

    async fn dispatch<F, T>(&self, operation: F) -> Result<T, BridgeError>
    where
        F: std::future::Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        self.lifecycle
            .dispatch(operation)
            .await
            .map_err(|()| BridgeError::shutdown())
    }
}

struct DesktopTerminal {
    state: DesktopState,
    handle: tauri::AppHandle,
}
impl lifecycle::TerminalAdapter for DesktopTerminal {
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
    fn install(&self, pending: lifecycle::Install) -> Result<(), lifecycle::InstallFailure> {
        if update::probe_enabled() {
            eprintln!("application-update-probe terminal=install-blocked");
            return Err(lifecycle::InstallFailure { previous_app: None });
        }
        pending()
    }
    fn restart(&self) {
        if update::probe_enabled() {
            eprintln!("application-update-probe terminal=restart-blocked");
            return;
        }
        self.handle.restart();
    }
    fn exit(&self) {
        self.handle.exit(0);
    }
}

async fn shutdown_handoff(
    mounts: impl std::future::Future<Output = ()>,
    host: impl std::future::Future<Output = (bool, bool)>,
    core: impl std::future::Future<Output = bool>,
) -> lifecycle::ShutdownErrors {
    mounts.await;
    let (host, uncertain) = host.await;
    let core = core.await;
    lifecycle::ShutdownErrors {
        host,
        core,
        uncertain,
    }
}

async fn open_runtime_modules(
    data_directory: PathBuf,
    application_root: PathBuf,
    application_index: PathBuf,
) -> Result<(Arc<CoreRuntime>, Arc<PackageHost>), &'static str> {
    let runtime = Arc::new(
        CoreRuntime::open(CoreRuntimeConfig::new(data_directory.clone()))
            .await
            .map_err(|error| {
                // The user-facing text stays deliberately vague; the cause is
                // written once to stderr so a developer can see what failed.
                // Every Core error's `Display` is already redacted.
                eprintln!("teamarness-desktop startup failed stage=core error={error}");
                error
            })
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
        Err(error) => {
            eprintln!("teamarness-desktop startup failed stage=host error={error}");
            let _ = runtime.shutdown().await;
            return Err("Package Host startup failed");
        }
    };
    Ok((runtime, packages))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    debug_assert_eq!(
        commands::COMMAND_NAMES.len()
            + package_commands::COMMAND_NAMES.len()
            + update_commands::COMMAND_NAMES.len(),
        37
    );
    let app = update::configure(tauri::Builder::default())
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol(
            "teamarness-package",
            |context, request, responder| {
                let state = context.app_handle().state::<DesktopState>().inner().clone();
                tauri::async_runtime::spawn(async move {
                    package_protocol::respond(state, request, move |response| {
                        responder.respond(response)
                    })
                    .await;
                });
            },
        )
        .register_asynchronous_uri_scheme_protocol(
            "teamarness-content",
            |context, request, responder| {
                let state = context.app_handle().state::<DesktopState>().inner().clone();
                tauri::async_runtime::spawn(async move {
                    content_protocol::respond(state, request, move |response| {
                        responder.respond(response)
                    })
                    .await;
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
            commands::principals_snapshot,
            commands::subscribe_principals,
            commands::rename_workspace_principal,
            commands::add_workspace_agent,
            commands::remove_workspace_agent,
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
            update_commands::application_update_status,
            update_commands::application_update_check,
            update_commands::application_update_restart,
            update_commands::application_update_quit,
            package_commands::package_choose_directory,
            package_commands::package_inspect_directory,
            package_commands::package_install,
            package_commands::package_reload,
            package_commands::package_remove,
            package_commands::package_write_override,
            package_commands::package_authoring_catalog,
            package_commands::package_restart,
            package_commands::app_restart,
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
            let mut state = DesktopState::new(runtime, packages);
            state.updates = Some(update::UpdateService::start(app.handle()));
            app.manage(state);
            update_commands::start_events(app.handle());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build Teamarness desktop application");

    app.run(|handle, event| {
        let state = handle.state::<DesktopState>().inner().clone();
        match event {
            RunEvent::ExitRequested { api, .. } => {
                if state.lifecycle.exit_allowed() {
                    return;
                }
                api.prevent_exit();
            }
            RunEvent::WindowEvent {
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } => {
                if state.lifecycle.exit_allowed() {
                    return;
                }
                api.prevent_close();
            }
            _ => return,
        }
        state.lifecycle.quit(Arc::new(DesktopTerminal {
            state: state.clone(),
            handle: handle.clone(),
        }));
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
            .chain(update_commands::COMMAND_NAMES.iter())
        {
            assert!(
                source.contains(&format!("commands::{name}"))
                    || source.contains(&format!("package_commands::{name}"))
                    || source.contains(&format!("update_commands::{name}"))
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
            37
        );
    }

    /// Verifies: TEAM.SYS.SHELL-R05
    #[test]
    fn package_management_permissions_are_granted_only_to_the_main_shell() {
        let management = [
            "allow-package-choose-directory",
            "allow-package-inspect-directory",
            "allow-package-install",
            "allow-package-reload",
            "allow-package-remove",
            "allow-package-write-override",
            "allow-package-authoring-catalog",
            "allow-package-restart",
            "allow-app-restart",
        ];
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities");
        let mut main_shell = 0;
        for entry in std::fs::read_dir(directory).unwrap() {
            let capability: serde_json::Value =
                serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap();
            let permissions = capability["permissions"].as_array().unwrap();
            let granted = management
                .iter()
                .filter(|name| permissions.iter().any(|value| value == **name))
                .count();
            if capability["identifier"] == "main-shell" {
                main_shell += 1;
                assert_eq!(capability["webviews"], serde_json::json!(["main"]));
                assert_eq!(granted, management.len());
            } else {
                assert_eq!(granted, 0, "{}", capability["identifier"]);
            }
            // The folder picker runs in Rust; no webview gets the dialog plugin's own commands.
            assert!(!permissions.iter().any(|value| {
                value
                    .as_str()
                    .is_some_and(|name| name.starts_with("dialog:"))
            }));
        }
        assert_eq!(main_shell, 1);
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert!(config["app"]["security"].get("capabilities").is_none());
    }

    /// Verifies: TEAM.SYS-R01, TEAM.SYS.SHELL-R05
    #[test]
    fn bridge_and_registry_source_deny_internal_and_product_concepts() {
        let reviewed = [
            include_str!("commands.rs")
                .split("#[cfg(test)]")
                .next()
                .unwrap(),
            include_str!("dto.rs").split("#[cfg(test)]").next().unwrap(),
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
}
