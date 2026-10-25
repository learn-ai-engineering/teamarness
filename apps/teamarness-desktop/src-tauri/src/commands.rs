use std::str::FromStr;
use std::sync::Arc;

use tauri::{State, ipc::Channel};
use teamarness_core::{PrincipalId, WorkspaceId, WorkspaceInvitation};
use teamarness_package_host::{PackageHostCommand, ReconcileWorkspaces};

use crate::DesktopState;
use crate::dto::{
    InstallationDto, InvitationDto, InvitationSensitivity, MembershipDto, PrincipalsSnapshotDto,
    WorkspaceDto, WorkspacePrincipalDto, WorkspaceStateDto,
};
use crate::error::BridgeError;

pub const COMMAND_NAMES: [&str; 11] = [
    "installation_details",
    "list_workspaces",
    "create_workspace",
    "create_workspace_invitation",
    "join_workspace",
    "current_membership",
    "principals_snapshot",
    "subscribe_principals",
    "rename_workspace_principal",
    "add_workspace_agent",
    "remove_workspace_agent",
];

async fn blocking<T, F>(state: &DesktopState, operation: F) -> Result<T, BridgeError>
where
    T: Send + 'static,
    F: FnOnce(Arc<teamarness_core::CoreRuntime>) -> Result<T, BridgeError> + Send + 'static,
{
    let runtime = state.runtime();
    let lease = state.lifecycle.blocking_work();
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        operation(runtime)
    })
    .await
    .map_err(|_| BridgeError::shutdown())?
}

#[tauri::command]
pub async fn installation_details(
    state: State<'_, DesktopState>,
) -> Result<InstallationDto, BridgeError> {
    let state = state.inner().clone();
    let dispatch = state.clone();
    dispatch
        .dispatch(async move {
            blocking(&state, |runtime| {
                Ok(InstallationDto {
                    identity: runtime.public_identity().to_string(),
                    display_name: runtime.display_name(),
                })
            })
            .await
        })
        .await?
}

#[tauri::command]
pub async fn list_workspaces(
    state: State<'_, DesktopState>,
) -> Result<Vec<WorkspaceDto>, BridgeError> {
    let state = state.inner().clone();
    let dispatch = state.clone();
    dispatch
        .dispatch(async move {
            blocking(&state, |runtime| {
                runtime
                    .list_workspaces()
                    .map(|values| values.into_iter().map(WorkspaceDto::from).collect())
                    .map_err(BridgeError::from)
            })
            .await
        })
        .await?
}

#[tauri::command]
pub async fn create_workspace(
    name: String,
    person_name: String,
    state: State<'_, DesktopState>,
) -> Result<WorkspaceStateDto, BridgeError> {
    let state = state.inner().clone();
    let dispatch = state.clone();
    dispatch
        .dispatch(async move {
            let created = blocking(&state, move |runtime| {
                let workspace = runtime
                    .create_workspace_with_person_name(&name, &person_name)
                    .map_err(BridgeError::from)?;
                let membership = roster(&runtime, workspace.id())?;
                Ok(WorkspaceStateDto {
                    workspace: workspace.into(),
                    membership,
                })
            })
            .await?;
            reconcile_packages(&state).await?;
            Ok(created)
        })
        .await?
}

#[tauri::command]
pub async fn create_workspace_invitation(
    workspace_id: String,
    state: State<'_, DesktopState>,
) -> Result<InvitationDto, BridgeError> {
    let state = state.inner().clone();
    let dispatch = state.clone();
    dispatch
        .dispatch(async move {
            let workspace_id =
                WorkspaceId::from_str(&workspace_id).map_err(|_| BridgeError::invalid_input())?;
            let runtime = state.runtime();
            let invitation = runtime
                .create_workspace_invitation(workspace_id)
                .await
                .map_err(BridgeError::from)?;
            Ok(InvitationDto {
                value: invitation.to_string(),
                sensitivity: InvitationSensitivity::BearerAdmission,
            })
        })
        .await?
}

#[tauri::command]
pub async fn join_workspace(
    invitation: String,
    person_name: String,
    state: State<'_, DesktopState>,
) -> Result<WorkspaceStateDto, BridgeError> {
    let state = state.inner().clone();
    let dispatch = state.clone();
    dispatch
        .dispatch(async move {
            let invitation = WorkspaceInvitation::from_str(&invitation)
                .map_err(|_| BridgeError::invalid_input())?;
            let runtime = state.runtime();
            let (workspace, _) = runtime
                .join_workspace_with_person_name(&invitation, &person_name)
                .await
                .map_err(BridgeError::from)?;
            reconcile_packages(&state).await?;
            Ok(WorkspaceStateDto {
                membership: roster(&runtime, workspace.id())?,
                workspace: workspace.into(),
            })
        })
        .await?
}

#[tauri::command]
pub async fn current_membership(
    workspace_id: String,
    state: State<'_, DesktopState>,
) -> Result<MembershipDto, BridgeError> {
    let state = state.inner().clone();
    let dispatch = state.clone();
    dispatch
        .dispatch(async move {
            let workspace_id =
                WorkspaceId::from_str(&workspace_id).map_err(|_| BridgeError::invalid_input())?;
            blocking(&state, move |runtime| roster(&runtime, workspace_id)).await
        })
        .await?
}

fn roster(
    runtime: &teamarness_core::CoreRuntime,
    workspace: WorkspaceId,
) -> Result<MembershipDto, BridgeError> {
    let (membership, principals) = runtime
        .principal_roster(workspace)
        .map_err(BridgeError::from)?;
    MembershipDto::from_core(membership, &principals)
}

fn snapshot_dto(
    workspace: WorkspaceId,
    snapshot: (u64, Vec<teamarness_core::WorkspacePrincipal>),
) -> PrincipalsSnapshotDto {
    PrincipalsSnapshotDto {
        workspace_id: workspace.to_string(),
        cursor: snapshot.0.to_string(),
        principals: snapshot
            .1
            .into_iter()
            .map(WorkspacePrincipalDto::from)
            .collect(),
    }
}

#[tauri::command]
pub async fn principals_snapshot(
    workspace_id: String,
    state: State<'_, DesktopState>,
) -> Result<PrincipalsSnapshotDto, BridgeError> {
    let state = state.inner().clone();
    let dispatch = state.clone();
    dispatch
        .dispatch(async move {
            let workspace =
                WorkspaceId::from_str(&workspace_id).map_err(|_| BridgeError::invalid_input())?;
            blocking(&state, move |runtime| {
                runtime
                    .principals_snapshot(workspace)
                    .map(|snapshot| snapshot_dto(workspace, snapshot))
                    .map_err(BridgeError::from)
            })
            .await
        })
        .await?
}

#[tauri::command]
pub async fn rename_workspace_principal(
    workspace_id: String,
    principal_id: String,
    name: String,
    state: State<'_, DesktopState>,
) -> Result<(), BridgeError> {
    let state = state.inner().clone();
    let dispatch = state.clone();
    dispatch
        .dispatch(async move {
            let workspace =
                WorkspaceId::from_str(&workspace_id).map_err(|_| BridgeError::invalid_input())?;
            let principal =
                PrincipalId::from_str(&principal_id).map_err(|_| BridgeError::invalid_input())?;
            blocking(&state, move |runtime| {
                runtime
                    .rename_principal(workspace, principal, &name)
                    .map_err(BridgeError::from)
            })
            .await
        })
        .await?
}

#[tauri::command]
pub async fn add_workspace_agent(
    workspace_id: String,
    name: String,
    state: State<'_, DesktopState>,
) -> Result<(), BridgeError> {
    let state = state.inner().clone();
    let dispatch = state.clone();
    dispatch
        .dispatch(async move {
            let workspace =
                WorkspaceId::from_str(&workspace_id).map_err(|_| BridgeError::invalid_input())?;
            blocking(&state, move |runtime| {
                add_agent_mutation(&runtime, workspace, &name)
            })
            .await
        })
        .await?
}

fn add_agent_mutation(
    runtime: &teamarness_core::CoreRuntime,
    workspace: WorkspaceId,
    name: &str,
) -> Result<(), BridgeError> {
    runtime
        .add_agent(workspace, name)
        .map(|_| ())
        .map_err(BridgeError::from)
}

#[tauri::command]
pub async fn remove_workspace_agent(
    workspace_id: String,
    agent_id: String,
    state: State<'_, DesktopState>,
) -> Result<(), BridgeError> {
    let state = state.inner().clone();
    let dispatch = state.clone();
    dispatch
        .dispatch(async move {
            let workspace =
                WorkspaceId::from_str(&workspace_id).map_err(|_| BridgeError::invalid_input())?;
            let agent =
                PrincipalId::from_str(&agent_id).map_err(|_| BridgeError::invalid_input())?;
            blocking(&state, move |runtime| {
                remove_agent_mutation(&runtime, workspace, agent)
            })
            .await
        })
        .await?
}

fn remove_agent_mutation(
    runtime: &teamarness_core::CoreRuntime,
    workspace: WorkspaceId,
    agent: PrincipalId,
) -> Result<(), BridgeError> {
    runtime
        .remove_agent(workspace, agent)
        .map_err(BridgeError::from)
}

#[tauri::command]
pub async fn subscribe_principals(
    workspace_id: String,
    after_cursor: String,
    on_event: Channel<PrincipalsStreamEvent>,
    state: State<'_, DesktopState>,
) -> Result<(), BridgeError> {
    let state = state.inner().clone();
    let dispatch = state.clone();
    dispatch
        .dispatch(async move {
            let workspace =
                WorkspaceId::from_str(&workspace_id).map_err(|_| BridgeError::invalid_input())?;
            let after: u64 = after_cursor
                .parse()
                .map_err(|_| BridgeError::invalid_input())?;
            if after.to_string() != after_cursor {
                return Err(BridgeError::invalid_input());
            }
            let runtime = state.runtime();
            let watch_runtime = Arc::clone(&runtime);
            let lease = state.lifecycle.blocking_work();
            let mut changes = tauri::async_runtime::spawn_blocking(move || {
                let _lease = lease;
                watch_runtime.subscribe_principals(workspace)
            })
            .await
            .map_err(|_| BridgeError::shutdown())?
            .map_err(BridgeError::from)?;
            if *changes.borrow() < after {
                return Err(BridgeError::invalid_input());
            }
            let lifecycle = state.lifecycle.clone();
            let forwarder = lifecycle.forwarder();
            tauri::async_runtime::spawn(async move {
                let _forwarder = forwarder;
                forward_principals(
                    &lifecycle,
                    &mut changes,
                    after,
                    || {
                        let runtime = Arc::clone(&runtime);
                        let lease = lifecycle.blocking_work();
                        async move {
                            tauri::async_runtime::spawn_blocking(move || {
                                let _lease = lease;
                                runtime.principals_snapshot(workspace)
                            })
                            .await
                            .ok()?
                            .ok()
                            .map(|snapshot| snapshot_dto(workspace, snapshot))
                        }
                    },
                    |event| on_event.send(event).is_ok(),
                )
                .await;
            });
            Ok(())
        })
        .await?
}

#[derive(serde::Serialize)]
#[serde(tag = "kind", content = "snapshot", rename_all = "snake_case")]
pub enum PrincipalsStreamEvent {
    Snapshot(PrincipalsSnapshotDto),
    Terminated,
}

async fn forward_principals<F, Read, Send>(
    lifecycle: &crate::lifecycle::Lifecycle,
    changes: &mut tokio::sync::watch::Receiver<u64>,
    after: u64,
    mut read: F,
    mut send: Send,
) where
    F: FnMut() -> Read,
    Read: std::future::Future<Output = Option<PrincipalsSnapshotDto>>,
    Send: FnMut(PrincipalsStreamEvent) -> bool,
{
    let mut cursor = after;
    loop {
        if lifecycle.status().phase != crate::lifecycle::Phase::Running {
            break;
        }
        let generation = *changes.borrow_and_update();
        if generation < cursor {
            break;
        }
        if generation > cursor {
            let Some(snapshot) = read().await else {
                break;
            };
            let Ok(next) = snapshot.cursor.parse::<u64>() else {
                break;
            };
            if next > cursor {
                cursor = next;
                if !send(PrincipalsStreamEvent::Snapshot(snapshot)) {
                    break;
                }
            }
            continue;
        }
        tokio::select! { biased;
            _ = lifecycle.cancelled() => break,
            result = changes.changed() => if result.is_err() { break; },
        }
    }
    let _ = send(PrincipalsStreamEvent::Terminated);
}

async fn reconcile_packages(state: &DesktopState) -> Result<(), BridgeError> {
    state
        .package_host()
        .execute(PackageHostCommand::ReconcileWorkspaces(
            ReconcileWorkspaces {},
        ))
        .await
        .map_err(|error| BridgeError::reconcile(&error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roster_remains_consistent_when_an_agent_is_removed_between_reads() {
        let path = std::env::temp_dir().join(format!(
            "shell-roster-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let runtime = teamarness_core::CoreRuntime::open_loopback_for_tests(
            teamarness_core::CoreRuntimeConfig::new(&path),
        )
        .await
        .unwrap();
        let workspace = runtime
            .create_workspace_with_person_name("Team", "Owner")
            .unwrap()
            .id();
        let agent = runtime.add_agent(workspace, "Agent").unwrap();
        let old_membership = runtime.membership_snapshot(workspace).unwrap();
        runtime.remove_agent(workspace, agent).unwrap();
        let (_, new_principals) = runtime.principals_snapshot(workspace).unwrap();
        assert!(MembershipDto::from_core(old_membership, &new_principals).is_err());
        let current = roster(&runtime, workspace).unwrap();
        assert_eq!(current.members.len(), 1);
        assert_eq!(
            current.epoch,
            runtime
                .membership_snapshot(workspace)
                .unwrap()
                .epoch()
                .value()
                .to_string()
        );
        runtime.shutdown().await.unwrap();
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn agent_mutation_acknowledges_core_commit_without_a_roster_read() {
        let path = std::env::temp_dir().join(format!(
            "shell-agent-ack-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let runtime = teamarness_core::CoreRuntime::open_loopback_for_tests(
            teamarness_core::CoreRuntimeConfig::new(&path),
        )
        .await
        .unwrap();
        let workspace = runtime
            .create_workspace_with_person_name("Team", "Owner")
            .unwrap()
            .id();
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../contracts/fixtures/bridge-contract.json"
        ))
        .unwrap();
        add_agent_mutation(&runtime, workspace, "Agent").unwrap();
        assert_eq!(
            serde_json::to_value(()).unwrap(),
            fixture["agentMutationResponses"]["addWorkspaceAgent"]
        );
        let agent = runtime
            .principal_roster(workspace)
            .unwrap()
            .1
            .into_iter()
            .find(|principal| principal.kind() == teamarness_core::PrincipalKind::Agent)
            .unwrap()
            .id();
        remove_agent_mutation(&runtime, workspace, agent).unwrap();
        assert_eq!(
            serde_json::to_value(()).unwrap(),
            fixture["agentMutationResponses"]["removeWorkspaceAgent"]
        );
        assert!(roster(&runtime, WorkspaceId::from_str(&"0".repeat(32)).unwrap()).is_err());
        runtime.shutdown().await.unwrap();
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn post_start_name_read_failure_signals_termination() {
        let (wake, mut changes) = tokio::sync::watch::channel(1_u64);
        let mut reads = 0;
        let mut events = Vec::new();
        forward_principals(
            &crate::lifecycle::Lifecycle::new(),
            &mut changes,
            0,
            || {
                reads += 1;
                let snapshot = (reads == 1).then(|| PrincipalsSnapshotDto {
                    workspace_id: "a".repeat(32),
                    cursor: "1".into(),
                    principals: vec![],
                });
                async move { snapshot }
            },
            |event| {
                if matches!(event, PrincipalsStreamEvent::Snapshot(_)) {
                    wake.send(2).unwrap();
                }
                events.push(event);
                true
            },
        )
        .await;
        assert_eq!(reads, 2);
        assert!(matches!(
            events.as_slice(),
            [
                PrincipalsStreamEvent::Snapshot(_),
                PrincipalsStreamEvent::Terminated
            ]
        ));
        assert_eq!(
            serde_json::to_value(&events[1]).unwrap(),
            serde_json::json!({"kind": "terminated"})
        );
    }

    #[test]
    fn command_registry_is_exact_and_domain_neutral() {
        assert_eq!(
            COMMAND_NAMES,
            [
                "installation_details",
                "list_workspaces",
                "create_workspace",
                "create_workspace_invitation",
                "join_workspace",
                "current_membership",
                "principals_snapshot",
                "subscribe_principals",
                "rename_workspace_principal",
                "add_workspace_agent",
                "remove_workspace_agent",
            ]
        );
        let registry = COMMAND_NAMES.join(" ");
        for forbidden in [
            "conversation",
            "repository",
            "storage_path",
            "endpoint",
            "router",
        ] {
            assert!(!registry.contains(forbidden));
        }
    }

    #[tokio::test]
    async fn blocking_dispatch_retains_lease_after_waiter_drop_through_reconciliation() {
        let root = tempfile::tempdir().unwrap();
        let application = root.path().join("application");
        std::fs::create_dir(&application).unwrap();
        let index = application.join("index.json");
        std::fs::write(&index, br#"{"schemaVersion":1,"packages":[]}"#).unwrap();
        let runtime = Arc::new(
            teamarness_core::CoreRuntime::open_loopback_for_tests(
                teamarness_core::CoreRuntimeConfig::new(root.path().join("core")),
            )
            .await
            .unwrap(),
        );
        let host = Arc::new(
            teamarness_package_host::PackageHost::open(
                teamarness_package_host::PackageHostConfig::new(
                    application,
                    index,
                    root.path().join("private"),
                    teamarness_package_host::InstallationPublicIdentity::new(
                        runtime.public_identity().to_string(),
                    )
                    .unwrap(),
                ),
                runtime.clone(),
            )
            .await
            .unwrap(),
        );
        let state = DesktopState::new(runtime.clone(), host);
        let (entered, received) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let work = state.clone();
        let dispatch = state.clone();
        let waiter = tokio::spawn(async move {
            dispatch
                .dispatch(async move {
                    blocking(&work, move |runtime| {
                        entered.send(()).unwrap();
                        released.recv().unwrap();
                        runtime.create_workspace("Admitted").unwrap();
                        Ok(())
                    })
                    .await
                    .unwrap();
                    reconcile_packages(&work).await.unwrap();
                })
                .await
        });
        received.await.unwrap();
        waiter.abort();
        // First intent closes admission while the actual blocking adapter owns its lease.
        struct Cleanup(DesktopState);
        impl crate::lifecycle::TerminalAdapter for Cleanup {
            fn stop_updates(&self) {}
            fn cleanup(
                &self,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = crate::lifecycle::ShutdownErrors> + Send + '_>,
            > {
                Box::pin(async {
                    assert_eq!(self.0.packages.snapshot().await.instances.len(), 0);
                    assert_eq!(self.0.runtime.list_workspaces().unwrap().len(), 1);
                    self.0.cleanup().await
                })
            }
            fn install(
                &self,
                _: crate::lifecycle::Install,
            ) -> Result<(), crate::lifecycle::InstallFailure> {
                panic!("Quit never installs")
            }
            fn restart(&self) {
                panic!("Quit never restarts")
            }
            fn exit(&self) {}
        }
        state.lifecycle.quit(Arc::new(Cleanup(state.clone())));
        assert_eq!(
            state.lifecycle.status().phase,
            crate::lifecycle::Phase::Stopping
        );
        release.send(()).unwrap();
        state.lifecycle.finished().await;
        assert_eq!(
            state.lifecycle.status().phase,
            crate::lifecycle::Phase::Stopped
        );
    }
}
