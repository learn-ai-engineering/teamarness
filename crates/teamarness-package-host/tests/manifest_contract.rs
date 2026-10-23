#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::symlink;

use sha2::{Digest, Sha256};
use tempfile::TempDir;

pub use teamarness_package_host::{PackageErrorCode, PackageIdentity, PackageValue};

#[path = "../src/manifest.rs"]
mod manifest;
#[path = "../src/ui.rs"]
mod ui;

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn valid_package() -> TempDir {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("backend")).unwrap();
    fs::create_dir(temp.path().join("ui")).unwrap();
    fs::write(temp.path().join("backend/package"), b"backend").unwrap();
    fs::write(
        temp.path().join("ui/index.js"),
        b"export function mount() {}",
    )
    .unwrap();
    fs::write(temp.path().join("ui/icon.svg"), b"<svg/>").unwrap();
    fs::write(
        temp.path().join("ui/contract.json"),
        br##"{"schemaVersion":1,"$defs":{"item":{"type":"object","properties":{"kind":{"type":"string"}},"required":["kind"],"additionalProperties":false}},"requests":{"reference.read":{"request":{"$ref":"#/$defs/item"},"response":{"$ref":"#/$defs/item"}}},"subscriptions":{"reference.changes":{"request":{"$ref":"#/$defs/item"},"event":{"$ref":"#/$defs/item"}}}}"##,
    )
    .unwrap();
    write_manifest(&temp, serde_json::json!({}));
    temp
}

fn write_manifest(temp: &TempDir, patch: serde_json::Value) {
    let backend = fs::read(temp.path().join("backend/package")).unwrap();
    let entrypoint = fs::read(temp.path().join("ui/index.js")).unwrap();
    let asset = fs::read(temp.path().join("ui/icon.svg")).unwrap();
    let contract = fs::read(temp.path().join("ui/contract.json")).unwrap();
    let mut value = serde_json::json!({
        "schemaVersion": 1,
        "identity": "dev.teamarness.reference",
        "version": "1.2.3",
        "presentation": { "displayName": "Reference" },
        "protocol": { "major": 1, "minMinor": 0, "maxMinor": 0 },
        "backends": [{
            "target": "aarch64-apple-darwin",
            "path": "backend/package",
            "size": backend.len(),
            "sha256": digest(&backend)
        }],
        "ui": {
            "entrypoint": {
                "path": "ui/index.js",
                "size": entrypoint.len(),
                "sha256": digest(&entrypoint)
            },
            "assets": [{
                "path": "ui/icon.svg",
                "mediaType": "image/svg+xml",
                "size": asset.len(),
                "sha256": digest(&asset)
            }],
            "contract": { "path": "ui/contract.json", "sha256": digest(&contract) }
        },
        "capabilities": [{
            "identity": "teamarness.membership",
            "major": 1,
            "minMinor": 0,
            "maxMinor": 0
        }],
        "extensions": { "dev.example.build": { "channel": "local" } }
    });
    merge(&mut value, patch);
    fs::write(
        temp.path().join("teamarness.package.json"),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .unwrap();
}

fn write_ui_manifest(temp: &TempDir, entrypoint: &[u8], assets: &[(&str, &[u8])], contract: &[u8]) {
    fs::write(temp.path().join("ui/index.js"), entrypoint).unwrap();
    fs::write(temp.path().join("ui/contract.json"), contract).unwrap();
    let assets = assets
        .iter()
        .map(|(name, bytes)| {
            fs::write(temp.path().join("ui").join(name), bytes).unwrap();
            serde_json::json!({
                "path": format!("ui/{name}"),
                "mediaType": "application/octet-stream",
                "size": bytes.len(),
                "sha256": digest(bytes),
            })
        })
        .collect::<Vec<_>>();
    let backend = fs::read(temp.path().join("backend/package")).unwrap();
    fs::write(
        temp.path().join("teamarness.package.json"),
        serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1,
            "identity": "dev.teamarness.reference",
            "version": "1.2.3",
            "presentation": {"displayName": "Reference"},
            "protocol": {"major": 1, "minMinor": 0, "maxMinor": 0},
            "backends": [{
                "target": "aarch64-apple-darwin", "path": "backend/package",
                "size": backend.len(), "sha256": digest(&backend)
            }],
            "ui": {
                "entrypoint": {"path":"ui/index.js","size":entrypoint.len(),"sha256":digest(entrypoint)},
                "assets": assets,
                "contract": {"path":"ui/contract.json","sha256":digest(contract)}
            },
            "capabilities": []
        }))
        .unwrap(),
    )
    .unwrap();
}

fn merge(base: &mut serde_json::Value, patch: serde_json::Value) {
    let serde_json::Value::Object(patch) = patch else {
        return;
    };
    let serde_json::Value::Object(base) = base else {
        return;
    };
    for (key, value) in patch {
        base.insert(key, value);
    }
}

fn code(temp: &TempDir) -> teamarness_package_host::PackageErrorCode {
    manifest::load(temp.path(), "aarch64-apple-darwin")
        .unwrap_err()
        .code
}

#[test]
fn reserved_principal_identity_cannot_be_selected_by_manifest() {
    let temp = valid_package();
    write_manifest(
        &temp,
        serde_json::json!({"identity": "internal:teamarness.principals"}),
    );
    assert_eq!(code(&temp), PackageErrorCode::InvalidPackageIdentity);
    assert!(
        !teamarness_package_contract::identity::is_valid_package_identity(
            "internal:teamarness.principals"
        )
    );
    assert!(
        "internal:teamarness.principals"
            .parse::<PackageIdentity>()
            .is_err()
    );
    assert!(
        teamarness_package_contract::identity::is_valid_package_identity(
            "internal.teamarness.principals"
        )
    );
    assert!(
        "internal.teamarness.principals"
            .parse::<PackageIdentity>()
            .is_ok()
    );
    write_manifest(
        &temp,
        serde_json::json!({"identity": "internal.teamarness.principals"}),
    );
    assert_eq!(
        manifest::load(temp.path(), "aarch64-apple-darwin")
            .unwrap()
            .identity
            .as_str(),
        "internal.teamarness.principals"
    );
}

#[test]
fn valid_manifest_selects_exact_target_and_compiles_contract() {
    let temp = valid_package();
    let loaded = manifest::load(temp.path(), "aarch64-apple-darwin").unwrap();
    assert_eq!(loaded.identity.as_str(), "dev.teamarness.reference");
    assert_eq!(loaded.version.to_string(), "1.2.3");
    assert_eq!(loaded.backend.relative_path, "backend/package");
    assert_eq!(loaded.ui.assets.len(), 1);
    assert_eq!(loaded.capabilities.len(), 1);
    ui::compile_contract(&loaded.ui.contract.bytes).unwrap();
}

#[test]
fn strict_json_and_manifest_versions_have_stable_codes() {
    let temp = valid_package();
    fs::write(
        temp.path().join("teamarness.package.json"),
        br#"{"schemaVersion":1,"schemaVersion":1}"#,
    )
    .unwrap();
    assert_eq!(
        code(&temp),
        teamarness_package_host::PackageErrorCode::ManifestMalformed
    );

    let temp = valid_package();
    write_manifest(&temp, serde_json::json!({"unknown": true}));
    assert_eq!(
        code(&temp),
        teamarness_package_host::PackageErrorCode::ManifestMalformed
    );

    let temp = valid_package();
    write_manifest(&temp, serde_json::json!({"schemaVersion": 2}));
    assert_eq!(
        code(&temp),
        teamarness_package_host::PackageErrorCode::ManifestUnsupported
    );

    let temp = valid_package();
    write_manifest(&temp, serde_json::json!({"version": "1.2"}));
    assert_eq!(
        code(&temp),
        teamarness_package_host::PackageErrorCode::ManifestUnsupported
    );
}

#[test]
fn identity_ranges_and_target_selection_are_strict() {
    for (patch, expected) in [
        (
            serde_json::json!({"identity": "Bad.identity"}),
            teamarness_package_host::PackageErrorCode::InvalidPackageIdentity,
        ),
        (
            serde_json::json!({"identity": "dev.example-"}),
            teamarness_package_host::PackageErrorCode::InvalidPackageIdentity,
        ),
        (
            serde_json::json!({"protocol": {"major": 2, "minMinor": 0, "maxMinor": 0}}),
            teamarness_package_host::PackageErrorCode::ManifestUnsupported,
        ),
        (
            serde_json::json!({"protocol": {"major": 1, "minMinor": 2, "maxMinor": 1}}),
            teamarness_package_host::PackageErrorCode::ManifestUnsupported,
        ),
        (
            serde_json::json!({"backends": []}),
            teamarness_package_host::PackageErrorCode::TargetUnsupported,
        ),
    ] {
        let temp = valid_package();
        write_manifest(&temp, patch);
        assert_eq!(code(&temp), expected);
    }

    let temp = valid_package();
    let backend = fs::read(temp.path().join("backend/package")).unwrap();
    let artifact = serde_json::json!({
        "target": "aarch64-apple-darwin", "path": "backend/package",
        "size": backend.len(), "sha256": digest(&backend)
    });
    write_manifest(
        &temp,
        serde_json::json!({"backends": [artifact.clone(), artifact]}),
    );
    assert_eq!(
        code(&temp),
        teamarness_package_host::PackageErrorCode::TargetUnsupported
    );
}

#[test]
fn paths_symlinks_sizes_digests_and_media_types_are_rejected() {
    for path in [
        "/absolute",
        "../escape",
        "ui/./index.js",
        "ui\\index.js",
        "ui//index.js",
        "ui/",
    ] {
        let temp = valid_package();
        write_manifest(
            &temp,
            serde_json::json!({"ui": {
                "entrypoint": {"path": path, "size": 26, "sha256": "00".repeat(32)},
                "assets": [],
                "contract": {"path": "ui/contract.json", "sha256": digest(&fs::read(temp.path().join("ui/contract.json")).unwrap())}
            }}),
        );
        assert_eq!(
            code(&temp),
            teamarness_package_host::PackageErrorCode::ManifestMalformed,
            "{path}"
        );
    }

    let temp = valid_package();
    fs::remove_file(temp.path().join("backend/package")).unwrap();
    assert_eq!(
        code(&temp),
        teamarness_package_host::PackageErrorCode::ArtifactMissing
    );

    let temp = valid_package();
    fs::write(temp.path().join("backend/package"), b"changed").unwrap();
    assert_eq!(
        code(&temp),
        teamarness_package_host::PackageErrorCode::ArtifactChanged
    );

    let temp = valid_package();
    fs::remove_file(temp.path().join("ui/index.js")).unwrap();
    symlink("icon.svg", temp.path().join("ui/index.js")).unwrap();
    assert_eq!(
        code(&temp),
        teamarness_package_host::PackageErrorCode::UiEntrypointMissing
    );

    let temp = valid_package();
    write_manifest(
        &temp,
        serde_json::json!({"ui": {
            "entrypoint": {"path": "ui/index.js", "size": 26, "sha256": digest(&fs::read(temp.path().join("ui/index.js")).unwrap())},
            "assets": [{"path": "ui/icon.svg", "mediaType": "not a mime", "size": 6, "sha256": digest(b"<svg/>")}],
            "contract": {"path": "ui/contract.json", "sha256": digest(&fs::read(temp.path().join("ui/contract.json")).unwrap())}
        }}),
    );
    assert_eq!(
        code(&temp),
        teamarness_package_host::PackageErrorCode::ManifestMalformed
    );
}

#[test]
fn ui_file_count_byte_utf8_and_total_limits_are_enforced() {
    let contract = br#"{"schemaVersion":1,"$defs":{},"requests":{},"subscriptions":{}}"#;

    let temp = valid_package();
    write_ui_manifest(&temp, &vec![b'x'; 8 * 1024 * 1024], &[], contract);
    assert!(manifest::load(temp.path(), "aarch64-apple-darwin").is_ok());
    let temp = valid_package();
    write_ui_manifest(&temp, &vec![b'x'; 8 * 1024 * 1024 + 1], &[], contract);
    assert_eq!(code(&temp), PackageErrorCode::ManifestMalformed);
    let temp = valid_package();
    write_ui_manifest(&temp, &[0xff], &[], contract);
    assert_eq!(code(&temp), PackageErrorCode::ManifestMalformed);

    let temp = valid_package();
    let large_asset = vec![0_u8; 8 * 1024 * 1024 + 1];
    write_ui_manifest(
        &temp,
        b"export {};",
        &[("large.bin", &large_asset)],
        contract,
    );
    assert_eq!(code(&temp), PackageErrorCode::ManifestMalformed);

    let temp = valid_package();
    let names = (0..65)
        .map(|index| format!("asset-{index}.bin"))
        .collect::<Vec<_>>();
    let bytes = [0_u8];
    let assets = names
        .iter()
        .map(|name| (name.as_str(), bytes.as_slice()))
        .collect::<Vec<_>>();
    write_ui_manifest(&temp, b"export {};", &assets[..64], contract);
    assert!(manifest::load(temp.path(), "aarch64-apple-darwin").is_ok());
    write_ui_manifest(&temp, b"export {};", &assets, contract);
    assert_eq!(code(&temp), PackageErrorCode::ManifestMalformed);

    let temp = valid_package();
    let part = vec![0_u8; 8 * 1024 * 1024];
    let final_part = vec![0_u8; 8 * 1024 * 1024 - contract.len()];
    write_ui_manifest(
        &temp,
        &part,
        &[
            ("one.bin", &part),
            ("two.bin", &part),
            ("three.bin", &final_part),
        ],
        contract,
    );
    assert!(manifest::load(temp.path(), "aarch64-apple-darwin").is_ok());
    write_ui_manifest(
        &temp,
        &part,
        &[("one.bin", &part), ("two.bin", &part), ("three.bin", &part)],
        contract,
    );
    assert_eq!(code(&temp), PackageErrorCode::ManifestMalformed);

    let temp = valid_package();
    let mut maximum_contract = contract.to_vec();
    maximum_contract.resize(1024 * 1024, b' ');
    write_ui_manifest(&temp, b"export {};", &[], &maximum_contract);
    let loaded = manifest::load(temp.path(), "aarch64-apple-darwin").unwrap();
    assert!(ui::compile_contract(&loaded.ui.contract.bytes).is_ok());
    maximum_contract.push(b' ');
    write_ui_manifest(&temp, b"export {};", &[], &maximum_contract);
    assert_eq!(code(&temp), PackageErrorCode::UiContractViolation);
}

#[test]
fn normative_source_schemas_compile_and_classify_fixtures() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/packages/v1");
    for name in [
        "manifest",
        "application-index",
        "registrations",
        "ui-contract-profile",
    ] {
        let schema: serde_json::Value = serde_json::from_slice(
            &fs::read(root.join(format!("schemas/{name}.schema.json"))).unwrap(),
        )
        .unwrap();
        jsonschema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .build(&schema)
            .unwrap();
    }
    let manifest_schema: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("schemas/manifest.schema.json")).unwrap())
            .unwrap();
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&manifest_schema)
        .unwrap();
    let valid: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("fixtures/manifest.valid.json")).unwrap())
            .unwrap();
    let invalid: serde_json::Value = serde_json::from_slice(
        &fs::read(root.join("fixtures/manifest.invalid-unknown-field.json")).unwrap(),
    )
    .unwrap();
    assert!(validator.is_valid(&valid));
    assert!(!validator.is_valid(&invalid));
}

#[test]
fn restricted_contract_rejects_remote_executable_and_recursive_schemas() {
    for contract in [
        serde_json::json!({"schemaVersion":1,"$defs":{},"requests":{},"subscriptions":{},"unknown":true}),
        serde_json::json!({"schemaVersion":1,"$defs":{"node":{"$ref":"https://example.com/schema.json"}},"requests":{},"subscriptions":{}}),
        serde_json::json!({"schemaVersion":1,"$defs":{"node":{"$dynamicRef":"#node"}},"requests":{},"subscriptions":{}}),
        serde_json::json!({"schemaVersion":1,"$defs":{"node":{"$ref":"#/$defs/node"}},"requests":{},"subscriptions":{}}),
    ] {
        assert!(ui::compile_contract(&serde_json::to_vec(&contract).unwrap()).is_err());
    }
    assert!(
        ui::compile_contract(
            &fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../contracts/packages/v1/fixtures/ui-contract.valid.json"),
            )
            .unwrap()
        )
        .is_ok()
    );
}

// Verifies: TEAM.SYS.PKG-R12
#[test]
fn port_declarations_are_loaded_with_schema_metadata_and_digest_checks() {
    let temp = valid_package();
    let bytes = fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../contracts/package-ports/v1/fixtures/version-1-0.json"
    ))
    .unwrap();
    fs::write(temp.path().join("ports.json"), &bytes).unwrap();
    let consume = serde_json::json!([{"identity":"dev.example.conversations","major":1,"minMinor":0,"maxMinor":1,"ports":[{"name":"conversation.list","kind":"query"}]}]);
    write_manifest(
        &temp,
        serde_json::json!({"portContract":{"path":"ports.json","sha256":digest(&bytes)},"consumedPorts":consume}),
    );
    let loaded = manifest::load(temp.path(), "aarch64-apple-darwin").unwrap();
    assert_eq!(
        loaded.port_contract.as_ref().unwrap().contracts[0]
            .ports
            .len(),
        3
    );
    assert!(
        loaded.port_contract.as_ref().unwrap().contracts[0].ports[0]
            .request
            .is_some()
    );
    assert_eq!(loaded.consumed_ports[0].ports[0].name, "conversation.list");
    assert_eq!(
        loaded.port_contract_digest.as_deref(),
        Some(digest(&bytes).as_str())
    );

    fs::write(temp.path().join("ports.json"), b"{}").unwrap();
    assert_eq!(code(&temp), PackageErrorCode::ArtifactChanged);
    fs::write(temp.path().join("ports.json"), &bytes).unwrap();
    write_manifest(
        &temp,
        serde_json::json!({"portContract":{"path":"ui/contract.json","sha256":digest(&bytes)}}),
    );
    assert_eq!(code(&temp), PackageErrorCode::ManifestMalformed);
    write_manifest(
        &temp,
        serde_json::json!({"consumedPorts":[{"identity":"dev.example.conversations","major":1,"minMinor":0,"maxMinor":0,"ports":[{"name":"conversation.list","kind":"query"},{"name":"conversation.list","kind":"command"}]}]}),
    );
    assert_eq!(code(&temp), PackageErrorCode::ManifestMalformed);
}

// Verifies: TEAM.SYS.PKG-R12
#[tokio::test]
async fn authoring_catalog_is_static_complete_and_absent_from_ui_snapshot() {
    use std::sync::Arc;
    use teamarness_core::{CoreRuntime, CoreRuntimeConfig};
    use teamarness_package_host::{
        InstallationPublicIdentity, PackageHost, PackageHostCommand, PackageHostConfig,
        ShutdownHost,
    };

    let root = tempfile::tempdir().unwrap();
    let application = root.path().join("application");
    let private = root.path().join("private");
    fs::create_dir(&application).unwrap();
    fs::create_dir(&private).unwrap();
    let temp = valid_package();
    let mut document: serde_json::Value = serde_json::from_slice(
        &fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../contracts/package-ports/v1/fixtures/version-1-0.json"
        ))
        .unwrap(),
    )
    .unwrap();
    document["$defs"]["message"] = serde_json::json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false});
    document["contracts"][0]["ports"][0]["request"] = serde_json::json!({"$ref":"#/$defs/message"});
    let bytes = serde_json::to_vec(&document).unwrap();
    fs::write(temp.path().join("ports.json"), &bytes).unwrap();
    write_manifest(
        &temp,
        serde_json::json!({"portContract":{"path":"ports.json","sha256":digest(&bytes)},"consumedPorts":[{"identity":"dev.example.conversations","major":1,"minMinor":0,"maxMinor":1,"required":false,"ports":[{"name":"conversation.list","kind":"query"}]}]}),
    );
    fs::rename(temp.path(), application.join("one")).unwrap();
    let index = application.join("application-packages.v1.json");
    fs::write(
        &index,
        br#"{"schemaVersion":1,"packages":[{"directory":"one"}]}"#,
    )
    .unwrap();
    let core = Arc::new(
        CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(root.path().join("core")))
            .await
            .unwrap(),
    );
    let installation = InstallationPublicIdentity::new(core.public_identity().to_string()).unwrap();
    let host = PackageHost::open(
        PackageHostConfig::new(&application, &index, &private, installation),
        core,
    )
    .await
    .unwrap();
    let catalog = host.authoring_catalog();
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog[0].provided[0].ports.len(), 3);
    assert_eq!(catalog[0].definitions["message"]["type"], "object");
    assert!(catalog[0].provided[0].ports[0].request.is_some());
    let catalog_json = serde_json::to_value(&catalog).unwrap();
    assert_eq!(
        catalog_json[0]["provided"][0]["ports"][0]["request"]["$ref"],
        "#/$defs/message"
    );
    assert_eq!(catalog[0].consumed[0].ports[0].name, "conversation.list");
    assert_eq!(host.snapshot().await.packages.len(), 1);
    let snapshot_json = serde_json::to_string(&host.snapshot().await).unwrap();
    assert!(!snapshot_json.contains("conversation.send"));
    host.execute(PackageHostCommand::Shutdown(ShutdownHost {}))
        .await
        .unwrap();
}

// Verifies: TEAM.SYS.PKG-R12
#[test]
fn port_file_symlink_oversize_and_invalid_schema_fail_locally() {
    let temp = valid_package();
    let bytes = fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../contracts/package-ports/v1/fixtures/version-1-0.json"
    ))
    .unwrap();
    fs::write(temp.path().join("ports.json"), &bytes).unwrap();
    write_manifest(
        &temp,
        serde_json::json!({"portContract":{"path":"ports.json","sha256":digest(&bytes)}}),
    );
    fs::remove_file(temp.path().join("ports.json")).unwrap();
    symlink("ui/contract.json", temp.path().join("ports.json")).unwrap();
    assert_eq!(code(&temp), PackageErrorCode::PortContractViolation);
    fs::remove_file(temp.path().join("ports.json")).unwrap();
    fs::write(temp.path().join("ports.json"), vec![b' '; 1024 * 1024 + 1]).unwrap();
    assert_eq!(code(&temp), PackageErrorCode::PortContractViolation);
    let mut invalid: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    invalid["contracts"][0]["ports"][0]["request"] =
        serde_json::json!({"$ref":"https://example.com/invalid"});
    let invalid = serde_json::to_vec(&invalid).unwrap();
    fs::write(temp.path().join("ports.json"), &invalid).unwrap();
    write_manifest(
        &temp,
        serde_json::json!({"portContract":{"path":"ports.json","sha256":digest(&invalid)}}),
    );
    assert_eq!(code(&temp), PackageErrorCode::PortContractViolation);
}
