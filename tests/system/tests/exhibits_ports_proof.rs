//! The Exhibits Port proof: a real Exhibits Backend and the test-only Exhibits Consumer, both
//! real processes under a real Package Host and Core Runtime, exercised once with the
//! Installation Person as the Port actor and default-denied for a bound Workspace Agent.
//!
//! The Consumer creates a Draft, uploads a file larger than one Port message in parts, resumes
//! an upload whose response it dropped through `files.missing`, updates the Draft so its revision
//! moves, lists it, reads its manifest and files back byte for byte, and deletes it. The test
//! then opens the Draft through the Exhibits UI contract and reads its bytes through the Host's
//! content view, which is how the Shell will serve it.
#![cfg(unix)]
#[allow(dead_code)] // Shared qualification helpers are used by different integration targets.
mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::harness::{
    self, Addressing, ConversationClient, open_core, open_host, shutdown, workspace_root,
};
use teamarness_package_host::{
    ContentViewToken, OpenUiMount, OpenUiSubscription, PackageHost, PackageHostCommand,
    PackageHostNotification, PackageHostOutcome, PackageHostSubscriptionSelector, PackageIdentity,
    PackageInstanceKey, PackageValue, ReadContent, UiMountId, UiSubscriptionId, WorkspaceId,
};
use teamarness_system_tests::exhibit_fixtures;

const EXHIBITS: &str = "dev.teamarness.exhibits";
const CONSUMER: &str = "dev.teamarness.exhibits-fixture-consumer";

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn write_executable(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

/// Stages the test-only Consumer from its manifest template, the way the packaged smoke does.
fn stage_consumer(application: &Path) {
    let source = workspace_root().join("tests/system/exhibits-consumer");
    let root = application.join(CONSUMER);
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(source.join("teamarness.package.json")).unwrap()).unwrap();
    let backend = fs::read(env!("CARGO_BIN_EXE_exhibits-port-consumer-under-test")).unwrap();
    write_executable(
        &root.join(manifest["backends"][0]["path"].as_str().unwrap()),
        &backend,
    );
    manifest["backends"][0]["size"] = json!(backend.len());
    manifest["backends"][0]["sha256"] = json!(digest(&backend));
    let ui = fs::read(source.join("ui/index.js")).unwrap();
    fs::create_dir_all(root.join("ui")).unwrap();
    fs::write(root.join("ui/index.js"), &ui).unwrap();
    manifest["ui"]["entrypoint"]["size"] = json!(ui.len());
    manifest["ui"]["entrypoint"]["sha256"] = json!(digest(&ui));
    let contract = fs::read(source.join("ui-contract.json")).unwrap();
    fs::write(root.join("ui-contract.json"), &contract).unwrap();
    manifest["ui"]["contract"]["sha256"] = json!(digest(&contract));
    fs::write(
        root.join("teamarness.package.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}

fn stage_application(root: &Path) -> PathBuf {
    let application = root.join("application");
    harness::stage_exhibits(&application);
    stage_consumer(&application);
    fs::write(
        application.join("application-packages.v1.json"),
        serde_json::to_vec(&json!({
            "schemaVersion": 1,
            "packages": [{"directory": EXHIBITS}, {"directory": CONSUMER}]
        }))
        .unwrap(),
    )
    .unwrap();
    application
}

fn key(identity: &str, workspace: &str) -> PackageInstanceKey {
    PackageInstanceKey::new(
        PackageIdentity::from_str(identity).unwrap(),
        WorkspaceId::new(workspace.to_owned()).unwrap(),
    )
}

/// Opens a UI mount and returns it with whether the Host lets it nest content frames.
async fn mount(host: &PackageHost, key: &PackageInstanceKey) -> (UiMountId, bool) {
    match host
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount::main(
            key.clone(),
        )))
        .await
        .unwrap()
    {
        PackageHostOutcome::UiMountOpened { mount, loader, .. } => (mount, loader.nested_content),
        outcome => panic!("unexpected mount outcome: {outcome:?}"),
    }
}

fn json_of(value: &PackageValue) -> Value {
    serde_json::to_value(value).unwrap()
}

fn object(value: Value) -> PackageValue {
    serde_json::from_value(value).unwrap()
}

async fn ui(client: &ConversationClient<'_>, operation: &str, payload: Value) -> Value {
    let id = operation.replace('.', "-");
    json_of(&client.request(&id, operation, object(payload)).await)
}

/// Waits for the Consumer's background proof to finish and returns its report.
async fn wait_for_report(consumer: &ConversationClient<'_>) -> Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let result = ui(consumer, "proof.result", json!({})).await;
        if result["state"] == "done" {
            return result["report"].clone();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the Consumer's proof must finish: {result}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn read_content(host: &PackageHost, token: &str, path: &str) -> (Vec<u8>, String) {
    match host
        .execute(PackageHostCommand::ReadContent(ReadContent {
            token: ContentViewToken::new(token).unwrap(),
            path: path.to_owned(),
        }))
        .await
        .unwrap()
    {
        PackageHostOutcome::Content { bytes, media_type } => (bytes, media_type),
        outcome => panic!("unexpected content outcome: {outcome:?}"),
    }
}

/// Runs the Consumer's proof in one Workspace and checks its report and the UI view.
async fn prove(host: &PackageHost, workspace: &str, prefix: &str, expected_author: (&str, &str)) {
    let (consumer_mount, consumer_nests) = mount(host, &key(CONSUMER, workspace)).await;
    assert!(
        consumer_nests,
        "the Consumer declares content-views for the packaged smoke's unsandboxed frame probe"
    );
    let consumer = ConversationClient {
        host,
        mount: &consumer_mount,
    };
    if expected_author.0 == "agent" {
        let bound = ui(&consumer, "agent.bind", json!({})).await;
        assert_eq!(bound["identity"], expected_author.1);
    }
    assert_eq!(
        ui(&consumer, "proof.start", json!({"prefix": prefix})).await,
        json!({"started": true})
    );
    let report = wait_for_report(&consumer).await;
    if expected_author.0 == "agent" {
        // Phase 1's empty settings must govern declared guide-bearing access too.
        // Enabled Agent authoring is qualified after durable settings are implemented.
        assert_eq!(report["ok"], false, "{report}");
        assert_eq!(report["details"]["code"], "port_not_enabled");
        assert_eq!(report["details"]["uncertain"], false);
        assert!(
            host.snapshot()
                .await
                .instances
                .iter()
                .filter(|i| i.key == key(EXHIBITS, workspace))
                .all(|i| i.process_generation.is_none())
        );
        return;
    }
    assert_eq!(report["ok"], true, "{report}");
    let result = &report["result"];
    let site = format!("{prefix}-site");
    let media = format!("{prefix}-media");
    let revision_1 = exhibit_fixtures::fixture("revision-1").unwrap();

    assert_eq!(result["guideChunkBytes"], 131_072);
    assert_eq!(result["guideHasHtml"], true);
    // Create, then update: the revision moves, and only the changed entry page is uploaded.
    assert_eq!(result["first"], 1);
    assert_eq!(result["firstSent"], revision_1.len());
    assert_eq!(result["second"], 2);
    assert_eq!(result["secondSent"], 1);
    // A file larger than one Port message, in parts, resumed after a dropped response.
    assert!(result["clipBytes"].as_u64().unwrap() > 256 * 1024);
    assert_eq!(result["clipMissingBefore"], true);
    assert_eq!(result["lastChunkFirstComplete"], false);
    assert_eq!(result["resumedChunks"], json!([1]));
    assert_eq!(result["resumedSent"], 1);
    assert_eq!(result["clipMissingAfter"], false);
    assert_eq!(result["mediaRevision"], 1);
    assert_eq!(result["mediaSent"], 3);
    // Refusals the Agent can act on.
    assert_eq!(result["stale"], "exhibits.revision_conflict");
    assert_eq!(result["refused"], "exhibits.type_refused");
    // Listed newest first, read back exactly, and deleted.
    assert_eq!(result["listIds"], json!([media, site]));
    assert_eq!(result["readRevision"], 2);
    assert_eq!(result["readEntry"], "index.html");
    assert_eq!(result["readTitle"], "Site");
    assert_eq!(
        result["readUpdatedBy"],
        json!({"kind": expected_author.0, "identity": expected_author.1})
    );
    assert_eq!(result["readFilesMatch"], true);
    assert_eq!(result["byteForByte"], true);
    assert_eq!(result["deleted"], true);
    assert_eq!(result["afterDelete"], "exhibits.not_found");

    // The Exhibits UI sees the remaining Draft and opens it as a content view the Host serves.
    let (exhibits_mount, exhibits_nests) = mount(host, &key(EXHIBITS, workspace)).await;
    assert!(
        exhibits_nests,
        "Exhibits declares content-views, so it may nest content frames"
    );
    let exhibits = ConversationClient {
        host,
        mount: &exhibits_mount,
    };
    let listing = ui(&exhibits, "exhibit.list", json!({})).await;
    assert_eq!(listing["exhibits"][0]["id"], site);
    assert_eq!(listing["exhibits"][0]["revision"], 2);
    assert_eq!(listing["exhibits"].as_array().unwrap().len(), 1);
    let opened = ui(&exhibits, "exhibit.open", json!({"exhibitId": site})).await;
    assert_eq!(opened["revision"], 2);
    let url = opened["url"].as_str().unwrap();
    let rest = url
        .strip_prefix("teamarness-content://localhost/")
        .expect("a content view URL");
    let (token, entry) = rest.split_once('/').unwrap();
    assert_eq!(entry, "index.html");
    let revision_2 = exhibit_fixtures::fixture("revision-2").unwrap();
    for (path, bytes) in &revision_2 {
        let (served, media_type) = read_content(host, token, path).await;
        assert_eq!(served, *bytes, "{path} is served byte for byte");
        assert!(
            media_type.starts_with(match path.rsplit('.').next().unwrap() {
                "html" => "text/html",
                "css" => "text/css",
                "mjs" => "text/javascript",
                "json" => "application/json",
                "png" => "image/png",
                "svg" => "image/svg+xml",
                "wasm" => "application/wasm",
                other => panic!("unexpected fixture extension {other}"),
            })
        );
    }
    let entry_page = String::from_utf8(read_content(host, token, "index.html").await.0).unwrap();
    assert!(entry_page.contains("Exhibit fixture revision 2"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn person_authors_exhibits_and_bound_agent_is_default_denied() {
    let root = tempfile::tempdir().unwrap();
    let application = stage_application(root.path());
    let core = open_core(Addressing::Loopback, &root.path().join("core")).await;
    let person_workspace = core.create_workspace("Exhibits as Person").unwrap().id();
    let agent_workspace = core.create_workspace("Exhibits as Agent").unwrap().id();
    let agent = core.add_agent(agent_workspace, "Scout").unwrap();
    let agent_key = core
        .principal(agent_workspace, agent)
        .unwrap()
        .unwrap()
        .keys()[0]
        .to_string();
    let person_key = core.public_identity().to_string();
    assert_ne!(agent_key, person_key);
    let host = open_host(&application, root.path().join("host"), core.clone()).await;
    let snapshot = host.snapshot().await;
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:?}",
        snapshot.diagnostics
    );
    let provider = host
        .authoring_catalog()
        .into_iter()
        .find(|entry| entry.package.identity.as_str() == EXHIBITS)
        .expect("the real Exhibits Package appears in the authoring catalog");
    let provided = serde_json::to_value(&provider.provided).unwrap();
    assert_eq!(provided[0]["identity"], EXHIBITS);
    assert_eq!(provided[0]["ports"].as_array().unwrap().len(), 8);

    prove(
        &host,
        &person_workspace.to_string(),
        "person",
        ("person", &person_key),
    )
    .await;
    prove(
        &host,
        &agent_workspace.to_string(),
        "agent",
        ("agent", &agent_key),
    )
    .await;

    // The fixture-publishing operation the packaged smoke and manual checks use.
    let (consumer_mount, _) = mount(&host, &key(CONSUMER, &person_workspace.to_string())).await;
    let consumer = ConversationClient {
        host: &host,
        mount: &consumer_mount,
    };
    let first = ui(
        &consumer,
        "fixture.publish",
        json!({"fixture": "revision-1"}),
    )
    .await;
    assert_eq!(first["exhibitId"], "fixture-revision-1");
    assert_eq!(first["revision"], 1);
    // The person proof already stored every revision 1 file, so nothing is uploaded again.
    assert_eq!(first["chunksSent"], 0);
    let second = ui(
        &consumer,
        "fixture.publish",
        json!({"fixture": "revision-2", "exhibitId": "fixture-revision-1"}),
    )
    .await;
    assert_eq!(second["revision"], 2);
    let media = ui(&consumer, "fixture.publish", json!({"fixture": "media"})).await;
    assert_eq!(media["revision"], 1);
    let probe = ui(
        &consumer,
        "fixture.publish",
        json!({"fixture": "isolation-probe"}),
    )
    .await;
    assert_eq!(probe["exhibitId"], "fixture-isolation-probe");
    assert_eq!(probe["revision"], 1);

    // The delayed publish that manual check M1 and the packaged smoke use to change an open
    // Draft: the request returns at once, and the update reaches the Exhibits UI's
    // `exhibits.changed` stream as exactly one event.
    let (exhibits_mount, _) = mount(&host, &key(EXHIBITS, &person_workspace.to_string())).await;
    let changes = UiSubscriptionId::new("exhibits-changed").unwrap();
    host.execute(PackageHostCommand::OpenUiSubscription(OpenUiSubscription {
        mount: exhibits_mount.clone(),
        subscription: changes.clone(),
        operation: "exhibits.changed".into(),
        payload: object(json!({})),
    }))
    .await
    .unwrap();
    let mut events = host
        .subscribe(PackageHostSubscriptionSelector::UiEvents {
            mount: exhibits_mount.clone(),
            subscription: changes,
        })
        .await
        .unwrap();
    let scheduled = ui(
        &consumer,
        "fixture.publish",
        json!({"fixture": "revision-1", "exhibitId": "fixture-revision-1", "delayMs": 200}),
    )
    .await;
    assert_eq!(
        scheduled,
        json!({"exhibitId": "fixture-revision-1", "scheduledInMs": 200})
    );
    let exhibits = ConversationClient {
        host: &host,
        mount: &exhibits_mount,
    };
    let event = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let PackageHostNotification::UiEvent(event) = events.changed().await.unwrap() {
                break json_of(&event.payload);
            }
        }
    })
    .await
    .expect("the delayed publish reaches the Exhibits change stream");
    assert_eq!(
        event,
        json!({"exhibitId": "fixture-revision-1", "revision": 3, "deleted": false})
    );
    let opened = ui(
        &exhibits,
        "exhibit.open",
        json!({"exhibitId": "fixture-revision-1"}),
    )
    .await;
    assert_eq!(opened["revision"], 3);
    assert!(
        consumer
            .request_fails(
                "fixture-publish",
                "fixture.publish",
                object(json!({"fixture": "revision-1", "delayMs": 60_001})),
            )
            .await,
        "a delay beyond the contract's bound is refused"
    );

    // The Consumer's own content view of the probe, which the packaged smoke frames without a
    // `sandbox` attribute.
    let framed = ui(&consumer, "frame.probe", json!({})).await;
    let url = framed["url"].as_str().unwrap();
    let (token, entry) = url
        .strip_prefix("teamarness-content://localhost/")
        .and_then(|rest| rest.split_once('/'))
        .expect("a content view URL");
    assert_eq!(entry, "index.html");
    for (path, bytes) in exhibit_fixtures::fixture("isolation-probe").unwrap() {
        assert_eq!(read_content(&host, token, path).await.0, bytes, "{path}");
    }

    shutdown(host).await;
    core.shutdown().await.unwrap();
}
