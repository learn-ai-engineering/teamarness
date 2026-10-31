#![allow(clippy::result_large_err)]

use std::path::PathBuf;
use std::str::FromStr;

use tauri::{AppHandle, Manager, State, ipc::Channel};
use tauri_plugin_dialog::DialogExt;
use teamarness_package_host::{
    AcknowledgeUiEvents, CancelUiRequest, CloseUiMount, CloseUiSubscription, DisposePackage,
    HostRevision, InstallPersonal, InstallationRegistrationSnapshot, OpenUiMount,
    OpenUiSubscription, PackageErrorCode, PackageErrorStage, PackageHost, PackageHostCommand,
    PackageHostNotification, PackageHostOutcome, PackageHostSubscriptionSelector, PackageIdentity,
    PackageInstanceKey, PackageTier, PackageUiView, PackageValue, RecoverPackage,
    RegistrationStatus, ReloadPersonal, RemoveOverride, RemovePersonal, ReportUiFault,
    RestartPersonal, SuspendPackage, UiMountId, UiRequest, UiRequestId, UiSubscriptionId,
    WorkspaceId, WriteOverride,
};

use crate::DesktopState;
use crate::package_dto::{
    HostStateNotificationDto, InspectedPackageDto, OpenUiMountDto, PackageAuthoringEntryDto,
    PackageChangeDto, PackageHostSnapshotDto, PackageInstanceKeyDto, UiStreamMessageDto,
};
use crate::package_error::{PackageBridgeError, invalid_input};

pub const COMMAND_NAMES: [&str; 22] = [
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
    "package_choose_directory",
    "package_inspect_directory",
    "package_install",
    "package_reload",
    "package_remove",
    "package_write_override",
    "package_authoring_catalog",
    "package_restart",
    "app_restart",
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
    view: Option<String>,
    state: State<'_, DesktopState>,
) -> Result<OpenUiMountDto, PackageBridgeError> {
    let instance = parse_instance(instance)?;
    let view = parse_view(view.as_deref())?;
    state.remove_instance_tokens(&instance).await;
    let outcome = state
        .package_host()?
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount {
            instance: instance.clone(),
            view,
        }))
        .await
        .map_err(PackageBridgeError::from)?;
    let PackageHostOutcome::UiMountOpened { mount, loader, .. } = outcome else {
        unreachable!("OpenUiMount has one success outcome")
    };
    state
        .register_protocol_mount(
            loader.token.as_str(),
            &mount,
            &instance,
            loader.nested_content,
        )
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
    on_event: Channel<UiStreamMessageDto>,
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

async fn run_package_lifecycle(
    instance: PackageInstanceKeyDto,
    state: State<'_, DesktopState>,
    command: fn(PackageInstanceKey) -> PackageHostCommand,
    remove_tokens: bool,
) -> Result<(), PackageBridgeError> {
    let instance = parse_instance(instance)?;
    state
        .package_host()?
        .execute(command(instance.clone()))
        .await
        .map_err(PackageBridgeError::from)?;
    if remove_tokens {
        state.remove_instance_tokens(&instance).await;
    }
    Ok(())
}

#[tauri::command]
pub async fn package_suspend(
    instance: PackageInstanceKeyDto,
    state: State<'_, DesktopState>,
) -> Result<(), PackageBridgeError> {
    run_package_lifecycle(
        instance,
        state,
        |instance| PackageHostCommand::Suspend(SuspendPackage { instance }),
        false,
    )
    .await
}

#[tauri::command]
pub async fn package_recover(
    instance: PackageInstanceKeyDto,
    state: State<'_, DesktopState>,
) -> Result<(), PackageBridgeError> {
    run_package_lifecycle(
        instance,
        state,
        |instance| PackageHostCommand::Recover(RecoverPackage { instance }),
        false,
    )
    .await
}

#[tauri::command]
pub async fn package_dispose(
    instance: PackageInstanceKeyDto,
    state: State<'_, DesktopState>,
) -> Result<(), PackageBridgeError> {
    run_package_lifecycle(
        instance,
        state,
        |instance| PackageHostCommand::Dispose(DisposePackage { instance }),
        true,
    )
    .await
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

/// Replaces the process of one Personal Package Instance with a new generation.
/// A Team Instance is refused with `override_required`.
#[tauri::command]
pub async fn package_restart(
    instance: PackageInstanceKeyDto,
    state: State<'_, DesktopState>,
) -> Result<(), PackageBridgeError> {
    let host = state.package_host()?;
    restart(&host, parse_instance(instance)?).await
}

async fn restart(
    host: &PackageHost,
    instance: PackageInstanceKey,
) -> Result<(), PackageBridgeError> {
    host.execute(PackageHostCommand::RestartPersonal(RestartPersonal {
        instance,
    }))
    .await
    .map_err(PackageBridgeError::from)?;
    Ok(())
}

/// The declared Semantic Capabilities and Ports of every admitted Package, for
/// the Package page. Reading it starts no Package.
#[tauri::command]
pub async fn package_authoring_catalog(
    state: State<'_, DesktopState>,
) -> Result<Vec<PackageAuthoringEntryDto>, PackageBridgeError> {
    let host = state.package_host()?;
    Ok(authoring_catalog(&host))
}

fn authoring_catalog(host: &PackageHost) -> Vec<PackageAuthoringEntryDto> {
    host.authoring_catalog()
        .into_iter()
        .map(Into::into)
        .collect()
}

/// Shows the native folder picker. Returns the chosen absolute path, or `None`
/// when the person cancels.
#[tauri::command]
pub async fn package_choose_directory(
    app: AppHandle,
) -> Result<Option<String>, PackageBridgeError> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let mut picker = app.dialog().file().set_title("Choose a Package folder");
    if let Some(window) = app.get_webview_window("main") {
        picker = picker.set_parent(&window);
    }
    picker.pick_folder(move |chosen| {
        let _ = sender.send(chosen);
    });
    let Some(chosen) = receiver.await.ok().flatten() else {
        return Ok(None);
    };
    let path = chosen.into_path().map_err(|_| {
        invalid_input(
            PackageErrorCode::InvalidRegistration,
            PackageErrorStage::Discovery,
        )
    })?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Reads a Package Directory for the install preview. Runs no Package code.
#[tauri::command]
pub async fn package_inspect_directory(
    directory: String,
    state: State<'_, DesktopState>,
) -> Result<InspectedPackageDto, PackageBridgeError> {
    let host = state.package_host()?;
    inspect_directory(&host, directory).await
}

/// Registers and starts a Personal Package after the person accepted its risks.
#[tauri::command]
pub async fn package_install(
    directory: String,
    expected_identity: String,
    accepted_risks: Vec<String>,
    state: State<'_, DesktopState>,
) -> Result<PackageChangeDto, PackageBridgeError> {
    let host = state.package_host()?;
    install(&host, directory, expected_identity, accepted_risks).await
}

/// Re-admits a Personal Package from its directory after a rebuild.
#[tauri::command]
pub async fn package_reload(
    identity: String,
    state: State<'_, DesktopState>,
) -> Result<PackageChangeDto, PackageBridgeError> {
    let host = state.package_host()?;
    reload(&host, identity).await
}

/// Removes a Personal Package now, or the file entry of a Team Package identity
/// (an override, or one written by hand without it) at the next start. Data is kept.
#[tauri::command]
pub async fn package_remove(
    identity: String,
    state: State<'_, DesktopState>,
) -> Result<PackageChangeDto, PackageBridgeError> {
    let host = state.package_host()?;
    remove(&host, identity).await
}

/// Writes an override of a Team Package. It applies at the next start.
#[tauri::command]
pub async fn package_write_override(
    directory: String,
    expected_identity: String,
    accepted_risks: Vec<String>,
    state: State<'_, DesktopState>,
) -> Result<PackageChangeDto, PackageBridgeError> {
    let host = state.package_host()?;
    write_override(&host, directory, expected_identity, accepted_risks).await
}

/// Stops the Package Host and Core the way quitting does, then starts Teamarness again.
#[tauri::command]
pub async fn app_restart(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<(), PackageBridgeError> {
    let state = state.inner().clone();
    if !state.lifecycle.begin_shutdown() {
        return Err(invalid_input(
            PackageErrorCode::ShuttingDown,
            PackageErrorStage::Shutdown,
        ));
    }
    state.shutdown().await;
    // The exit handler sees the completed shutdown and lets the restart proceed.
    app.request_restart();
    Ok(())
}

async fn inspect_directory(
    host: &PackageHost,
    directory: String,
) -> Result<InspectedPackageDto, PackageBridgeError> {
    Ok(host
        .inspect_directory(PathBuf::from(directory))
        .await
        .map_err(PackageBridgeError::from)?
        .into())
}

async fn install(
    host: &PackageHost,
    directory: String,
    expected_identity: String,
    accepted_risks: Vec<String>,
) -> Result<PackageChangeDto, PackageBridgeError> {
    let outcome = host
        .execute(PackageHostCommand::InstallPersonal(InstallPersonal {
            directory: PathBuf::from(directory),
            expected_identity: parse_identity(expected_identity)?,
            accepted_risks,
        }))
        .await
        .map_err(PackageBridgeError::from)?;
    let PackageHostOutcome::PersonalInstalled { revision } = outcome else {
        unreachable!("InstallPersonal has one success outcome")
    };
    Ok(PackageChangeDto::live(revision))
}

async fn reload(
    host: &PackageHost,
    identity: String,
) -> Result<PackageChangeDto, PackageBridgeError> {
    let outcome = host
        .execute(PackageHostCommand::ReloadPersonal(ReloadPersonal {
            identity: parse_identity(identity)?,
        }))
        .await
        .map_err(PackageBridgeError::from)?;
    let PackageHostOutcome::PersonalReloaded { revision } = outcome else {
        unreachable!("ReloadPersonal has one success outcome")
    };
    Ok(PackageChangeDto::live(revision))
}

async fn remove(
    host: &PackageHost,
    identity: String,
) -> Result<PackageChangeDto, PackageBridgeError> {
    let identity = parse_identity(identity)?;
    // The snapshot registration says which tier the entry changes. An entry for
    // a Team Package identity (an override, or one written by hand without it)
    // is removed from the file and applies at the next start.
    let team_entry = host
        .snapshot()
        .await
        .registrations
        .iter()
        .any(|registration| {
            registration.expected_identity == identity.as_str() && team_identity_entry(registration)
        });
    if team_entry {
        let identity_text = identity.as_str().to_owned();
        host.execute(PackageHostCommand::RemoveOverride(RemoveOverride {
            identity,
        }))
        .await
        .map_err(PackageBridgeError::from)?;
        let snapshot = host.snapshot().await;
        // An override removed before it applied leaves the bundled Package running as it is.
        if !snapshot
            .registrations
            .iter()
            .any(|registration| registration.expected_identity == identity_text)
        {
            return Ok(PackageChangeDto::live(snapshot.revision));
        }
        return Ok(PackageChangeDto::after_restart());
    }
    let outcome = host
        .execute(PackageHostCommand::RemovePersonal(RemovePersonal {
            identity,
        }))
        .await
        .map_err(PackageBridgeError::from)?;
    let PackageHostOutcome::PersonalRemoved { revision } = outcome else {
        unreachable!("RemovePersonal has one success outcome")
    };
    Ok(PackageChangeDto::live(revision))
}

/// Whether a registration is a file entry for a Team Package identity, which
/// changes only at the next start.
fn team_identity_entry(registration: &InstallationRegistrationSnapshot) -> bool {
    registration.override_application
        || registration.tier == PackageTier::Team
        || matches!(
            registration.status,
            RegistrationStatus::PendingRestart { .. }
                | RegistrationStatus::Refused {
                    code: PackageErrorCode::OverrideRequired
                }
        )
}

async fn write_override(
    host: &PackageHost,
    directory: String,
    expected_identity: String,
    accepted_risks: Vec<String>,
) -> Result<PackageChangeDto, PackageBridgeError> {
    host.execute(PackageHostCommand::WriteOverride(WriteOverride {
        directory: PathBuf::from(directory),
        expected_identity: parse_identity(expected_identity)?,
        accepted_risks,
    }))
    .await
    .map_err(PackageBridgeError::from)?;
    Ok(PackageChangeDto::after_restart())
}

fn parse_identity(value: String) -> Result<PackageIdentity, PackageBridgeError> {
    PackageIdentity::from_str(&value).map_err(|_| {
        invalid_input(
            PackageErrorCode::InvalidPackageIdentity,
            PackageErrorStage::Discovery,
        )
    })
}

async fn forward_ui_events(
    host: std::sync::Arc<teamarness_package_host::PackageHost>,
    mut receiver: teamarness_package_host::PackageHostSubscription,
    mount: UiMountId,
    subscription: UiSubscriptionId,
    on_event: Channel<UiStreamMessageDto>,
) {
    loop {
        let message = match receiver.changed().await {
            Ok(PackageHostNotification::UiEvent(value)) => UiStreamMessageDto::Event(value.into()),
            // The Package ended the stream; the Shell reports it and stops, without a fault.
            Ok(PackageHostNotification::UiStreamCompleted(value)) => {
                let _ = on_event.send(UiStreamMessageDto::Completed(value.into()));
                return;
            }
            Err(error) => {
                let _ = on_event.send(UiStreamMessageDto::Failure(error.into()));
                return;
            }
            Ok(PackageHostNotification::HostState(_)) => unreachable!("typed UI subscription"),
        };
        if on_event.send(message).is_err() {
            let _ = host
                .execute(PackageHostCommand::CloseUiSubscription(
                    CloseUiSubscription {
                        mount: mount.clone(),
                        subscription: subscription.clone(),
                    },
                ))
                .await;
            return;
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

/// An absent view is the main view. A name the mount contract does not define is refused like
/// an undeclared view, since no Package can declare it.
fn parse_view(value: Option<&str>) -> Result<PackageUiView, PackageBridgeError> {
    match value {
        None | Some("main") => Ok(PackageUiView::Main),
        Some("settings") => Ok(PackageUiView::Settings),
        Some(_) => Err(invalid_input(
            PackageErrorCode::UiViewNotDeclared,
            PackageErrorStage::Ui,
        )),
    }
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
    use std::sync::{Arc, Mutex as StdMutex};

    use sha2::{Digest, Sha256};
    use tauri::ipc::InvokeResponseBody;
    use teamarness_core::{CoreRuntime, CoreRuntimeConfig};
    use teamarness_package_host::{
        InstallationPublicIdentity, PackageHost, PackageHostConfig, ShutdownHost,
    };

    #[test]
    fn command_registry_is_generic_and_exact() {
        assert_eq!(COMMAND_NAMES.len(), 22);
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
    fn an_absent_view_opens_main_and_an_unknown_view_is_refused() {
        assert_eq!(parse_view(None).unwrap(), PackageUiView::Main);
        assert_eq!(parse_view(Some("main")).unwrap(), PackageUiView::Main);
        assert_eq!(
            parse_view(Some("settings")).unwrap(),
            PackageUiView::Settings
        );
        for unknown in ["", "Settings", "preferences"] {
            let error = serde_json::to_value(parse_view(Some(unknown)).unwrap_err()).unwrap();
            assert_eq!(error["code"], "ui_view_not_declared", "{unknown}");
            assert_eq!(error["stage"], "ui", "{unknown}");
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
                .execute(PackageHostCommand::OpenUiMount(OpenUiMount::main(
                    instance.clone(),
                )))
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
    fn backend_event_contract_failure_reaches_desktop_without_stopping_the_backend() {
        tauri::async_runtime::block_on(async {
            let (root, core, host) = fixture().await;
            let before = host.snapshot().await.instances[0].clone();
            let opened = host
                .execute(PackageHostCommand::OpenUiMount(OpenUiMount::main(
                    before.key.clone(),
                )))
                .await
                .unwrap();
            let PackageHostOutcome::UiMountOpened { mount, .. } = opened else {
                unreachable!()
            };
            let subscription = UiSubscriptionId::new("invalid-event").unwrap();
            host.execute(PackageHostCommand::OpenUiSubscription(OpenUiSubscription {
                mount: mount.clone(),
                subscription: subscription.clone(),
                operation: "fixture.invalid_event".into(),
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
            let messages = Arc::new(StdMutex::new(Vec::new()));
            let received = Arc::clone(&messages);
            let channel = Channel::new(move |body| {
                let InvokeResponseBody::Json(json) = body else {
                    panic!("stream messages use JSON");
                };
                received
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str::<serde_json::Value>(&json).unwrap());
                Ok(())
            });

            forward_ui_events(
                Arc::clone(&host),
                receiver,
                mount.clone(),
                subscription.clone(),
                channel,
            )
            .await;

            {
                let messages = messages.lock().unwrap();
                assert_eq!(messages.len(), 1);
                assert_eq!(messages[0]["kind"], "failure");
                assert_eq!(messages[0]["value"]["code"], "ui_contract_violation");
                assert_eq!(messages[0]["value"]["stage"], "ui");
                assert_eq!(messages[0]["value"]["recovery"], "reload_ui");
            }
            assert!(
                host.execute(PackageHostCommand::AcknowledgeUiEvents(
                    AcknowledgeUiEvents {
                        mount: mount.clone(),
                        subscription,
                        through_sequence: 1,
                    },
                ))
                .await
                .is_err()
            );
            let after = host.snapshot().await.instances[0].clone();
            assert_eq!(after.lifecycle, before.lifecycle);
            assert_eq!(after.lifecycle.as_str(), "active");
            assert_eq!(after.process_generation, before.process_generation);
            assert_eq!(
                after.mount_state.map(|state| state.as_str()),
                Some("failed")
            );
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
                teamarness_package_host::ReconcileWorkspaces {},
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
        fs::create_dir_all(&private).unwrap();
        write_package(&application.join("fixture"), FIXTURE);
        let index = application.join("application-packages.v1.json");
        fs::write(
            &index,
            br#"{"schemaVersion":1,"packages":[{"directory":"fixture"}]}"#,
        )
        .unwrap();
        let core = Arc::new(
            CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(root.path().join("core")))
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

    /// Shuts `host` down and opens a new Host over the same directories.
    async fn reopen(
        root: &tempfile::TempDir,
        core: &Arc<CoreRuntime>,
        host: Arc<PackageHost>,
    ) -> Arc<PackageHost> {
        host.execute(PackageHostCommand::Shutdown(ShutdownHost))
            .await
            .unwrap();
        let application = root.path().join("application");
        Arc::new(
            PackageHost::open(
                PackageHostConfig::new(
                    application.clone(),
                    application.join("application-packages.v1.json"),
                    root.path().join("private"),
                    InstallationPublicIdentity::new(core.public_identity().to_string()).unwrap(),
                ),
                Arc::clone(core),
            )
            .await
            .unwrap(),
        )
    }

    const FIXTURE: &str = "com.example.desktop-fixture";
    const PERSONAL: &str = "com.example.desktop-personal";
    const RISKS: [&str; 3] = ["filesystem", "network", "process"];

    /// Writes a Package Directory with a Node Backend and a small UI contract.
    fn write_package(package: &Path, identity: &str) {
        fs::create_dir_all(package.join("backend")).unwrap();
        fs::create_dir_all(package.join("ui")).unwrap();
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
            "subscriptions": {
                "fixture.saturation": {"request": {}, "event": {}},
                "fixture.invalid_event": {
                    "request": {},
                    "event": {
                        "type": "object",
                        "properties": {"order": {"type": "integer"}},
                        "required": ["order"],
                        "additionalProperties": false
                    }
                }
            }
        }))
        .unwrap();
        fs::write(package.join("ui/index.js"), entrypoint).unwrap();
        fs::write(package.join("ui/contract.json"), &contract).unwrap();
        fs::write(
            package.join("teamarness.package.json"),
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1,
                "identity": identity,
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
    }

    fn registration_file(root: &tempfile::TempDir) -> Option<Vec<u8>> {
        fs::read(root.path().join("private/packages/registrations.v1.json")).ok()
    }

    fn code(error: PackageBridgeError) -> String {
        serde_json::to_value(error).unwrap()["code"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    async fn identities(host: &PackageHost) -> Vec<String> {
        host.snapshot()
            .await
            .instances
            .into_iter()
            .map(|instance| instance.key.package_identity.to_string())
            .collect()
    }

    async fn close(root: tempfile::TempDir, core: Arc<CoreRuntime>, host: Arc<PackageHost>) {
        host.execute(PackageHostCommand::Shutdown(ShutdownHost))
            .await
            .unwrap();
        core.shutdown().await.unwrap();
        drop(root);
    }

    /// Verifies: TEAM.SYS.PKG-R09
    #[test]
    fn installing_a_valid_directory_adds_its_instance_and_remove_drops_it() {
        tauri::async_runtime::block_on(async {
            let (root, core, host) = fixture().await;
            let directory = root.path().join("sources/personal");
            write_package(&directory, PERSONAL);
            let path = directory.to_string_lossy().into_owned();

            let inspected =
                serde_json::to_value(inspect_directory(&host, path.clone()).await.unwrap())
                    .unwrap();
            assert_eq!(inspected["identity"], PERSONAL);
            assert_eq!(inspected["displayName"], "Desktop fixture");
            assert_eq!(inspected["kind"], "new");
            assert!(inspected.get("description").is_none());
            assert_eq!(
                inspected["capabilities"][0]["identity"],
                "teamarness.workspace-context"
            );

            let installed = install(
                &host,
                path,
                PERSONAL.into(),
                RISKS.map(str::to_owned).to_vec(),
            )
            .await
            .unwrap();
            let installed = serde_json::to_value(installed).unwrap();
            assert_eq!(installed["restartRequired"], false);
            let snapshot = host.snapshot().await;
            assert_eq!(installed["revision"], snapshot.revision.0);
            assert!(identities(&host).await.contains(&PERSONAL.to_owned()));
            assert!(registration_file(&root).is_some());

            let removed =
                serde_json::to_value(remove(&host, PERSONAL.into()).await.unwrap()).unwrap();
            assert_eq!(removed["restartRequired"], false);
            assert_eq!(identities(&host).await, vec![FIXTURE.to_owned()]);
            close(root, core, host).await;
        });
    }

    /// Verifies: TEAM.SYS.PKG-R09
    #[test]
    fn an_invalid_directory_leaves_the_snapshot_and_file_unchanged() {
        tauri::async_runtime::block_on(async {
            let (root, core, host) = fixture().await;
            let empty = root.path().join("sources/empty");
            fs::create_dir_all(&empty).unwrap();
            let tampered = root.path().join("sources/tampered");
            write_package(&tampered, PERSONAL);
            fs::write(tampered.join("backend/package.mjs"), b"// changed").unwrap();
            let before = host.snapshot().await;

            for (directory, expected) in [
                (empty, "manifest_malformed"),
                (tampered, "artifact_changed"),
            ] {
                let path = directory.to_string_lossy().into_owned();
                let inspected = inspect_directory(&host, path.clone()).await.unwrap_err();
                assert_eq!(code(inspected), expected);
                let refused = install(
                    &host,
                    path,
                    PERSONAL.into(),
                    RISKS.map(str::to_owned).to_vec(),
                )
                .await
                .unwrap_err();
                assert_eq!(code(refused), expected);
            }
            let relative = install(
                &host,
                "sources/personal".into(),
                PERSONAL.into(),
                RISKS.map(str::to_owned).to_vec(),
            )
            .await
            .unwrap_err();
            assert_eq!(code(relative), "invalid_registration");
            let after = host.snapshot().await;
            assert_eq!(after.revision, before.revision);
            assert_eq!(identities(&host).await, vec![FIXTURE.to_owned()]);
            assert_eq!(registration_file(&root), None);
            close(root, core, host).await;
        });
    }

    /// Verifies: TEAM.SYS.PKG-R09
    #[test]
    fn an_override_is_written_for_a_restart_and_removed_the_same_way() {
        tauri::async_runtime::block_on(async {
            let (root, core, host) = fixture().await;
            let directory = root.path().join("sources/team-build");
            write_package(&directory, FIXTURE);
            let path = directory.to_string_lossy().into_owned();
            let inspected =
                serde_json::to_value(inspect_directory(&host, path.clone()).await.unwrap())
                    .unwrap();
            assert_eq!(inspected["kind"], "overrides_team");
            let refused = install(
                &host,
                path.clone(),
                FIXTURE.into(),
                RISKS.map(str::to_owned).to_vec(),
            )
            .await
            .unwrap_err();
            assert_eq!(code(refused), "override_required");
            let before = host.snapshot().await;

            let written = write_override(
                &host,
                path,
                FIXTURE.into(),
                RISKS.map(str::to_owned).to_vec(),
            )
            .await
            .unwrap();
            assert_eq!(
                serde_json::to_value(written).unwrap(),
                serde_json::json!({"revision": null, "restartRequired": true})
            );
            let file: serde_json::Value =
                serde_json::from_slice(&registration_file(&root).unwrap()).unwrap();
            assert_eq!(file["registrations"][0]["overrideApplication"], true);
            // No Instance changes; the snapshot publishes the pending override.
            let after = host.snapshot().await;
            assert!(after.revision > before.revision);
            assert_eq!(
                after.instances[0].process_generation,
                before.instances[0].process_generation
            );
            let pending = serde_json::to_value(PackageHostSnapshotDto::from(after)).unwrap();
            assert_eq!(
                pending["registrations"][0]["status"],
                serde_json::json!({"kind": "pending_restart", "change": "written"})
            );

            // Removing the override before it applied leaves nothing to restart for.
            let removed = remove(&host, FIXTURE.into()).await.unwrap();
            assert_eq!(
                serde_json::to_value(removed).unwrap()["restartRequired"],
                false
            );
            assert!(host.snapshot().await.registrations.is_empty());
            let file: serde_json::Value =
                serde_json::from_slice(&registration_file(&root).unwrap()).unwrap();
            assert_eq!(file["registrations"], serde_json::json!([]));
            assert_eq!(identities(&host).await, vec![FIXTURE.to_owned()]);
            close(root, core, host).await;
        });
    }

    /// Verifies: TEAM.SYS.PKG-R09
    #[test]
    fn a_hand_written_entry_for_a_team_identity_is_removed_for_the_next_start() {
        tauri::async_runtime::block_on(async {
            let (root, core, host) = fixture().await;
            let directory = root.path().join("sources/team-build");
            write_package(&directory, FIXTURE);
            write_override(
                &host,
                directory.to_string_lossy().into_owned(),
                FIXTURE.into(),
                RISKS.map(str::to_owned).to_vec(),
            )
            .await
            .unwrap();
            // A person drops `overrideApplication` by hand.
            let path = root.path().join("private/packages/registrations.v1.json");
            let mut file: serde_json::Value =
                serde_json::from_slice(&registration_file(&root).unwrap()).unwrap();
            file["registrations"][0]["overrideApplication"] = serde_json::json!(false);
            fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
            let host = reopen(&root, &core, host).await;
            assert_eq!(identities(&host).await, Vec::<String>::new());

            let reloaded = reload(&host, FIXTURE.into()).await.unwrap_err();
            assert_eq!(code(reloaded), "override_required");

            let removed = remove(&host, FIXTURE.into()).await.unwrap();
            assert_eq!(
                serde_json::to_value(removed).unwrap(),
                serde_json::json!({"revision": null, "restartRequired": true})
            );
            let file: serde_json::Value =
                serde_json::from_slice(&registration_file(&root).unwrap()).unwrap();
            assert_eq!(file["registrations"], serde_json::json!([]));
            let snapshot =
                serde_json::to_value(PackageHostSnapshotDto::from(host.snapshot().await)).unwrap();
            assert_eq!(
                snapshot["registrations"][0]["status"],
                serde_json::json!({"kind": "pending_restart", "change": "removed"})
            );

            let host = reopen(&root, &core, host).await;
            assert_eq!(identities(&host).await, vec![FIXTURE.to_owned()]);
            close(root, core, host).await;
        });
    }

    /// Verifies: TEAM.SYS.PKG-R09
    #[test]
    fn the_authoring_catalog_lists_capabilities_and_restart_is_personal_only() {
        tauri::async_runtime::block_on(async {
            let (root, core, host) = fixture().await;
            let directory = root.path().join("sources/personal");
            write_package(&directory, PERSONAL);
            install(
                &host,
                directory.to_string_lossy().into_owned(),
                PERSONAL.into(),
                RISKS.map(str::to_owned).to_vec(),
            )
            .await
            .unwrap();

            let catalog = serde_json::to_value(authoring_catalog(&host)).unwrap();
            let entries = catalog.as_array().unwrap();
            assert_eq!(entries.len(), 2, "Team and Personal Packages are listed");
            let personal = entries
                .iter()
                .find(|entry| entry["identity"] == PERSONAL)
                .unwrap();
            assert_eq!(
                personal["capabilities"],
                serde_json::json!([{
                    "identity": "teamarness.workspace-context",
                    "major": 1, "minMinor": 0, "maxMinor": 0
                }])
            );
            assert_eq!(personal["provided"], serde_json::json!([]));
            assert_eq!(personal["consumed"], serde_json::json!([]));
            assert!(
                personal.get("definitions").is_none(),
                "Port schemas stay out of the Shell"
            );

            let snapshot = host.snapshot().await;
            let key = |identity: &str| {
                snapshot
                    .instances
                    .iter()
                    .find(|instance| instance.key.package_identity.as_str() == identity)
                    .unwrap()
                    .clone()
            };
            let (team, personal) = (key(FIXTURE), key(PERSONAL));
            restart(&host, personal.key.clone()).await.unwrap();
            let refused = restart(&host, team.key.clone()).await.unwrap_err();
            assert_eq!(code(refused), "override_required");
            let after = host.snapshot().await;
            let generation = |identity: &str| {
                after
                    .instances
                    .iter()
                    .find(|instance| instance.key.package_identity.as_str() == identity)
                    .unwrap()
                    .process_generation
            };
            assert_eq!(
                generation(PERSONAL),
                personal.process_generation.map(|value| value + 1)
            );
            assert_eq!(generation(FIXTURE), team.process_generation);
            close(root, core, host).await;
        });
    }

    fn digest(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}
