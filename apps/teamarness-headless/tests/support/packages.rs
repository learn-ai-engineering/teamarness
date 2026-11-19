use serde_json::json;
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const PERSONAL: &str = "org.unfamiliar.headless-proof";
pub const TEAM: &str = "org.unfamiliar.team-proof";

pub fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn executable() -> &'static Path {
    static EXECUTABLE: OnceLock<PathBuf> = OnceLock::new();
    EXECUTABLE.get_or_init(|| {
        // Cargo rebuilds this source-owned test bin before returning its machine-readable path.
        // Never select an existing target/debug artifact merely because it happens to exist.
        let output = std::process::Command::new(env!("CARGO"))
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .args([
                "build",
                "-p",
                "teamarness-system-tests",
                "--locked",
                "--bin",
                "headless-personal-fixture",
                "--message-format=json",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find_map(|value| {
                if value["target"]["name"] == "headless-personal-fixture" {
                    value["executable"].as_str().map(PathBuf::from)
                } else {
                    None
                }
            })
            .expect("Cargo reported source-current executable")
    })
}

pub fn stage(root: &Path, identity: &str) -> PathBuf {
    std::fs::create_dir_all(root.join("backend")).unwrap();
    std::fs::create_dir_all(root.join("ui")).unwrap();
    let backend = std::fs::read(executable()).unwrap();
    std::fs::write(root.join("backend/fixture"), &backend).unwrap();
    std::fs::set_permissions(
        root.join("backend/fixture"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    // Admission validates these inert bytes; inspection/hosting must never execute them.
    let ui = b"throw new Error('UI_MUST_NOT_EXECUTE');";
    std::fs::write(root.join("ui/index.js"), ui).unwrap();
    let contract = serde_json::to_vec(&request_contract()).unwrap();
    std::fs::write(root.join("ui/contract.json"), &contract).unwrap();
    let manifest = json!({
        "schemaVersion":1,"identity":identity,"version":"1.0.0",
        "presentation":{"displayName":"Independent proof"},
        "protocol":{"major":1,"minMinor":1,"maxMinor":1},
        "activation":"eager",
        "backends":[{"target":teamarness_package_host::NATIVE_BACKEND_TARGET,"path":"backend/fixture","size":backend.len(),"sha256":digest(&backend)}],
        "ui":{"entrypoint":{"path":"ui/index.js","size":ui.len(),"sha256":digest(ui)},"assets":[],"contract":{"path":"ui/contract.json","sha256":digest(&contract)}},
        "capabilities":[{"identity":"teamarness.agent-binding","major":1,"minMinor":0,"maxMinor":0}]
    });
    std::fs::write(
        root.join("teamarness.package.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    root.to_owned()
}

pub fn edit_manifest(path: &Path, edit: impl FnOnce(&mut serde_json::Value)) {
    let file = path.join("teamarness.package.json");
    let mut value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    edit(&mut value);
    std::fs::write(file, serde_json::to_vec(&value).unwrap()).unwrap();
}

pub fn bundle(fixture: &super::Fixture) {
    stage(&fixture.config.application_root.join("packages/team"), TEAM);
    std::fs::write(
        fixture
            .config
            .application_root
            .join("packages/application-packages.v1.json"),
        serde_json::to_vec(&json!({"schemaVersion":1,"packages":[{"directory":"packages/team"}]}))
            .unwrap(),
    )
    .unwrap();
}

pub fn request_contract() -> serde_json::Value {
    let empty = json!({"type":"object","additionalProperties":false});
    let settings = json!({"type":"object","properties":{"value":{"type":"string","maxLength":262132}},"required":["value"],"additionalProperties":false});
    let mut requests = serde_json::Map::new();
    for name in [
        "proof.settings.get",
        "proof.status",
        "proof.secret",
        "proof.error",
        "proof.invalid",
        "proof.binding.current",
        "proof.binding.clear",
    ] {
        requests.insert(
            name.into(),
            json!({"request":empty,"response":{"type":"object"}}),
        );
    }
    requests["proof.settings.get"]["response"] = json!({"$ref":"#/$defs/settings"});
    requests["proof.invalid"]["response"] = settings.clone();
    requests["proof.secret"]["response"] = json!({"type":"object","required":["secret"],"additionalProperties":false,"properties":{"secret":{"type":["string","null"],"maxLength":64}}});
    requests.insert(
        "proof.settings.set".into(),
        json!({"request":{"$ref":"#/$defs/settings"},"response":{"$ref":"#/$defs/settings"}}),
    );
    requests.insert("proof.mutate".into(), json!({"request":{"type":"object","additionalProperties":false,"required":["delayMs"],"properties":{"delayMs":{"type":"integer","minimum":0,"maximum":5000}}},"response":{"type":"object"}}));
    requests.insert("proof.binding.set".into(), json!({"request":{"type":"object","additionalProperties":false,"required":["principalId"],"properties":{"principalId":{"type":"string","pattern":"^[a-f0-9]{64}$"}}},"response":{"type":"object"}}));
    json!({"schemaVersion":1,"$defs":{"settings":settings,"unused":{"type":"string","description":"complete definitions retained"}},"requests":requests,
        "subscriptions":{"proof.watch":{"request":empty,"event":{"$ref":"#/$defs/settings"}}}})
}

pub fn write_contract(directory: &Path, document: &serde_json::Value) -> String {
    let bytes = serde_json::to_vec(document).unwrap();
    std::fs::write(directory.join("ui/contract.json"), &bytes).unwrap();
    let digest = digest(&bytes);
    edit_manifest(directory, |manifest| {
        manifest["ui"]["contract"]["sha256"] = json!(digest)
    });
    digest
}
