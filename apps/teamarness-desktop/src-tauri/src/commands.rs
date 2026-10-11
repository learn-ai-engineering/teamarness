use std::str::FromStr;
use std::sync::Arc;

use tauri::{State, ipc::Channel};
use teamarness_core::{WorkspaceId, WorkspaceInvitation};
use teamarness_package_host::{PackageHostCommand, ReconcileWorkspaces};

use crate::DesktopState;
use crate::dto::{
    InstallationDto, InvitationDto, InvitationSensitivity, MembershipDto, PersonNameSnapshotDto,
    WorkspaceDto, WorkspacePersonDto, WorkspaceStateDto,
};
use crate::error::BridgeError;

pub const COMMAND_NAMES: [&str; 9] = [
    "installation_details",
    "list_workspaces",
    "create_workspace",
    "create_workspace_invitation",
    "join_workspace",
    "current_membership",
    "person_name_snapshot",
    "subscribe_person_names",
    "set_own_person_name",
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
        let membership = runtime
            .membership_snapshot(workspace.id())
            .map_err(BridgeError::from)?;
        Ok(WorkspaceStateDto {
            workspace: workspace.into(),
            membership: membership.into(),
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
    let (workspace, membership) = runtime
        .join_workspace_with_person_name(&invitation, &person_name)
        .await
        .map_err(BridgeError::from)?;
    reconcile_packages(&state).await?;
    Ok(WorkspaceStateDto {
        workspace: workspace.into(),
        membership: membership.into(),
    })
}

#[tauri::command]
pub async fn current_membership(
    workspace_id: String,
    state: State<'_, DesktopState>,
) -> Result<MembershipDto, BridgeError> {
    let workspace_id =
        WorkspaceId::from_str(&workspace_id).map_err(|_| BridgeError::invalid_input())?;
    blocking(&state, move |runtime| {
        runtime
            .membership_snapshot(workspace_id)
            .map(MembershipDto::from)
            .map_err(BridgeError::from)
    })
    .await
}

fn snapshot_dto(
    workspace: WorkspaceId,
    snapshot: (
        u64,
        Vec<(
            teamarness_core::PublicIdentity,
            teamarness_core::WorkspacePerson,
        )>,
    ),
) -> PersonNameSnapshotDto {
    PersonNameSnapshotDto {
        workspace_id: workspace.to_string(),
        cursor: snapshot.0.to_string(),
        persons: snapshot
            .1
            .into_iter()
            .map(|(identity, person)| WorkspacePersonDto::from_core(identity, person))
            .collect(),
    }
}

#[tauri::command]
pub async fn person_name_snapshot(
    workspace_id: String,
    state: State<'_, DesktopState>,
) -> Result<PersonNameSnapshotDto, BridgeError> {
    let workspace =
        WorkspaceId::from_str(&workspace_id).map_err(|_| BridgeError::invalid_input())?;
    blocking(&state, move |runtime| {
        runtime
            .person_name_snapshot(workspace)
            .map(|snapshot| snapshot_dto(workspace, snapshot))
            .map_err(BridgeError::from)
    })
    .await
}

#[tauri::command]
pub async fn set_own_person_name(
    workspace_id: String,
    name: String,
    state: State<'_, DesktopState>,
) -> Result<(), BridgeError> {
    let workspace =
        WorkspaceId::from_str(&workspace_id).map_err(|_| BridgeError::invalid_input())?;
    blocking(&state, move |runtime| {
        runtime
            .set_own_person_name(workspace, &name)
            .map_err(BridgeError::from)
    })
    .await
}

#[tauri::command]
pub async fn subscribe_person_names(
    workspace_id: String,
    after_cursor: String,
    on_event: Channel<PersonNameSnapshotDto>,
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
    let mut changes = tauri::async_runtime::spawn_blocking(move || {
        watch_runtime.subscribe_person_names(workspace)
    })
    .await
    .map_err(|_| BridgeError::shutdown())?
    .map_err(BridgeError::from)?;
    if *changes.borrow() < after {
        return Err(BridgeError::invalid_input());
    }
    tauri::async_runtime::spawn(async move {
        let mut cursor = after;
        loop {
            let generation = *changes.borrow_and_update();
            if generation < cursor {
                break;
            }
            if generation > cursor {
                let snapshot_runtime = Arc::clone(&runtime);
                let read = tauri::async_runtime::spawn_blocking(move || {
                    snapshot_runtime.person_name_snapshot(workspace)
                })
                .await;
                let Ok(Ok(snapshot)) = read else {
                    break;
                };
                if snapshot.0 > cursor {
                    cursor = snapshot.0;
                    if on_event.send(snapshot_dto(workspace, snapshot)).is_err() {
                        break;
                    }
                }
                continue;
            }
            if changes.changed().await.is_err() {
                break;
            }
        }
    });
    Ok(())
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
                "person_name_snapshot",
                "subscribe_person_names",
                "set_own_person_name",
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
