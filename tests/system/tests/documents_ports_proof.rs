//! Documents Port Contract 1.0 through the Host broker: a real SDK consumer Package lists,
//! reads, revises and observes the real Documents Backend, and every row of the shared merge
//! scenario table runs through `document.revise`.
#![cfg(unix)]
#[allow(dead_code)] // Shared qualification helpers are used by different integration targets.
mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use base64::{Engine as _, engine::general_purpose::STANDARD};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::harness::{
    self, Addressing, ConversationClient, PackageStage, open_core, open_host, open_mount,
    poll_short, shutdown, workspace_root,
};
use teamarness_package_host::{
    CloseUiSubscription, OpenUiSubscription, PackageHostCommand, PackageHostNotification,
    PackageHostSubscriptionSelector, PackageIdentity, PackageInstanceKey, PackageValue,
    UiSubscriptionId, WorkspaceId,
};

const DOCUMENTS: &str = "dev.teamarness.documents";
const CONSUMER: &str = "dev.teamarness.documents-port-proof";
/// A canonical token for `(peer 12345, counter 0)`, which no Document here has seen.
const UNKNOWN_BASE: &str = "AAAAAAAAMDkAAAAA";

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn documents_stage() -> PackageStage {
    PackageStage {
        identity: DOCUMENTS,
        display_name: "Documents",
        backend: PathBuf::from(env!("CARGO_BIN_EXE_documents-backend-under-test")),
        backend_file: "backend/teamarness-documents-package-aarch64-apple-darwin",
        source: "packages/documents-package",
        record_log_minor: 1,
        ui_contract: None,
    }
}

fn stage_consumer(application: &Path) {
    let root = application.join(CONSUMER);
    fs::create_dir_all(root.join("backend")).unwrap();
    fs::create_dir_all(root.join("ui")).unwrap();
    let binary = fs::read(env!("CARGO_BIN_EXE_documents-port-consumer-under-test")).unwrap();
    let backend = "backend/consumer";
    fs::write(root.join(backend), &binary).unwrap();
    fs::set_permissions(root.join(backend), fs::Permissions::from_mode(0o700)).unwrap();
    let ui = b"export function mount() { return async () => {}; }";
    let contract = br#"{"schemaVersion":1,"$defs":{},"requests":{},"subscriptions":{}}"#;
    fs::write(root.join("ui/index.js"), ui).unwrap();
    fs::write(root.join("ui-contract.json"), contract).unwrap();
    let manifest = json!({
        "schemaVersion":1,"identity":CONSUMER,"version":"1.0.0",
        "presentation":{"displayName":"Documents port proof"},
        "protocol":{"major":1,"minMinor":0,"maxMinor":0},
        "backends":[{"target":"aarch64-apple-darwin","path":backend,"size":binary.len(),"sha256":digest(&binary)}],
        "ui":{"entrypoint":{"path":"ui/index.js","size":ui.len(),"sha256":digest(ui)},"assets":[],
            "contract":{"path":"ui-contract.json","sha256":digest(contract)}},
        "capabilities":[{"identity":"teamarness.package-ports","major":1,"minMinor":0,"maxMinor":0}],
        "consumedPorts":[{"identity":DOCUMENTS,"major":1,"minMinor":0,"maxMinor":0,"ports":[
            {"name":"document.list","kind":"query"},{"name":"document.markdown","kind":"query"},
            {"name":"document.revise","kind":"command"},{"name":"documents.changed","kind":"event"}]}]
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
        .push(json!({"directory":CONSUMER}));
    fs::write(registration, serde_json::to_vec(&registered).unwrap()).unwrap();
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

/// The consumer Package, driven one numbered step at a time through its Data Directory.
struct Consumer {
    directory: PathBuf,
    next: u64,
}

impl Consumer {
    async fn step(&mut self, request: Value) -> Value {
        let step = self.next;
        self.next += 1;
        let path = self.directory.join(format!("step-{step}.json"));
        let temporary = path.with_extension("tmp");
        fs::write(&temporary, serde_json::to_vec(&request).unwrap()).unwrap();
        fs::rename(temporary, path).unwrap();
        let answer = self.directory.join(format!("answer-{step}.json"));
        assert!(
            poll_short(|| async { answer.exists() }).await,
            "the consumer answers step {step}: {request}"
        );
        serde_json::from_slice(&fs::read(answer).unwrap()).unwrap()
    }

    async fn call(&mut self, op: &str, port: &str, request: Value) -> Value {
        self.step(json!({"op":op,"port":port,"request":request}))
            .await
    }

    /// A successful call's Port value.
    async fn value(&mut self, op: &str, port: &str, request: Value) -> Value {
        let answer = self.call(op, port, request).await;
        assert_eq!(answer["ok"], true, "{port}: {answer}");
        answer["value"].clone()
    }

    async fn markdown(&mut self, document: &str) -> Value {
        self.value("query", "document.markdown", json!({"documentId":document}))
            .await
    }

    async fn revise(&mut self, document: &str, base: &Value, markdown: &str) -> Value {
        self.call(
            "command",
            "document.revise",
            json!({"documentId":document,"baseVersion":base,"markdown":markdown}),
        )
        .await
    }

    /// The next `documents.changed` event, or `None` when none arrives within `wait`.
    async fn next_event(&mut self, wait: Duration) -> Option<Value> {
        let answer = self
            .step(json!({"op":"next_event","timeoutMs":wait.as_millis() as u64}))
            .await;
        if answer["timeout"] == true {
            return None;
        }
        assert_eq!(answer["ok"], true, "{answer}");
        Some(answer["value"].clone())
    }
}

/// The Provider's own error code behind a broker `provider_error`.
fn provider_code(answer: &Value) -> &str {
    assert_eq!(answer["ok"], false, "{answer}");
    assert_eq!(answer["code"], "provider_error", "{answer}");
    answer["provider"].as_str().unwrap_or_default()
}

fn object(value: Value) -> PackageValue {
    serde_json::from_value(value).unwrap()
}

async fn ui(client: &ConversationClient<'_>, operation: &str, payload: Value) -> Value {
    serde_json::to_value(
        client
            .request("documents-proof", operation, object(payload))
            .await,
    )
    .unwrap()
}

/// A person typing into `document` through a real editor session, as the Documents UI does:
/// open `documents.document.session`, load the snapshot, type `text` at the start or end of the
/// first text run of top-level block `block` as the session peer, and send that one update
/// through `documents.document.update`.
async fn person_types(
    client: &ConversationClient<'_>,
    document: &str,
    block: usize,
    at_end: bool,
    text: &str,
) {
    use loro::{Container, ValueOrContainer};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    let subscription = UiSubscriptionId::new(format!("editor-session-{sequence}")).unwrap();
    client
        .host
        .execute(PackageHostCommand::OpenUiSubscription(OpenUiSubscription {
            mount: client.mount.clone(),
            subscription: subscription.clone(),
            operation: "documents.document.session".into(),
            payload: object(json!({ "documentId": document })),
        }))
        .await
        .unwrap();
    let mut events = client
        .host
        .subscribe(PackageHostSubscriptionSelector::UiEvents {
            mount: client.mount.clone(),
            subscription: subscription.clone(),
        })
        .await
        .unwrap();
    let mut opening = Vec::new();
    tokio::time::timeout(Duration::from_secs(30), async {
        while opening
            .last()
            .is_none_or(|event: &Value| event["event"] != "end")
        {
            if let PackageHostNotification::UiEvent(event) = events.changed().await.unwrap() {
                opening.push(serde_json::to_value(&event.payload).unwrap());
            }
        }
    })
    .await
    .expect("the session opens");
    let begin = &opening[0];
    assert_eq!(begin["event"], "begin", "{begin}");
    assert_eq!(begin["editable"], true, "{begin}");
    let mut snapshot = Vec::new();
    for chunk in &opening[1..opening.len() - 1] {
        snapshot.extend(STANDARD.decode(chunk["chunk"].as_str().unwrap()).unwrap());
    }
    let doc = loro::LoroDoc::new();
    doc.import(&snapshot).unwrap();
    let peer: u64 = begin["sessionPeer"].as_str().unwrap().parse().unwrap();
    doc.set_peer_id(peer).unwrap();
    let before = doc.oplog_vv();
    let Some(ValueOrContainer::Container(Container::List(blocks))) =
        doc.get_map("doc").get("children")
    else {
        panic!("the body has a children list");
    };
    let Some(ValueOrContainer::Container(Container::Map(node))) = blocks.get(block) else {
        panic!("the body has block {block}");
    };
    let Some(ValueOrContainer::Container(Container::List(children))) = node.get("children") else {
        panic!("block {block} has children");
    };
    let Some(ValueOrContainer::Container(Container::Text(run))) = children.get(0) else {
        panic!("block {block} starts with a text run");
    };
    run.insert(if at_end { run.len_unicode() } else { 0 }, text)
        .unwrap();
    doc.commit();
    let update = doc.export(loro::ExportMode::updates(&before)).unwrap();
    let request_id = format!("7e{sequence:030x}");
    let answer = ui(
        client,
        "documents.document.update",
        json!({
            "documentId": document, "sessionPeer": peer.to_string(),
            "requestId": request_id, "update": STANDARD.encode(update),
        }),
    )
    .await;
    assert_eq!(answer, json!({ "requestId": request_id }));
    client
        .host
        .execute(PackageHostCommand::CloseUiSubscription(
            CloseUiSubscription {
                mount: client.mount.clone(),
                subscription,
            },
        ))
        .await
        .unwrap();
}

fn unique_text(bytes: usize) -> String {
    let mut text = String::new();
    let mut index = 0u64;
    while text.len() < bytes {
        text.push_str(&format!(
            "{:016x} ",
            index.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ));
        index += 1;
    }
    text.truncate(bytes);
    text.trim_end().to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn real_sdk_consumer_lists_reads_revises_and_observes_documents() {
    let application = harness::StagedApplication::with(&documents_stage());
    stage_consumer(&application.application_path);
    let root = application.root.path();
    let core = open_core(Addressing::Loopback, &root.join("alice-core")).await;
    let workspace = core.create_workspace("Documents port proof").unwrap();
    let private = root.join("alice-host");
    let host = open_host(&application.application_path, private.clone(), core.clone()).await;
    assert!(
        poll_short(|| async { find_file(&private, "port-proof-ready").is_some() }).await,
        "consumer must activate"
    );
    let catalog = host.authoring_catalog();
    let provider = catalog
        .iter()
        .find(|entry| entry.package.identity.as_str() == DOCUMENTS)
        .expect("real Documents appears in the authoring catalog");
    let provided = serde_json::to_value(&provider.provided).unwrap();
    assert_eq!(provided[0]["identity"], DOCUMENTS);
    let ports = provided[0]["ports"].as_array().unwrap();
    assert_eq!(ports.len(), 4);
    for (name, kind) in [
        ("document.list", "query"),
        ("document.markdown", "query"),
        ("document.revise", "command"),
        ("documents.changed", "event"),
    ] {
        assert!(
            ports
                .iter()
                .any(|port| port["name"] == name && port["kind"] == kind),
            "{name}"
        );
    }
    let mut consumer = Consumer {
        directory: find_file(&private, "port-proof-ready")
            .unwrap()
            .parent()
            .unwrap()
            .to_owned(),
        next: 0,
    };

    let key = PackageInstanceKey::new(
        PackageIdentity::from_str(DOCUMENTS).unwrap(),
        WorkspaceId::new(workspace.id().to_string()).unwrap(),
    );
    let mount = open_mount(&host, &key).await;
    let client = ConversationClient {
        host: &host,
        mount: &mount,
    };
    let collection = ui(
        &client,
        "documents.collection.create",
        json!({"name":"Plans"}),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let create = |title: &'static str| {
        let collection = collection.clone();
        let client = &client;
        async move {
            ui(
                client,
                "documents.document.create",
                json!({"collectionId":collection,"title":title}),
            )
            .await["id"]
                .as_str()
                .unwrap()
                .to_owned()
        }
    };
    let document = create("Q4").await;

    // Observe first: the event is live-only, with no initial event.
    consumer
        .value("subscribe", "documents.changed", json!({}))
        .await;
    assert_eq!(
        consumer.next_event(Duration::from_millis(100)).await,
        None,
        "no initial event"
    );

    let page = consumer.value("query", "document.list", json!({})).await;
    assert_eq!(
        page["collections"],
        json!([{"id":collection,"name":"Plans","archived":false}])
    );
    assert_eq!(page["documents"][0]["id"], document);
    assert_eq!(page["documents"][0]["title"], "Q4");
    assert_eq!(page["documents"][0]["body"], "available");
    let read = consumer.markdown(&document).await;
    assert_eq!(read["markdown"], "");
    assert_eq!(read["version"], page["documents"][0]["version"]);

    // A revise needs a qualified replay; until then it is refused as pending, with certainty.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let answer = consumer.revise(&document, &read["version"], "").await;
        if answer["ok"] == true {
            assert_eq!(answer["value"]["changed"], false);
            assert_eq!(answer["value"]["version"], read["version"]);
            break;
        }
        assert_eq!(provider_code(&answer), "documents.pending", "{answer}");
        assert_eq!(answer["uncertain"], false);
        assert!(Instant::now() < deadline, "the replay qualifies");
        // poll-interval: bounded by the deadline above; qualification has no Port event.
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // One revise is one record, observed live with its Document named.
    let revised = consumer
        .revise(&document, &read["version"], "Hello from an agent\n")
        .await;
    assert_eq!(revised["ok"], true, "{revised}");
    assert_eq!(revised["value"]["changed"], true);
    assert_eq!(revised["value"]["normalised"], false);
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let event = consumer
            .next_event(deadline.saturating_duration_since(Instant::now()))
            .await
            .expect("the revise is observed");
        if event["documentIds"]
            .as_array()
            .is_some_and(|ids| ids.contains(&json!(document)))
        {
            break;
        }
    }
    let read = consumer.markdown(&document).await;
    assert_eq!(read["markdown"], "Hello from an agent\n");
    assert_eq!(read["version"], revised["value"]["version"]);

    // Merging back an unchanged export changes nothing: no op, so no record, and the same
    // version.
    let unchanged = consumer
        .revise(&document, &read["version"], "Hello from an agent\n")
        .await;
    assert_eq!(
        unchanged["value"],
        json!({"changed":false,"normalised":false,"version":read["version"]}),
        "{unchanged}"
    );
    assert_eq!(consumer.markdown(&document).await, read);

    // Typed refusals change nothing.
    let unknown = consumer
        .revise(&document, &json!(UNKNOWN_BASE), "Overwritten\n")
        .await;
    assert_eq!(provider_code(&unknown), "documents.base_unavailable");
    let huge = format!("Hello from an agent\n\n{}\n", unique_text(170 * 1024));
    let too_large = consumer.revise(&document, &read["version"], &huge).await;
    assert_eq!(provider_code(&too_large), "documents.revision_too_large");
    assert_eq!(too_large["uncertain"], false);
    // Person authority is Host-derived: a Consumer cannot name its own actor.
    let forged = consumer
        .call(
            "forged",
            "document.revise",
            json!({"documentId":document,"baseVersion":read["version"],"markdown":"Forged\n"}),
        )
        .await;
    assert_eq!(forged["code"], "capability_request_invalid", "{forged}");
    assert_eq!(consumer.markdown(&document).await, read);

    // Every row of the shared scenario table, through the Host broker. A person's concurrent
    // edits go through the Backend's one local write path before the revise reaches it.
    let table: Value = serde_json::from_slice(
        &fs::read(
            workspace_root().join("packages/documents-package/fixtures/merge-scenarios.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let scenarios = table["scenarios"].as_array().unwrap();
    assert!(!scenarios.is_empty());
    for scenario in scenarios {
        let name = scenario["name"].as_str().unwrap();
        let document = create("Scenario").await;
        let genesis = consumer.markdown(&document).await;
        assert_eq!(genesis["markdown"], "", "{name}");
        let setup = consumer
            .revise(
                &document,
                &genesis["version"],
                scenario["initial"].as_str().unwrap(),
            )
            .await;
        assert_eq!(setup["value"]["changed"], true, "{name}: {setup}");
        let initial = setup["value"]["version"].clone();
        for edit in scenario["concurrentHumanEdits"].as_array().unwrap() {
            person_types(
                &client,
                &document,
                usize::try_from(edit["block"].as_u64().unwrap()).unwrap(),
                edit["at"] == "end",
                edit["text"].as_str().unwrap(),
            )
            .await;
        }
        let current = consumer.markdown(&document).await;
        let base = match scenario["base"].as_str().unwrap() {
            "initial" => initial,
            "current" => current["version"].clone(),
            other => panic!("{name}: unknown base {other}"),
        };
        let revised = consumer
            .revise(&document, &base, scenario["revision"].as_str().unwrap())
            .await;
        assert_eq!(revised["ok"], true, "{name}: {revised}");
        let read = consumer.markdown(&document).await;
        assert_eq!(read["markdown"], scenario["expectedMarkdown"], "{name}");
        if scenario["expectedOps"] == 0 {
            assert_eq!(revised["value"]["changed"], false, "{name}");
            assert_eq!(read, current, "{name}: a no-op appends no record");
            assert_eq!(revised["value"]["version"], current["version"], "{name}");
        } else {
            assert_eq!(revised["value"]["changed"], true, "{name}");
            assert_eq!(read["version"], revised["value"]["version"], "{name}");
        }
    }

    consumer
        .value("close", "documents.changed", json!({}))
        .await;
    shutdown(host).await;
    core.shutdown().await.unwrap();
}
