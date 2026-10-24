#![cfg(unix)]
#[allow(dead_code)] // Shared qualification helpers are used by different integration targets.
mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::harness::{
    self, Addressing, ConversationClient, GENERAL_ID, PACKAGE_ID, create_invitation,
    has_message_on_page, introduce, open_core, open_host, open_mount, poll_short, shutdown,
};
use teamarness_package_host::{
    DisposePackage, PackageHostCommand, PackageIdentity, PackageInstanceKey, WorkspaceId,
};

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn stage_consumer(application: &Path) {
    let id = "dev.teamarness.conversations-port-proof";
    let root = application.join(id);
    fs::create_dir_all(root.join("backend")).unwrap();
    fs::create_dir_all(root.join("ui")).unwrap();
    let binary = fs::read(env!("CARGO_BIN_EXE_conversations-port-consumer-under-test")).unwrap();
    let backend = "backend/consumer";
    fs::write(root.join(backend), &binary).unwrap();
    fs::set_permissions(root.join(backend), fs::Permissions::from_mode(0o700)).unwrap();
    let ui = b"export function mount() { return async () => {}; }";
    let contract = br#"{"schemaVersion":1,"$defs":{},"requests":{},"subscriptions":{}}"#;
    fs::write(root.join("ui/index.js"), ui).unwrap();
    fs::write(root.join("ui-contract.json"), contract).unwrap();
    let manifest = json!({
        "schemaVersion":1,"identity":id,"version":"1.0.0","presentation":{"displayName":"Port proof"},
        "protocol":{"major":1,"minMinor":0,"maxMinor":0},
        "backends":[{"target":"aarch64-apple-darwin","path":backend,"size":binary.len(),"sha256":digest(&binary)}],
        "ui":{"entrypoint":{"path":"ui/index.js","size":ui.len(),"sha256":digest(ui)},"assets":[],
            "contract":{"path":"ui-contract.json","sha256":digest(contract)}},
        "capabilities":[{"identity":"teamarness.package-ports","major":1,"minMinor":0,"maxMinor":0}],
        "consumedPorts":[{"identity":PACKAGE_ID,"major":1,"minMinor":0,"maxMinor":0,"ports":[
            {"name":"message.post","kind":"command"},{"name":"message.page","kind":"query"},
            {"name":"messages.changed","kind":"event"}]}]
    });
    fs::write(
        root.join("teamarness.package.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let registration = application.join("application-packages.v1.json");
    let mut registered: Value = serde_json::from_slice(&fs::read(&registration).unwrap()).unwrap();
    registered["packages"]
        .as_array_mut()
        .unwrap()
        .push(json!({"directory":id}));
    fs::write(registration, serde_json::to_vec(&registered).unwrap()).unwrap();
}

fn provider_logged(root: &Path, id: &str, stage: &str) -> bool {
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if provider_logged(&path, id, stage) {
                return true;
            }
        } else if let Ok(text) = fs::read_to_string(path) {
            for line in text.lines() {
                if let Ok(value) = serde_json::from_str::<Value>(line)
                    && value["kind"] == "conversations.port"
                    && value["providerId"] == id
                    && value["stage"] == stage
                {
                    return true;
                }
            }
        }
    }
    false
}

fn find_file(root: &Path, filename: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(root).ok()? {
        let path = entry.ok()?.path();
        if path.file_name().is_some_and(|name| name == filename) {
            return Some(path);
        }
        if path.is_dir()
            && let Some(found) = find_file(&path, filename)
        {
            return Some(found);
        }
    }
    None
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn real_sdk_consumer_posts_invalidates_recovers_and_converges() {
    if std::env::var_os("TEAMARNESS_PORT_PROOF_CHILD").is_none() {
        // Set the Provider diagnostic opt-in for this process only. Mutating a multi-threaded
        // test runner's environment would affect unrelated tests and is not safe.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "real_sdk_consumer_posts_invalidates_recovers_and_converges",
                "--nocapture",
            ])
            .env("TEAMARNESS_PORT_PROOF_CHILD", "1")
            .env("TEAMARNESS_CONVERSATIONS_PORT_DIAGNOSTICS", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child proof failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let alice_app = harness::StagedApplication::new();
    stage_consumer(&alice_app.application_path);
    let root = alice_app.root.path();
    let alice_core = open_core(Addressing::Loopback, &root.join("alice-core")).await;
    let workspace = alice_core.create_workspace("Port proof").unwrap();
    let workspace_id = workspace.id();
    let alice_private = root.join("alice-host");
    let alice_host = open_host(
        &alice_app.application_path,
        alice_private.clone(),
        alice_core.clone(),
    )
    .await;
    assert!(
        poll_short(|| async { find_file(&alice_private, "port-proof-ready").is_some() }).await,
        "consumer must activate"
    );
    let catalog = alice_host.authoring_catalog();
    let provider = catalog
        .iter()
        .find(|entry| entry.package.identity.as_str() == PACKAGE_ID)
        .expect("real Conversations appears in the authoring catalog");
    let provided = serde_json::to_value(&provider.provided).unwrap();
    assert_eq!(provided[0]["identity"], PACKAGE_ID);
    assert_eq!(provided[0]["minor"], 1);
    assert_eq!(provided[0]["ports"].as_array().unwrap().len(), 4);
    for (name, kind) in [
        ("message.post", "command"),
        ("message.page", "query"),
        ("messages.changed", "event"),
        ("channel.list", "query"),
    ] {
        assert!(provided[0]["ports"].as_array().unwrap().iter().any(|port| {
            port["name"] == name
                && port["kind"] == kind
                && port["description"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty())
        }));
    }
    let mut observations = alice_host.observe_ports();
    let start = find_file(&alice_private, "port-proof-ready")
        .unwrap()
        .with_file_name("port-proof-start");
    fs::write(start, b"").unwrap();
    assert!(
        poll_short(|| async { find_file(&alice_private, "port-proof.json").is_some() }).await,
        "consumer must finish"
    );
    let proof: Value = serde_json::from_slice(
        &fs::read(find_file(&alice_private, "port-proof.json").unwrap()).unwrap(),
    )
    .unwrap();
    let trace: Vec<_> = std::iter::from_fn(|| observations.try_recv().ok()).collect();
    let post = trace
        .iter()
        .find(|event| {
            event.kind == "command"
                && event.stage == teamarness_package_host::PortStage::Dispatched
                && event.port.as_deref() == Some("message.post")
        })
        .expect("post dispatch observation");
    assert!(post.consumer_id.starts_with("p:"));
    assert!(post.provider_id.as_deref().unwrap().starts_with("h:port-"));
    let provider_id = post.provider_id.as_deref().unwrap();
    assert!(
        poll_short(|| async {
            provider_logged(
                &alice_private.join("packages/logs"),
                provider_id,
                "Completed",
            )
        })
        .await,
        "Provider SDK's h: terminal must join the Host p: to h: mapping"
    );
    assert!(
        trace
            .iter()
            .any(|event| event.consumer_id == post.consumer_id
                && event.consumer_package == post.consumer_package
                && event.provider_id == post.provider_id
                && event.stage == teamarness_package_host::PortStage::Completed)
    );
    assert!(trace.iter().any(|event| {
        event.kind == "event"
            && event.stage == teamarness_package_host::PortStage::Subscribed
            && event
                .provider_id
                .as_deref()
                .unwrap()
                .starts_with("h:port-event-")
    }));
    assert!(
        trace.iter().any(|event| event.kind == "event"
            && event.stage == teamarness_package_host::PortStage::Event)
    );
    assert!(trace.iter().any(
        |event| event.stage == teamarness_package_host::PortStage::Rejected
            && event.provider_id.is_none()
    ));
    assert!(!format!("{trace:?}").contains("port-proof-first"));
    let diagnostic_text = fs::read_dir(alice_private.join("packages/logs"))
        .unwrap()
        .flatten()
        .filter_map(|entry| fs::read_to_string(entry.path()).ok())
        .collect::<String>();
    assert!(
        !diagnostic_text.contains("port-proof-first")
            && !diagnostic_text.contains("port-proof-second"),
        "Provider diagnostics must omit Message content"
    );
    assert_eq!(proof["ok"], true, "{proof}");
    let result = &proof["result"];
    assert_eq!(result["first"], json!({"ok":true}));
    assert_eq!(result["second"], json!({"ok":true}));
    assert_eq!(result["event"], json!({}));
    assert_eq!(result["initialQuiet"], true);
    assert_eq!(result["lateReplay"], false);
    assert_eq!(result["denied"], "ungranted_port");
    assert_eq!(result["malformed"], "port_request_invalid");
    assert_eq!(result["forged"], "capability_request_invalid");
    assert_eq!(result["hidden"], "provider_error");
    let failed_post = trace
        .iter()
        .find(|event| {
            event.kind == "command"
                && event.port.as_deref() == Some("message.post")
                && event.stage == teamarness_package_host::PortStage::Failed
                && event.provider_id.is_some()
        })
        .expect("real Provider refusal must retain the p: to h: join");
    assert!(failed_post.consumer_id.starts_with("p:"));
    let failed_provider_id = failed_post.provider_id.as_deref().unwrap();
    assert!(failed_provider_id.starts_with("h:port-"));
    assert!(
        poll_short(|| async {
            provider_logged(
                &alice_private.join("packages/logs"),
                failed_provider_id,
                "Failed",
            )
        })
        .await,
        "Provider SDK's failed h: diagnostic must join the Host's p: route"
    );
    let author = alice_core.public_identity().to_string();
    let messages = result["recovered"]["messages"].as_array().unwrap();
    for text in ["port-proof-first", "port-proof-second"] {
        assert!(
            messages
                .iter()
                .any(|row| row["markdown"] == text && row["author"] == author)
        );
    }
    assert!(
        result["page"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["markdown"] == "port-proof-first")
    );
    let key = PackageInstanceKey::new(
        PackageIdentity::from_str(PACKAGE_ID).unwrap(),
        WorkspaceId::new(workspace_id.to_string()).unwrap(),
    );
    let alice_mount = open_mount(&alice_host, &key).await;
    let alice_ui = ConversationClient {
        host: &alice_host,
        mount: &alice_mount,
    };
    let ui = alice_ui.channel_open("port-parity", GENERAL_ID, 64).await;
    let ui: Value = serde_json::to_value(&ui).unwrap();
    assert_eq!(ui["messages"], result["recovered"]["messages"]);

    let invitation = create_invitation(&alice_core, workspace_id).await;
    let bob_core = open_core(Addressing::Loopback, &root.join("bob-core")).await;
    introduce(Addressing::Loopback, &[&alice_core, &bob_core]);
    bob_core.join_workspace(&invitation).await.unwrap();
    let bob_app = harness::StagedApplication::new();
    let bob_host = open_host(
        &bob_app.application_path,
        root.join("bob-host"),
        bob_core.clone(),
    )
    .await;
    let bob_mount = open_mount(&bob_host, &key).await;
    let bob_ui = ConversationClient {
        host: &bob_host,
        mount: &bob_mount,
    };
    assert!(
        poll_short(|| async {
            has_message_on_page(
                &bob_ui.channel_open("port-remote", GENERAL_ID, 64).await,
                "port-proof-second",
            )
        })
        .await,
        "signed Message must converge"
    );
    let remote: Value =
        serde_json::to_value(&bob_ui.channel_open("port-author", GENERAL_ID, 64).await).unwrap();
    assert!(
        remote["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["markdown"] == "port-proof-second" && row["author"] == author)
    );
    alice_host
        .execute(PackageHostCommand::Dispose(DisposePackage {
            instance: key,
        }))
        .await
        .unwrap();
    fs::write(
        find_file(&alice_private, "port-proof-ready")
            .unwrap()
            .with_file_name("port-proof-failure-start"),
        b"",
    )
    .unwrap();
    assert!(
        poll_short(|| async { find_file(&alice_private, "port-proof-failure.json").is_some() })
            .await,
        "consumer must receive Provider failure"
    );
    let failed: Value = serde_json::from_slice(
        &fs::read(find_file(&alice_private, "port-proof-failure.json").unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(failed["ok"], false, "{failed}");
    assert_eq!(
        failed["uncertain"], false,
        "an undispatched query is a definite failure"
    );
    let failed_trace: Vec<_> = std::iter::from_fn(|| observations.try_recv().ok()).collect();
    assert!(failed_trace.iter().any(|event| event.stage
        == teamarness_package_host::PortStage::Rejected
        && event.provider_id.is_none()));
    shutdown(bob_host).await;
    shutdown(alice_host).await;
    bob_core.shutdown().await.unwrap();
    alice_core.shutdown().await.unwrap();
}
