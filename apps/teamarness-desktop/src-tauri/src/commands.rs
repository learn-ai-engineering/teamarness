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
    let runtime = state.runtime()?;
    tauri::async_runtime::spawn_blocking(move || operation(runtime))
        .await
        .map_err(|_| BridgeError::shutdown())?
}

#[tauri::command]
pub async fn installation_details(
    state: State<'_, DesktopState>,
) -> Result<InstallationDto, BridgeError> {
    blocking(&state, |runtime| {
        Ok(InstallationDto {
            identity: runtime.public_identity().to_string(),
            display_name: runtime.display_name(),
        })
    })
    .await
}

#[tauri::command]
pub async fn list_workspaces(
    state: State<'_, DesktopState>,
) -> Result<Vec<WorkspaceDto>, BridgeError> {
    blocking(&state, |runtime| {
        runtime
            .list_workspaces()
            .map(|values| values.into_iter().map(WorkspaceDto::from).collect())
            .map_err(BridgeError::from)
    })
    .await
}

#[tauri::command]
pub async fn create_workspace(
    name: String,
    person_name: String,
    state: State<'_, DesktopState>,
) -> Result<WorkspaceStateDto, BridgeError> {
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
}

#[tauri::command]
pub async fn create_workspace_invitation(
    workspace_id: String,
    state: State<'_, DesktopState>,
) -> Result<InvitationDto, BridgeError> {
    let workspace_id =
        WorkspaceId::from_str(&workspace_id).map_err(|_| BridgeError::invalid_input())?;
    let runtime = state.runtime()?;
    let invitation = runtime
        .create_workspace_invitation(workspace_id)
        .await
        .map_err(BridgeError::from)?;
    Ok(InvitationDto {
        value: invitation.to_string(),
        sensitivity: InvitationSensitivity::BearerAdmission,
    })
}

#[tauri::command]
pub async fn join_workspace(
    invitation: String,
    person_name: String,
    state: State<'_, DesktopState>,
) -> Result<WorkspaceStateDto, BridgeError> {
    let invitation =
        WorkspaceInvitation::from_str(&invitation).map_err(|_| BridgeError::invalid_input())?;
    let runtime = state.runtime()?;
    let (workspace, _) = runtime
        .join_workspace_with_person_name(&invitation, &person_name)
        .await
        .map_err(BridgeError::from)?;
    reconcile_packages(&state).await?;
    Ok(WorkspaceStateDto {
        membership: roster(&runtime, workspace.id())?,
        workspace: workspace.into(),
    })
}

#[tauri::command]
pub async fn current_membership(
    workspace_id: String,
    state: State<'_, DesktopState>,
) -> Result<MembershipDto, BridgeError> {
    let workspace_id =
        WorkspaceId::from_str(&workspace_id).map_err(|_| BridgeError::invalid_input())?;
    blocking(&state, move |runtime| roster(&runtime, workspace_id)).await
}

fn roster(
    runtime: &teamarness_core::CoreRuntime,
    workspace: WorkspaceId,
) -> Result<MembershipDto, BridgeError> {
    let membership = runtime
        .membership_snapshot(workspace)
        .map_err(BridgeError::from)?;
    let (_, principals) = runtime
        .principals_snapshot(workspace)
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
    let workspace =
        WorkspaceId::from_str(&workspace_id).map_err(|_| BridgeError::invalid_input())?;
    blocking(&state, move |runtime| {
        runtime
            .principals_snapshot(workspace)
            .map(|snapshot| snapshot_dto(workspace, snapshot))
            .map_err(BridgeError::from)
    })
    .await
}

#[tauri::command]
pub async fn rename_workspace_principal(
    workspace_id: String,
    principal_id: String,
    name: String,
    state: State<'_, DesktopState>,
) -> Result<(), BridgeError> {
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
}

#[tauri::command]
pub async fn add_workspace_agent(
    workspace_id: String,
    name: String,
    state: State<'_, DesktopState>,
) -> Result<MembershipDto, BridgeError> {
    let workspace =
        WorkspaceId::from_str(&workspace_id).map_err(|_| BridgeError::invalid_input())?;
    blocking(&state, move |runtime| {
        runtime
            .add_agent(workspace, &name)
            .map_err(BridgeError::from)?;
        roster(&runtime, workspace)
    })
    .await
}

#[tauri::command]
pub async fn remove_workspace_agent(
    workspace_id: String,
    agent_id: String,
    state: State<'_, DesktopState>,
) -> Result<MembershipDto, BridgeError> {
    let workspace =
        WorkspaceId::from_str(&workspace_id).map_err(|_| BridgeError::invalid_input())?;
    let agent = PrincipalId::from_str(&agent_id).map_err(|_| BridgeError::invalid_input())?;
    blocking(&state, move |runtime| {
        runtime
            .remove_agent(workspace, agent)
            .map_err(BridgeError::from)?;
        roster(&runtime, workspace)
    })
    .await
}

#[tauri::command]
pub async fn subscribe_principals(
    workspace_id: String,
    after_cursor: String,
    on_event: Channel<PrincipalsStreamEvent>,
    state: State<'_, DesktopState>,
) -> Result<(), BridgeError> {
    let workspace =
        WorkspaceId::from_str(&workspace_id).map_err(|_| BridgeError::invalid_input())?;
    let after: u64 = after_cursor
        .parse()
        .map_err(|_| BridgeError::invalid_input())?;
    if after.to_string() != after_cursor {
        return Err(BridgeError::invalid_input());
    }
    let runtime = state.runtime()?;
    let watch_runtime = Arc::clone(&runtime);
    let mut changes =
        tauri::async_runtime::spawn_blocking(move || watch_runtime.subscribe_principals(workspace))
            .await
            .map_err(|_| BridgeError::shutdown())?
            .map_err(BridgeError::from)?;
    if *changes.borrow() < after {
        return Err(BridgeError::invalid_input());
    }
    tauri::async_runtime::spawn(async move {
        forward_principals(
            &mut changes,
            after,
            || {
                let runtime = Arc::clone(&runtime);
                async move {
                    tauri::async_runtime::spawn_blocking(move || {
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
}

#[derive(serde::Serialize)]
#[serde(tag = "kind", content = "snapshot", rename_all = "snake_case")]
pub enum PrincipalsStreamEvent {
    Snapshot(PrincipalsSnapshotDto),
    Terminated,
}

async fn forward_principals<F, Read, Send>(
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
        if changes.changed().await.is_err() {
            break;
        }
    }
    let _ = send(PrincipalsStreamEvent::Terminated);
}

async fn reconcile_packages(state: &DesktopState) -> Result<(), BridgeError> {
    state
        .package_host()
        .map_err(|_| BridgeError::shutdown())?
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
    async fn post_start_name_read_failure_signals_termination() {
        let (wake, mut changes) = tokio::sync::watch::channel(1_u64);
        let mut reads = 0;
        let mut events = Vec::new();
        forward_principals(
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
}
