const COMMANDS: &[&str] = &[
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

fn production_release_inputs() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    const ENDPOINT: &str =
        "https://github.com/learn-ai-engineering/teamarness/releases/latest/download/latest.json";
    println!("cargo:rerun-if-env-changed=TAURI_CONFIG");
    let production = std::env::var_os("CARGO_FEATURE_PRODUCTION_RELEASE").is_some()
        && std::env::var("DEBUG").as_deref() == Ok("false");
    println!("cargo:rustc-env=TEAMARNESS_PRODUCTION_RELEASE={production}");
    if !production {
        return;
    }
    // Only reviewed build inputs compile into the client. Never include key/config values in errors.
    fn invalid() -> ! {
        panic!(
            "production release requires reviewed release configuration and a valid public updater key (values redacted)"
        );
    }
    let configuration: serde_json::Value = std::env::var("TAURI_CONFIG")
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| invalid());
    let updater = &configuration["plugins"]["updater"];
    let key = updater["pubkey"].as_str().unwrap_or_else(|| invalid());
    let decoded = STANDARD.decode(key.trim()).unwrap_or_else(|_| invalid());
    let decoded = std::str::from_utf8(&decoded).unwrap_or_else(|_| invalid());
    let lines: Vec<_> = decoded.lines().collect();
    if lines.len() != 2 || !lines[0].starts_with("untrusted comment: ") {
        invalid();
    }
    let packet = STANDARD.decode(lines[1]).unwrap_or_else(|_| invalid());
    if packet.len() != 42 || &packet[..2] != b"Ed" {
        invalid();
    }
    if updater["requireSignedVersion"] != true
        || updater["endpoints"] != serde_json::json!([ENDPOINT])
        || configuration["bundle"]["createUpdaterArtifacts"] != true
        || configuration["bundle"]["macOS"]["signingIdentity"] != "-"
        || std::env::var("TARGET").as_deref() != Ok("aarch64-apple-darwin")
    {
        invalid();
    }
    println!(
        "cargo:rustc-env=TEAMARNESS_UPDATE_PUBLIC_KEY={}",
        key.trim()
    );
    println!("cargo:rustc-env=TEAMARNESS_UPDATE_ENDPOINT={ENDPOINT}");
}

fn main() {
    production_release_inputs();
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
