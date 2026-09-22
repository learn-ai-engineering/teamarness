//! Manifest rules, pinned by code and rule name, plus the Package Directory path.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tempfile::TempDir;

use teamarness_package_contract::manifest::{load_package_directory, parse_manifest, sha256};
use teamarness_package_contract::{ContractError, ContractErrorCode};

const TARGET: &str = "aarch64-apple-darwin";

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn manifest(patch: Value) -> Value {
    let mut base = json!({
        "schemaVersion": 1,
        "identity": "dev.example.fixture",
        "version": "1.2.3",
        "presentation": {"displayName": "Fixture"},
        "protocol": {"major": 1, "minMinor": 0, "maxMinor": 0},
        "backends": [{
            "target": TARGET,
            "path": "backend/package",
            "size": 7,
            "sha256": "0".repeat(64),
        }],
        "ui": {
            "entrypoint": {"path": "ui/index.js", "size": 7, "sha256": "0".repeat(64)},
            "assets": [],
            "contract": {"path": "ui/contract.json", "sha256": "0".repeat(64)},
        },
        "capabilities": [],
    });
    let (Value::Object(base_object), Value::Object(patch)) = (&mut base, patch) else {
        return base;
    };
    for (key, value) in patch {
        base_object.insert(key, value);
    }
    base
}

fn reject(patch: Value) -> ContractError {
    parse_manifest(&serde_json::to_vec(&manifest(patch)).unwrap()).unwrap_err()
}

#[test]
fn accepts_bundled_package_manifests() {
    let packages = repository_root().join("packages");
    let mut seen = 0;
    for entry in fs::read_dir(&packages).unwrap() {
        let path = entry.unwrap().path().join("teamarness.package.json");
        if !path.is_file() {
            continue;
        }
        let bytes = fs::read(&path).unwrap();
        let document = parse_manifest(&bytes)
            .unwrap_or_else(|error| panic!("{} rejected: {error}", path.display()));
        assert!(
            document.backend_path(TARGET).is_some(),
            "{}",
            path.display()
        );
        seen += 1;
    }
    assert_eq!(seen, 4, "expected four bundled Package manifests");
}

/// The schema's `maxLength` counts code points, so the manifest rule does too.
///
/// Counting bytes rejected a display name the schema admits: 128 characters
/// outside ASCII are more than 128 bytes.
#[test]
fn a_display_name_is_bounded_in_code_points_as_the_schema_publishes_it() {
    let longest = "é".repeat(128);
    assert_eq!(longest.chars().count(), 128);
    assert_eq!(longest.len(), 256);
    let document = parse_manifest(
        &serde_json::to_vec(&manifest(
            json!({"presentation": {"displayName": longest.clone()}}),
        ))
        .unwrap(),
    )
    .expect("the schema admits 128 code points");
    assert_eq!(document.display_name, longest);

    let error = reject(json!({"presentation": {"displayName": "é".repeat(129)}}));
    assert_eq!(error.code, ContractErrorCode::ManifestMalformed);
    assert_eq!(error.rule, "manifest.display_name");
}

#[test]
fn accepts_placeholder_sizes_and_digests_in_a_source_tree() {
    let document = parse_manifest(
        &serde_json::to_vec(&manifest(json!({
            "backends": [{
                "target": TARGET,
                "path": "backend/package",
                "size": 0,
                "sha256": "0".repeat(64),
            }],
        })))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(document.backend_path(TARGET), Some("backend/package"));
}

#[test]
fn rejects_non_strict_versions() {
    for version in ["1.2", "1.02.3", "v1.2.3", "1.2.3+", ""] {
        let error = reject(json!({"version": version}));
        assert_eq!(
            error.code,
            ContractErrorCode::ManifestUnsupported,
            "{version}"
        );
        assert_eq!(error.rule, "manifest.version_not_strict", "{version}");
    }
}

#[test]
fn rejects_unsupported_protocol_ranges() {
    for protocol in [
        json!({"major": 2, "minMinor": 0, "maxMinor": 0}),
        json!({"major": 1, "minMinor": 1, "maxMinor": 2}),
        json!({"major": 0, "minMinor": 0, "maxMinor": 0}),
        json!({"major": 1, "minMinor": 2, "maxMinor": 1}),
    ] {
        let error = reject(json!({"protocol": protocol.clone()}));
        assert_eq!(
            error.code,
            ContractErrorCode::ManifestUnsupported,
            "{protocol}"
        );
        assert_eq!(error.rule, "manifest.protocol_unsupported", "{protocol}");
        assert_eq!(error.pointer.as_deref(), Some("/protocol"));
    }
}

#[test]
fn rejects_unnormalized_paths() {
    for path in [
        "/absolute",
        "../escape",
        "ui/./index.js",
        "ui\\index.js",
        "ui//index.js",
        "ui/",
        "",
    ] {
        let error = reject(json!({"ui": {
            "entrypoint": {"path": path, "size": 7, "sha256": "0".repeat(64)},
            "assets": [],
            "contract": {"path": "ui/contract.json", "sha256": "0".repeat(64)},
        }}));
        assert_eq!(error.code, ContractErrorCode::ManifestMalformed, "{path}");
        assert_eq!(error.rule, "manifest.path_not_normalized", "{path}");
    }
}

#[test]
fn rejects_invalid_extension_keys() {
    let error = reject(json!({"extensions": {"not-reverse-dns": {"a": 1}}}));
    assert_eq!(error.code, ContractErrorCode::ManifestMalformed);
    assert_eq!(error.rule, "manifest.extension_key");
}

#[test]
fn rejects_duplicate_targets_and_paths() {
    let artifact = json!({
        "target": TARGET, "path": "backend/package", "size": 7, "sha256": "0".repeat(64),
    });
    let error = reject(json!({"backends": [artifact.clone(), artifact]}));
    assert_eq!(error.code, ContractErrorCode::TargetUnsupported);
    assert_eq!(error.rule, "manifest.duplicate_target");

    let error = reject(json!({"backends": [
        {"target": TARGET, "path": "backend/package", "size": 7, "sha256": "0".repeat(64)},
        {"target": "x86_64-apple-darwin", "path": "backend/package", "size": 7, "sha256": "0".repeat(64)},
    ]}));
    assert_eq!(error.code, ContractErrorCode::ManifestMalformed);
    assert_eq!(error.rule, "manifest.duplicate_path");
}

fn write_directory(root: &Path, backend: &[u8], entrypoint: &[u8], contract: &[u8]) {
    fs::create_dir_all(root.join("backend")).unwrap();
    fs::create_dir_all(root.join("ui")).unwrap();
    fs::write(root.join("backend/package"), backend).unwrap();
    fs::write(root.join("ui/index.js"), entrypoint).unwrap();
    fs::write(root.join("ui/contract.json"), contract).unwrap();
    let document = manifest(json!({
        "backends": [{
            "target": TARGET, "path": "backend/package",
            "size": backend.len(), "sha256": sha256(backend),
        }],
        "ui": {
            "entrypoint": {"path": "ui/index.js", "size": entrypoint.len(), "sha256": sha256(entrypoint)},
            "assets": [],
            "contract": {"path": "ui/contract.json", "sha256": sha256(contract)},
        },
    }));
    fs::write(
        root.join("teamarness.package.json"),
        serde_json::to_vec(&document).unwrap(),
    )
    .unwrap();
}

#[test]
fn loads_a_package_directory_and_reports_a_digest_mismatch() {
    let contract = br#"{"schemaVersion":1,"$defs":{},"requests":{},"subscriptions":{}}"#;
    let temp = TempDir::new().unwrap();
    write_directory(temp.path(), b"backend", b"export {};", contract);
    let loaded = load_package_directory(temp.path(), TARGET).unwrap();
    assert_eq!(loaded.identity, "dev.example.fixture");
    assert_eq!(loaded.backend.relative_path, "backend/package");

    fs::write(temp.path().join("backend/package"), b"changed").unwrap();
    let error = load_package_directory(temp.path(), TARGET).unwrap_err();
    assert_eq!(error.code, ContractErrorCode::ArtifactChanged);
    assert_eq!(error.rule, "manifest.digest_mismatch");
    assert_eq!(error.pointer.as_deref(), Some("backend/package"));
}

#[test]
fn reports_a_missing_target_and_a_missing_artifact() {
    let contract = br#"{"schemaVersion":1,"$defs":{},"requests":{},"subscriptions":{}}"#;
    let temp = TempDir::new().unwrap();
    write_directory(temp.path(), b"backend", b"export {};", contract);

    let error = load_package_directory(temp.path(), "riscv64-unknown-linux-gnu").unwrap_err();
    assert_eq!(error.code, ContractErrorCode::TargetUnsupported);
    assert_eq!(error.rule, "manifest.target_missing");

    fs::remove_file(temp.path().join("backend/package")).unwrap();
    let error = load_package_directory(temp.path(), TARGET).unwrap_err();
    assert_eq!(error.code, ContractErrorCode::ArtifactMissing);
    assert_eq!(error.rule, "manifest.artifact_missing");
}
