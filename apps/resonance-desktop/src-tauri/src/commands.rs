use std::str::FromStr;
use std::sync::Arc;

use resonance_core::{WorkspaceId, WorkspaceInvitation};
use tauri::State;

use crate::DesktopState;
use crate::dto::{
    InstallationDto, InvitationDto, InvitationSensitivity, MembershipDto, WorkspaceDto,
    WorkspaceStateDto,
};
use crate::error::BridgeError;

pub const COMMAND_NAMES: [&str; 6] = [
    "installation_details",
    "list_workspaces",
    "create_workspace",
    "create_workspace_invitation",
    "join_workspace",
    "current_membership",
];

async fn blocking<T, F>(state: &DesktopState, operation: F) -> Result<T, BridgeError>
where
    T: Send + 'static,
    F: FnOnce(Arc<resonance_core::CoreRuntime>) -> Result<T, BridgeError> + Send + 'static,
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
    state: State<'_, DesktopState>,
) -> Result<WorkspaceStateDto, BridgeError> {
    blocking(&state, move |runtime| {
        let workspace = runtime.create_workspace(&name).map_err(BridgeError::from)?;
        let membership = runtime
            .membership_snapshot(workspace.id())
            .map_err(BridgeError::from)?;
        Ok(WorkspaceStateDto {
            workspace: workspace.into(),
            membership: membership.into(),
        })
    })
    .await
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
    state: State<'_, DesktopState>,
) -> Result<WorkspaceStateDto, BridgeError> {
    let invitation =
        WorkspaceInvitation::from_str(&invitation).map_err(|_| BridgeError::invalid_input())?;
    let runtime = state.runtime()?;
    let (workspace, membership) = runtime
        .join_workspace(&invitation)
        .await
        .map_err(BridgeError::from)?;
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
