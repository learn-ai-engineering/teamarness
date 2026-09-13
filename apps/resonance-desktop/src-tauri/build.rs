const COMMANDS: &[&str] = &[
    "installation_details",
    "list_workspaces",
    "create_workspace",
    "create_workspace_invitation",
    "join_workspace",
    "current_membership",
];

fn main() {
    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS));
    tauri_build::try_build(attributes).expect("failed to build Tauri application metadata");
}
