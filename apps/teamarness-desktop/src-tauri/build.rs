const COMMANDS: &[&str] = &[
    "installation_details",
    "list_workspaces",
    "create_workspace",
    "create_workspace_invitation",
    "join_workspace",
    "current_membership",
    "person_name_snapshot",
    "subscribe_person_names",
    "set_own_person_name",
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

fn main() {
    let frame_runtime = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../frame-dist/package-frame-runtime.js");
    println!("cargo:rerun-if-changed={}", frame_runtime.display());
    if !frame_runtime.is_file() {
        panic!(
            "the Package UI frame runtime is missing at {}\n\
             hint: the Package UI frame runtime is built by npm. \
             Run `npm ci && npm run build` from the repository root \
             (or `npm --workspace teamarness-desktop run build:frame-runtime`) \
             before cargo builds of teamarness-desktop, or use `--exclude teamarness-desktop`.",
            frame_runtime.display()
        );
    }
    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS));
    if let Err(error) = tauri_build::try_build(attributes) {
        panic!(
            "failed to build Tauri application metadata: {error:#}\n\
             hint: the bundled Application Packages under target/teamarness-packages are staged by npm. \
             Run `npm ci && npm run build` from the repository root before cargo builds of teamarness-desktop, \
             or use `--exclude teamarness-desktop` for Rust-only workspace checks."
        );
    }
}
