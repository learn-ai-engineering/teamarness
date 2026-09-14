#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::symlink;

use sha2::{Digest, Sha256};
use tempfile::TempDir;

pub use teamarness_package_host::{PackageErrorCode, PackageIdentity};

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
        br#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"kind":{"type":"string"}},"required":["kind"],"additionalProperties":false}"#,
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
    assert!(ui::compile_contract(br#"{"type":"object","type":"array"}"#).is_err());
    for schema in [
        serde_json::json!({"$ref": "https://example.com/schema.json"}),
        serde_json::json!({"$dynamicRef": "#node"}),
        serde_json::json!({"$defs": {"node": {"$ref": "#/$defs/node"}}, "$ref": "#/$defs/node"}),
    ] {
        assert!(ui::compile_contract(&serde_json::to_vec(&schema).unwrap()).is_err());
    }
    assert!(
        ui::compile_contract(br#"{"type":"object","properties":{"value":{"type":"string"}}}"#)
            .is_ok()
    );
}
