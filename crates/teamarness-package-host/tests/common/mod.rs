#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use teamarness_core::{CoreRuntime, CoreRuntimeConfig};
use teamarness_package_host::{InstallationPublicIdentity, PackageHostConfig};
use tempfile::TempDir;

pub struct Fixture {
    pub _root: TempDir,
    pub _private: PathBuf,
    pub core: Arc<CoreRuntime>,
    pub config: PackageHostConfig,
}

impl Fixture {
    pub async fn node(packages: &[(&str, &str)]) -> Self {
        Self::node_with_capabilities(
            packages,
            &[serde_json::json!({
                "identity":"teamarness.workspace-context", "major":1,
                "minMinor":0, "maxMinor":0
            })],
        )
        .await
    }

    pub async fn node_with_capabilities(
        packages: &[(&str, &str)],
        capabilities: &[serde_json::Value],
    ) -> Self {
        let root = tempfile::tempdir().unwrap();
        let application = root.path().join("application");
        let private = root.path().join("private");
        let core_dir = root.path().join("core");
        fs::create_dir_all(&application).unwrap();
        fs::create_dir_all(&private).unwrap();
        let mut entries = Vec::new();
        for (directory, identity) in packages {
            write_node_package_with_capabilities(
                &application.join(directory),
                identity,
                capabilities,
            );
            entries.push(serde_json::json!({"directory": directory}));
        }
        let index = application.join("application-packages.v1.json");
        fs::write(
            &index,
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1, "packages": entries
            }))
            .unwrap(),
        )
        .unwrap();
        let core = Arc::new(
            CoreRuntime::open(CoreRuntimeConfig::new(core_dir))
                .await
                .unwrap(),
        );
        core.create_workspace("Fixture Workspace").unwrap();
        let config = PackageHostConfig::new(
            &application,
            index,
            &private,
            InstallationPublicIdentity::new(core.public_identity().to_string()).unwrap(),
        );
        Self {
            _root: root,
            _private: private,
            core,
            config,
        }
    }
}

pub fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn write_node_package(root: &Path, identity: &str) {
    write_node_package_with_capabilities(
        root,
        identity,
        &[serde_json::json!({
            "identity":"teamarness.workspace-context", "major":1,
            "minMinor":0, "maxMinor":0
        })],
    );
}

pub fn write_node_package_with_capabilities(
    root: &Path,
    identity: &str,
    capabilities: &[serde_json::Value],
) {
    fs::create_dir_all(root.join("backend")).unwrap();
    fs::create_dir_all(root.join("ui")).unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/package-process/v1/fixtures/node-package-process.mjs");
    let backend = fs::read(source).unwrap();
    let backend_path = root.join("backend/package.mjs");
    fs::write(&backend_path, &backend).unwrap();
    fs::set_permissions(&backend_path, fs::Permissions::from_mode(0o700)).unwrap();
    let entrypoint = b"export function mount() { return async () => {}; }";
    let contract = serde_json::to_vec(&serde_json::json!({
        "schemaVersion": 1,
        "$defs": {},
        "requests": {
            "fixture.abort": {"request": {}, "response": {}},
            "fixture.capability": {"request": {}, "response": {}},
            "fixture.delayed": {"request": {}, "response": {}},
            "fixture.error": {"request": {}, "response": {}},
            "fixture.grant": {"request": {}, "response": {}},
            "fixture.immediate": {"request": {}, "response": {}},
            "fixture.immutable_append": {"request": {}, "response": {}},
            "fixture.immutable_close": {"request": {}, "response": {}},
            "fixture.immutable_credit": {"request": {}, "response": {}},
            "fixture.immutable_events": {"request": {}, "response": {}},
            "fixture.immutable_open": {"request": {}, "response": {}},
            "fixture.immutable_subscribe": {"request": {}, "response": {}},
            "fixture.invalid_response": {"request": {}, "response": {"type":"object","properties":{"value":{"type":"string"}},"required":["value"],"additionalProperties":false}},
            "fixture.malformed": {"request": {}, "response": {}},
            "fixture.membership_close": {"request": {}, "response": {}},
            "fixture.membership_credit": {"request": {}, "response": {}},
            "fixture.membership_events": {"request": {}, "response": {}},
            "fixture.membership_open": {"request": {}, "response": {}},
            "fixture.stderr": {"request": {}, "response": {}},
            "fixture.typed": {"request": {"type":"object","properties":{"value":{"type":"string"}},"required":["value"],"additionalProperties":false}, "response": {}}
        },
        "subscriptions": {
            "fixture.close": {"request": {}, "event": {}},
            "fixture.duplicate": {"request": {}, "event": {}},
            "fixture.events": {"request": {}, "event": {}},
            "fixture.gap": {"request": {}, "event": {}},
            "fixture.invalid_event": {"request": {}, "event": {"type":"object","properties":{"order":{"type":"integer"}},"required":["order"],"additionalProperties":false}},
            "fixture.overflow": {"request": {}, "event": {}},
            "fixture.rejected_stream": {"request": {}, "event": {}},
            "fixture.saturation": {"request": {}, "event": {}}
        }
    }))
    .unwrap();
    fs::write(root.join("ui/index.js"), entrypoint).unwrap();
    fs::write(root.join("ui/icon.svg"), b"<svg/>").unwrap();
    fs::write(root.join("ui/contract.json"), &contract).unwrap();
    fs::write(root.join("teamarness.package.json"), serde_json::to_vec(&serde_json::json!({
        "schemaVersion": 1,
        "identity": identity,
        "version": "1.0.0",
        "presentation": {"displayName": identity},
        "protocol": {"major": 1, "minMinor": 0, "maxMinor": 0},
        "backends": [{
            "target":"aarch64-apple-darwin", "path":"backend/package.mjs",
            "size":backend.len(), "sha256":digest(&backend)
        }],
        "ui": {
            "entrypoint":{"path":"ui/index.js","size":entrypoint.len(),"sha256":digest(entrypoint)},
            "assets":[{"path":"ui/icon.svg","mediaType":"image/svg+xml","size":6,"sha256":digest(b"<svg/>")}],
            "contract":{"path":"ui/contract.json","sha256":digest(&contract)}
        },
        "capabilities": capabilities
    })).unwrap()).unwrap();
}

/// Issues one Workspace invitation, waiting out endpoint readiness.
///
/// Readiness is a real-network condition, not the behaviour any Host test is
/// about, so the deadline only bounds failure.
pub async fn invite(
    core: &CoreRuntime,
    workspace: teamarness_core::WorkspaceId,
) -> teamarness_core::WorkspaceInvitation {
    let expiry = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match core.create_workspace_invitation(workspace).await {
            Ok(invitation) => return invitation,
            Err(teamarness_core::OnboardingError::Readiness)
                if std::time::Instant::now() < expiry =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(error) => panic!("the invitation could not be issued: {error}"),
        }
    }
}
