//! The author validator, exercised through its binary.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Instant;

use serde_json::Value;
use tempfile::TempDir;

use teamarness_package_contract::manifest::sha256;

const BINARY: &str = env!("CARGO_BIN_EXE_teamarness-package-validate");
const TARGET: &str = "aarch64-apple-darwin";

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn validate(arguments: &[&str]) -> Output {
    Command::new(BINARY)
        .args(arguments)
        .output()
        .expect("running the validator")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Writes the template's manifest and UI contract into `root` with the generator's tokens filled in.
fn template_source(root: &Path, contract_patch: impl FnOnce(&mut Value)) {
    let template = repository_root().join("templates/package-foundation/v1");
    let manifest = fs::read_to_string(template.join("teamarness.package.json"))
        .unwrap()
        .replace("__PACKAGE_IDENTITY__", "dev.example.fixture")
        .replace("__PACKAGE_DISPLAY_NAME_JSON__", "\"Fixture\"");
    fs::write(root.join("teamarness.package.json"), manifest).unwrap();
    let contract = fs::read_to_string(template.join("ui-contract.json"))
        .unwrap()
        .replace("__REQUEST_NAMESPACE__", "fixture");
    let mut contract: Value = serde_json::from_str(&contract).unwrap();
    contract_patch(&mut contract);
    fs::write(
        root.join("ui-contract.json"),
        serde_json::to_vec_pretty(&contract).unwrap(),
    )
    .unwrap();
}

#[test]
fn source_mode_accepts_template_placeholders() {
    let temp = TempDir::new().unwrap();
    template_source(temp.path(), |_| {});
    let output = validate(&["source", &temp.path().to_string_lossy()]);
    assert!(output.status.success(), "{}", stdout(&output));
    assert!(stdout(&output).ends_with("ok\n"), "{}", stdout(&output));
}

#[test]
fn source_mode_rejects_remote_references_with_rule() {
    let temp = TempDir::new().unwrap();
    template_source(temp.path(), |contract| {
        contract["requests"]["fixture.read"]["response"] =
            serde_json::json!({"$ref": "https://example.com/x"});
    });
    let output = validate(&["source", &temp.path().to_string_lossy()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stdout(&output).contains(
            "error[ui_contract_violation/ui_contract.reference_not_local] ui-contract.json#"
        ),
        "{}",
        stdout(&output)
    );
}

#[test]
fn json_output_lists_errors() {
    let temp = TempDir::new().unwrap();
    template_source(temp.path(), |contract| {
        contract["requests"]["fixture.read"]["response"] =
            serde_json::json!({"$ref": "https://example.com/x"});
    });
    let output = validate(&["source", &temp.path().to_string_lossy(), "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let document: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(document["ok"], Value::Bool(false));
    let errors = document["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["code"], "ui_contract_violation");
    assert_eq!(errors[0]["rule"], "ui_contract.reference_not_local");
    assert_eq!(errors[0]["file"], "ui-contract.json");
    assert!(errors[0]["pointer"].is_string());
}

/// Builds a Package Directory whose Backend is the normative Node process fixture.
fn package_directory(root: &Path) {
    let backend = fs::read(
        repository_root().join("contracts/package-process/v1/fixtures/node-package-process.mjs"),
    )
    .unwrap();
    // Node treats a file without an `.mjs` extension as CommonJS, and the fixture is a module.
    write_directory(root, "backend/package.mjs", &backend, 0o700);
}

fn write_directory(root: &Path, backend_path: &str, backend: &[u8], mode: u32) {
    use std::os::unix::fs::PermissionsExt;

    let entrypoint = b"export function mount() {}".to_vec();
    let contract = br#"{"schemaVersion":1,"$defs":{},"requests":{},"subscriptions":{}}"#.to_vec();
    fs::create_dir_all(root.join("backend")).unwrap();
    fs::create_dir_all(root.join("ui")).unwrap();
    let absolute_backend = root.join(backend_path);
    fs::write(&absolute_backend, backend).unwrap();
    fs::set_permissions(&absolute_backend, fs::Permissions::from_mode(mode)).unwrap();
    fs::write(root.join("ui/index.js"), &entrypoint).unwrap();
    fs::write(root.join("ui/contract.json"), &contract).unwrap();
    let manifest = serde_json::json!({
        "schemaVersion": 1,
        "identity": "dev.example.fixture",
        "version": "1.0.0",
        "presentation": {"displayName": "Fixture"},
        "protocol": {"major": 1, "minMinor": 0, "maxMinor": 0},
        "backends": [{
            "target": TARGET, "path": backend_path,
            "size": backend.len(), "sha256": sha256(backend),
        }],
        "ui": {
            "entrypoint": {"path": "ui/index.js", "size": entrypoint.len(), "sha256": sha256(&entrypoint)},
            "assets": [],
            "contract": {"path": "ui/contract.json", "sha256": sha256(&contract)},
        },
        "capabilities": [],
    });
    fs::write(
        root.join("teamarness.package.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}

#[test]
fn directory_mode_matches_host_admission() {
    let temp = TempDir::new().unwrap();
    package_directory(temp.path());
    let output = validate(&["directory", &temp.path().to_string_lossy()]);
    assert!(output.status.success(), "{}", stdout(&output));

    let contract_path = temp.path().join("ui/contract.json");
    let mut bytes = fs::read(&contract_path).unwrap();
    bytes.push(b' ');
    fs::write(&contract_path, bytes).unwrap();
    let output = validate(&["directory", &temp.path().to_string_lossy()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stdout(&output).contains("manifest.digest_mismatch"),
        "{}",
        stdout(&output)
    );
}

fn node_is_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[test]
fn smoke_launches_the_backend_through_shutdown() {
    if !node_is_available() {
        println!("skipped: node is not on PATH");
        return;
    }
    let temp = TempDir::new().unwrap();
    package_directory(temp.path());
    let output = validate(&["directory", &temp.path().to_string_lossy(), "--smoke"]);
    assert!(output.status.success(), "{}", stdout(&output));
    assert!(
        stdout(&output).contains("activated and disposed"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn smoke_reports_a_backend_that_never_initializes() {
    let temp = TempDir::new().unwrap();
    write_directory(
        temp.path(),
        "backend/package",
        b"#!/bin/sh\nsleep 30\n",
        0o700,
    );
    let started = Instant::now();
    let output = validate(&["directory", &temp.path().to_string_lossy(), "--smoke"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stdout(&output).contains("error[smoke_failed/smoke.handshake_timeout]"),
        "{}",
        stdout(&output)
    );
    assert!(started.elapsed().as_secs() < 7, "{:?}", started.elapsed());
}

#[test]
fn usage_errors_exit_two() {
    for arguments in [
        vec![],
        vec!["source"],
        vec!["nonsense", "."],
        vec!["source", ".", "--smoke"],
        vec!["directory", ".", "--target"],
    ] {
        let output = validate(&arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
    }
}
