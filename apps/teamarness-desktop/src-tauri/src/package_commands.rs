#![allow(clippy::result_large_err)]

use std::str::FromStr;

use tauri::{State, ipc::Channel};
use teamarness_package_host::{
    AcknowledgeUiEvents, CancelUiRequest, CloseUiMount, CloseUiSubscription, DisposePackage,
    HostRevision, OpenUiMount, OpenUiSubscription, PackageErrorCode, PackageErrorStage,
    PackageHostCommand, PackageHostNotification, PackageHostOutcome,
    PackageHostSubscriptionSelector, PackageIdentity, PackageInstanceKey, PackageValue,
    RecoverPackage, ReportUiFault, SuspendPackage, UiMountId, UiRequest, UiRequestId,
    UiSubscriptionId, WorkspaceId,
};

use crate::DesktopState;
use crate::package_dto::{
    HostStateNotificationDto, OpenUiMountDto, PackageHostSnapshotDto, PackageInstanceKeyDto,
    UiEventDto,
};
use crate::package_error::{PackageBridgeError, invalid_input};

pub const COMMAND_NAMES: [&str; 13] = [
    "package_catalog_snapshot",
    "package_subscribe_host_state",
    "package_open_mount",
    "package_close_mount",
    "package_request",
    "package_cancel_request",
    "package_open_stream",
    "package_acknowledge_events",
    "package_close_stream",
    "package_suspend",
    "package_recover",
    "package_dispose",
    "package_report_ui_fault",
];

#[tauri::command]
pub async fn package_catalog_snapshot(
    state: State<'_, DesktopState>,
) -> Result<PackageHostSnapshotDto, PackageBridgeError> {
    Ok(state.package_host()?.snapshot().await.into())
}

#[tauri::command]
pub async fn package_subscribe_host_state(
    after_revision: u64,
    on_event: Channel<HostStateNotificationDto>,
    state: State<'_, DesktopState>,
) -> Result<(), PackageBridgeError> {
    let host = state.package_host()?;
    let mut subscription = host
        .subscribe(PackageHostSubscriptionSelector::HostState {
            after_revision: HostRevision(after_revision),
        })
        .await
        .map_err(PackageBridgeError::from)?;
    tauri::async_runtime::spawn(async move {
        while let Ok(PackageHostNotification::HostState(value)) = subscription.changed().await {
            if on_event.send(value.into()).is_err() {
                break;
            }
        }
    });
    Ok(())
}

#[tauri::command]
pub async fn package_open_mount(
    instance: PackageInstanceKeyDto,
    state: State<'_, DesktopState>,
) -> Result<OpenUiMountDto, PackageBridgeError> {
    let instance = parse_instance(instance)?;
    state.remove_instance_tokens(&instance).await;
    let outcome = state
        .package_host()?
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount {
            instance: instance.clone(),
        }))
        .await
        .map_err(PackageBridgeError::from)?;
    let PackageHostOutcome::UiMountOpened { mount, loader, .. } = outcome else {
        unreachable!("OpenUiMount has one success outcome")
    };
    state
        .register_protocol_mount(loader.token.as_str(), &mount, &instance)
        .await;
    Ok(OpenUiMountDto {
        mount: mount.into(),
        loader: loader.into(),
    })
}

#[tauri::command]
pub async fn package_close_mount(
    mount: String,
    state: State<'_, DesktopState>,
) -> Result<(), PackageBridgeError> {
    let mount = parse_mount(mount)?;
    state.remove_mount_token(&mount).await;
    state
        .package_host()?
        .execute(PackageHostCommand::CloseUiMount(CloseUiMount { mount }))
        .await
        .map_err(PackageBridgeError::from)?;
    Ok(())
}

#[tauri::command]
pub async fn package_request(
    mount: String,
    request: String,
    operation: String,
    payload: PackageValue,
    state: State<'_, DesktopState>,
) -> Result<PackageValue, PackageBridgeError> {
    let outcome = state
        .package_host()?
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: parse_mount(mount)?,
            request: UiRequestId::new(request).map_err(|_| ui_input_error())?,
            operation,
            payload,
        }))
        .await
        .map_err(PackageBridgeError::from)?;
    let PackageHostOutcome::UiResponse { payload } = outcome else {
        unreachable!("UiRequest has one success outcome")
    };
    Ok(payload)
}

#[tauri::command]
pub async fn package_cancel_request(
    mount: String,
    request: String,
    state: State<'_, DesktopState>,
) -> Result<(), PackageBridgeError> {
    state
        .package_host()?
        .execute(PackageHostCommand::CancelUiRequest(CancelUiRequest {
            mount: parse_mount(mount)?,
            request: UiRequestId::new(request).map_err(|_| ui_input_error())?,
        }))
        .await
        .map_err(PackageBridgeError::from)?;
    Ok(())
}

#[tauri::command]
pub async fn package_open_stream(
    mount: String,
    subscription: String,
    operation: String,
    payload: PackageValue,
    on_event: Channel<UiEventDto>,
    state: State<'_, DesktopState>,
) -> Result<(), PackageBridgeError> {
    let mount = parse_mount(mount)?;
    let subscription = UiSubscriptionId::new(subscription).map_err(|_| ui_input_error())?;
    let host = state.package_host()?;
    host.execute(PackageHostCommand::OpenUiSubscription(OpenUiSubscription {
        mount: mount.clone(),
        subscription: subscription.clone(),
        operation,
        payload,
    }))
    .await
    .map_err(PackageBridgeError::from)?;
    let receiver = host
        .subscribe(PackageHostSubscriptionSelector::UiEvents {
            mount: mount.clone(),
            subscription: subscription.clone(),
        })
        .await
        .map_err(PackageBridgeError::from)?;
    tauri::async_runtime::spawn(forward_ui_events(
        host,
        receiver,
        mount,
        subscription,
        on_event,
    ));
    Ok(())
}

#[tauri::command]
pub async fn package_acknowledge_events(
    mount: String,
    subscription: String,
    through_sequence: String,
    state: State<'_, DesktopState>,
) -> Result<(), PackageBridgeError> {
    let through_sequence = through_sequence.parse().map_err(|_| ui_input_error())?;
    state
        .package_host()?
        .execute(PackageHostCommand::AcknowledgeUiEvents(
            AcknowledgeUiEvents {
                mount: parse_mount(mount)?,
                subscription: UiSubscriptionId::new(subscription).map_err(|_| ui_input_error())?,
                through_sequence,
            },
        ))
        .await
        .map_err(PackageBridgeError::from)?;
    Ok(())
}

#[tauri::command]
pub async fn package_close_stream(
    mount: String,
    subscription: String,
    state: State<'_, DesktopState>,
) -> Result<(), PackageBridgeError> {
    state
        .package_host()?
        .execute(PackageHostCommand::CloseUiSubscription(
            CloseUiSubscription {
                mount: parse_mount(mount)?,
                subscription: UiSubscriptionId::new(subscription).map_err(|_| ui_input_error())?,
            },
        ))
        .await
        .map_err(PackageBridgeError::from)?;
    Ok(())
}

#[tauri::command]
pub async fn package_suspend(
    instance: PackageInstanceKeyDto,
    state: State<'_, DesktopState>,
) -> Result<(), PackageBridgeError> {
    let instance = parse_instance(instance)?;
    state.remove_instance_tokens(&instance).await;
    state
        .package_host()?
        .execute(PackageHostCommand::Suspend(SuspendPackage { instance }))
        .await
        .map_err(PackageBridgeError::from)?;
    Ok(())
}

#[tauri::command]
pub async fn package_recover(
    instance: PackageInstanceKeyDto,
    state: State<'_, DesktopState>,
) -> Result<(), PackageBridgeError> {
    let instance = parse_instance(instance)?;
    state.remove_instance_tokens(&instance).await;
    state
        .package_host()?
        .execute(PackageHostCommand::Recover(RecoverPackage { instance }))
        .await
        .map_err(PackageBridgeError::from)?;
    Ok(())
}

#[tauri::command]
pub async fn package_dispose(
    instance: PackageInstanceKeyDto,
    state: State<'_, DesktopState>,
) -> Result<(), PackageBridgeError> {
    let instance = parse_instance(instance)?;
    state.remove_instance_tokens(&instance).await;
    state
        .package_host()?
        .execute(PackageHostCommand::Dispose(DisposePackage { instance }))
        .await
        .map_err(PackageBridgeError::from)?;
    Ok(())
}

#[tauri::command]
pub async fn package_report_ui_fault(
    mount: String,
    code: String,
    state: State<'_, DesktopState>,
) -> Result<(), PackageBridgeError> {
    if code != "ui_exception" {
        return Err(ui_input_error());
    }
    let mount = parse_mount(mount)?;
    state.remove_mount_token(&mount).await;
    state
        .package_host()?
        .execute(PackageHostCommand::ReportUiFault(ReportUiFault {
            mount,
            code,
        }))
        .await
        .map_err(PackageBridgeError::from)?;
    Ok(())
}

async fn forward_ui_events(
    host: std::sync::Arc<teamarness_package_host::PackageHost>,
    mut receiver: teamarness_package_host::PackageHostSubscription,
    mount: UiMountId,
    subscription: UiSubscriptionId,
    on_event: Channel<UiEventDto>,
) {
    while let Ok(PackageHostNotification::UiEvent(value)) = receiver.changed().await {
        if on_event.send(value.into()).is_err() {
            let _ = host
                .execute(PackageHostCommand::CloseUiSubscription(
                    CloseUiSubscription {
                        mount: mount.clone(),
                        subscription: subscription.clone(),
                    },
                ))
                .await;
            break;
        }
    }
}

fn parse_instance(value: PackageInstanceKeyDto) -> Result<PackageInstanceKey, PackageBridgeError> {
    let package_identity = PackageIdentity::from_str(&value.package_identity).map_err(|_| {
        invalid_input(
            PackageErrorCode::InvalidPackageIdentity,
            PackageErrorStage::Ui,
        )
    })?;
    let workspace_id = WorkspaceId::new(value.workspace_id).map_err(|_| ui_input_error())?;
    Ok(PackageInstanceKey::new(package_identity, workspace_id))
}

fn parse_mount(value: String) -> Result<UiMountId, PackageBridgeError> {
    UiMountId::new(value).map_err(|_| ui_input_error())
}

fn ui_input_error() -> PackageBridgeError {
    invalid_input(PackageErrorCode::UiContractViolation, PackageErrorStage::Ui)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::Arc;

    use sha2::{Digest, Sha256};
    use teamarness_core::{CoreRuntime, CoreRuntimeConfig};
    use teamarness_package_host::{
        InstallationPublicIdentity, PackageHost, PackageHostConfig, ShutdownHost,
    };

    #[test]
    fn command_registry_is_generic_and_exact() {
        assert_eq!(COMMAND_NAMES.len(), 13);
        let names = COMMAND_NAMES.join(" ");
        for forbidden in [
            "reference",
            "conversation",
            "counter",
            "message",
            "repository",
        ] {
            assert!(!names.contains(forbidden));
        }
    }

    #[test]
    fn sequences_cross_the_desktop_boundary_as_decimal_strings() {
        assert!("18446744073709551615".parse::<u64>().is_ok());
        assert!("1.0".parse::<u64>().is_err());
    }

    #[test]
    fn channel_failure_closes_the_host_stream_without_acknowledging() {
        tauri::async_runtime::block_on(async {
            let (root, core, host) = fixture().await;
            let initial = host.snapshot().await;
            let instance = initial.instances[0].key.clone();
            let generation = initial.instances[0].process_generation;
            let opened = host
                .execute(PackageHostCommand::OpenUiMount(OpenUiMount {
                    instance: instance.clone(),
                }))
                .await
                .unwrap();
            let PackageHostOutcome::UiMountOpened { mount, .. } = opened else {
                unreachable!()
            };
            let subscription = UiSubscriptionId::new("desktop-stream").unwrap();
            host.execute(PackageHostCommand::OpenUiSubscription(OpenUiSubscription {
                mount: mount.clone(),
                subscription: subscription.clone(),
                operation: "fixture.saturation".into(),
                payload: PackageValue::Object(Default::default()),
            }))
            .await
            .unwrap();
            let receiver = host
                .subscribe(PackageHostSubscriptionSelector::UiEvents {
                    mount: mount.clone(),
                    subscription: subscription.clone(),
                })
                .await
                .unwrap();
            let failed_channel = Channel::new(|_| Err(tauri::Error::FailedToReceiveMessage));
            forward_ui_events(
                Arc::clone(&host),
                receiver,
                mount.clone(),
                subscription.clone(),
                failed_channel,
            )
            .await;
            assert!(
                host.execute(PackageHostCommand::AcknowledgeUiEvents(
                    AcknowledgeUiEvents {
                        mount: mount.clone(),
                        subscription: subscription.clone(),
                        through_sequence: 1,
                    },
                ))
                .await
                .is_err()
            );
            host.execute(PackageHostCommand::CloseUiMount(CloseUiMount { mount }))
                .await
                .unwrap();
            let after = host.snapshot().await;
            assert_eq!(after.instances[0].process_generation, generation);
            assert_eq!(after.instances[0].lifecycle.as_str(), "active");
            host.execute(PackageHostCommand::Shutdown(ShutdownHost))
                .await
                .unwrap();
            core.shutdown().await.unwrap();
            drop(root);
        });
    }

    #[test]
    fn reconciliation_adds_new_workspaces_without_restarting_existing_instances() {
        tauri::async_runtime::block_on(async {
            let (root, core, host) = fixture().await;
            let before = host.snapshot().await;
            let existing = before.instances[0].clone();
            core.create_workspace("Second Workspace").unwrap();
            host.execute(PackageHostCommand::ReconcileWorkspaces(
                teamarness_package_host::ReconcileWorkspaces { workspaces: vec![] },
            ))
            .await
            .unwrap();
            let after = host.snapshot().await;
            assert_eq!(after.instances.len(), 2);
            let retained = after
                .instances
                .iter()
                .find(|value| value.key == existing.key)
                .unwrap();
            assert_eq!(retained.process_generation, existing.process_generation);
            host.execute(PackageHostCommand::Shutdown(ShutdownHost))
                .await
                .unwrap();
            core.shutdown().await.unwrap();
            drop(root);
        });
    }

    async fn fixture() -> (tempfile::TempDir, Arc<CoreRuntime>, Arc<PackageHost>) {
        let root = tempfile::tempdir().unwrap();
        let application = root.path().join("application");
        let private = root.path().join("private");
        let package = application.join("fixture");
        fs::create_dir_all(package.join("backend")).unwrap();
        fs::create_dir_all(package.join("ui")).unwrap();
        fs::create_dir_all(&private).unwrap();
        let backend = fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../contracts/package-process/v1/fixtures/node-package-process.mjs"),
        )
        .unwrap();
        let backend_path = package.join("backend/package.mjs");
        fs::write(&backend_path, &backend).unwrap();
        fs::set_permissions(&backend_path, fs::Permissions::from_mode(0o700)).unwrap();
        let entrypoint = b"export function mount() { return async () => {}; }";
        let contract = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1,
            "$defs": {},
            "requests": {},
            "subscriptions": {"fixture.saturation": {"request": {}, "event": {}}}
        }))
        .unwrap();
        fs::write(package.join("ui/index.js"), entrypoint).unwrap();
        fs::write(package.join("ui/contract.json"), &contract).unwrap();
        fs::write(
            package.join("teamarness.package.json"),
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1,
                "identity": "com.example.desktop-fixture",
                "version": "1.0.0",
                "presentation": {"displayName": "Desktop fixture"},
                "protocol": {"major": 1, "minMinor": 0, "maxMinor": 0},
                "backends": [{
                    "target": "aarch64-apple-darwin",
                    "path": "backend/package.mjs",
                    "size": backend.len(),
                    "sha256": digest(&backend)
                }],
                "ui": {
                    "entrypoint": {"path": "ui/index.js", "size": entrypoint.len(), "sha256": digest(entrypoint)},
                    "assets": [],
                    "contract": {"path": "ui/contract.json", "sha256": digest(&contract)}
                },
                "capabilities": [{
                    "identity": "teamarness.workspace-context", "major": 1,
                    "minMinor": 0, "maxMinor": 0
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        let index = application.join("application-packages.v1.json");
        fs::write(
            &index,
            br#"{"schemaVersion":1,"packages":[{"directory":"fixture"}]}"#,
        )
        .unwrap();
        let core = Arc::new(
            CoreRuntime::open(CoreRuntimeConfig::new(root.path().join("core")))
                .await
                .unwrap(),
        );
        core.create_workspace("First Workspace").unwrap();
        let host = Arc::new(
            PackageHost::open(
                PackageHostConfig::new(
                    application,
                    index,
                    private,
                    InstallationPublicIdentity::new(core.public_identity().to_string()).unwrap(),
                ),
                Arc::clone(&core),
            )
            .await
            .unwrap(),
        );
        (root, core, host)
    }

    fn digest(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}
