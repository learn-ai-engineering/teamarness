//! Backlog Port Contract 1.0 through the Host broker: a real SDK Consumer bound to a Workspace
//! Agent enables Backlog, reads its guide, and lists, reads, creates and updates Tickets as that
//! Agent against the real Backlog Backend. A stale base is refused, Core holds the Agent's
//! records signed with the Agent's key, and a UI subscription sees the Agent's change live.
#![cfg(unix)]
#[allow(dead_code)] // Shared qualification helpers are used by different integration targets.
mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::harness::{
    Addressing, ConversationClient, open_core, open_host, open_mount, poll_short, shutdown,
    stage_backlog,
};
use teamarness_package_host::{
    OpenUiSubscription, PackageHostCommand, PackageHostNotification,
    PackageHostSubscriptionSelector, PackageIdentity, PackageInstanceKey, PackageValue,
    UiSubscriptionId, WorkspaceId,
};

const BACKLOG: &str = "dev.teamarness.backlog";
const CONSUMER: &str = "dev.teamarness.backlog-port-proof";
const LIVE: Duration = Duration::from_secs(10);

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Stages the Consumer Package, declaring every Backlog 1.0 Port, and registers it.
fn stage_consumer(application: &Path) {
    let root = application.join(CONSUMER);
    fs::create_dir_all(root.join("backend")).unwrap();
    fs::create_dir_all(root.join("ui")).unwrap();
    let binary = fs::read(env!("CARGO_BIN_EXE_backlog-port-consumer-under-test")).unwrap();
    let backend = "backend/consumer";
    fs::write(root.join(backend), &binary).unwrap();
    fs::set_permissions(root.join(backend), fs::Permissions::from_mode(0o700)).unwrap();
    let ui = b"export function mount() { return async () => {}; }";
    let contract = br#"{"schemaVersion":1,"$defs":{},"requests":{},"subscriptions":{}}"#;
    fs::write(root.join("ui/index.js"), ui).unwrap();
    fs::write(root.join("ui-contract.json"), contract).unwrap();
    let manifest = json!({
        "schemaVersion":1,"identity":CONSUMER,"version":"1.0.0",
        "presentation":{"displayName":"Backlog port proof"},
        "protocol":{"major":1,"minMinor":0,"maxMinor":0},
        "backends":[{"target":"aarch64-apple-darwin","path":backend,"size":binary.len(),"sha256":digest(&binary)}],
        "ui":{"entrypoint":{"path":"ui/index.js","size":ui.len(),"sha256":digest(ui)},"assets":[],
            "contract":{"path":"ui-contract.json","sha256":digest(contract)}},
        "capabilities":[{"identity":"teamarness.package-ports","major":2,"minMinor":0,"maxMinor":0}],
        "consumedPorts":[{"identity":BACKLOG,"major":1,"minMinor":0,"maxMinor":0,"ports":[
            {"name":"runtime.guide","kind":"query"},
            {"name":"ticket.list","kind":"query"},
            {"name":"ticket.get","kind":"query"},
            {"name":"ticket.create","kind":"command"},
            {"name":"ticket.update","kind":"command"},
            {"name":"backlog.changed","kind":"event"}
        ]}]
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

/// The Consumer Package, driven one numbered step at a time through its Data Directory.
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

    /// The next `backlog.changed` event, or `None` when none arrives within `wait`.
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

async fn ui(client: &ConversationClient<'_>, operation: &str, payload: Value) -> Value {
    let payload: PackageValue = serde_json::from_value(payload).unwrap();
    serde_json::to_value(client.request("backlog-proof", operation, payload).await).unwrap()
}

/// Verifies: TEAM.SYS.BACKLOG-R09, TEAM.SYS.BACKLOG-R04 (AC19–AC25) — a Consumer bound to a
/// Workspace Agent enables Backlog, reads its guide, lists Epics and Tickets, reads a Ticket,
/// creates one and updates it as the Agent, and is refused `backlog.stale_version` on a stale
/// base. Core holds the Agent's records signed with the Agent's key; an open UI subscription
/// receives a new snapshot after the Agent's update, and the UI shows the Agent as the last
/// editor; `backlog.changed` fires.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn agent_bound_consumer_lists_reads_creates_and_updates_tickets_as_the_agent() {
    let root = tempfile::tempdir().unwrap();
    let application = stage_backlog(root.path());
    stage_consumer(&application);
    let core = open_core(Addressing::Loopback, &root.path().join("alice-core")).await;
    let workspace = core.create_workspace("Backlog agent proof").unwrap();
    let workspace_id = workspace.id();
    core.set_own_person_name(workspace_id, "Alice").unwrap();
    let agent = core.add_agent(workspace_id, "Planner").unwrap();
    let agent_key = core.principal(workspace_id, agent).unwrap().unwrap().keys()[0].to_string();
    let person_key = core.public_identity().to_string();
    assert_ne!(agent_key, person_key);
    let private = root.path().join("alice-host");
    let host = open_host(&application, private.clone(), core.clone()).await;
    assert!(
        poll_short(|| async { find_file(&private, "port-proof-ready").is_some() }).await,
        "consumer must activate"
    );
    let provided = serde_json::to_value(
        &host
            .authoring_catalog()
            .iter()
            .find(|entry| entry.package.identity.as_str() == BACKLOG)
            .expect("real Backlog appears in the authoring catalog")
            .provided,
    )
    .unwrap();
    assert_eq!(provided[0]["identity"], BACKLOG);
    assert_eq!(provided[0]["ports"].as_array().unwrap().len(), 6);
    let mut consumer = Consumer {
        directory: find_file(&private, "port-proof-ready")
            .unwrap()
            .parent()
            .unwrap()
            .to_owned(),
        next: 0,
    };

    // A Person sets up an Epic and a Ticket in the UI, and keeps a change subscription open.
    let key = PackageInstanceKey::new(
        PackageIdentity::from_str(BACKLOG).unwrap(),
        WorkspaceId::new(workspace_id.to_string()).unwrap(),
    );
    let mount = open_mount(&host, &key).await;
    let client = ConversationClient {
        host: &host,
        mount: &mount,
    };
    let epic_id = ui(&client, "backlog.epic.create", json!({"title":"Launch"})).await["epicId"]
        .as_str()
        .unwrap()
        .to_owned();
    let person_ticket = ui(
        &client,
        "backlog.ticket.create",
        json!({"epicId":epic_id,"title":"Pick a date","body":"Ask the team.","priority":"high"}),
    )
    .await;
    let person_ticket_id = person_ticket["ticketId"].as_str().unwrap().to_owned();
    let subscription = UiSubscriptionId::new("backlog-ui-changes").unwrap();
    host.execute(PackageHostCommand::OpenUiSubscription(OpenUiSubscription {
        mount: mount.clone(),
        subscription: subscription.clone(),
        operation: "backlog.changes".into(),
        payload: PackageValue::Object(Default::default()),
    }))
    .await
    .unwrap();
    let mut snapshots = host
        .subscribe(PackageHostSubscriptionSelector::UiEvents {
            mount: mount.clone(),
            subscription,
        })
        .await
        .unwrap();

    // From now on the Consumer is the Agent. Backlog has a guide, so the bound Agent may use it
    // only once its harness has enabled it.
    core.bind_agent_package(workspace_id, agent, CONSUMER)
        .unwrap();
    let refused = consumer.call("query", "runtime.guide", json!({})).await;
    assert_eq!(refused["code"], "port_not_enabled", "{refused}");
    consumer.value("enable", "", json!({})).await;
    let guide = consumer.value("query", "runtime.guide", json!({})).await;
    let guide = guide["markdown"].as_str().unwrap();
    for section in [
        "## What this is",
        "## Statuses",
        "## Priorities",
        "## What you can do",
        "## Saving",
        "## Limits",
    ] {
        assert!(guide.contains(section), "{section}: {guide}");
    }
    assert!(guide.contains("backlog.stale_version"));

    consumer
        .value("subscribe", "backlog.changed", json!({}))
        .await;
    assert_eq!(
        consumer.next_event(Duration::from_millis(100)).await,
        None,
        "no initial event"
    );

    // List shows the Epic and the Person's Ticket without its body; get reads it in full.
    let page = consumer.value("query", "ticket.list", json!({})).await;
    assert_eq!(
        page["epics"],
        json!([{"epicId":epic_id,"title":"Launch","archived":false}])
    );
    assert_eq!(page["tickets"][0]["ticketId"], person_ticket_id);
    assert_eq!(page["tickets"][0]["priority"], "high");
    assert!(page["tickets"][0].get("body").is_none());
    let read = consumer
        .value("query", "ticket.get", json!({"ticketId":person_ticket_id}))
        .await;
    assert_eq!(read["body"], "Ask the team.");
    assert_eq!(read["creator"], person_key);
    assert_eq!(read["version"], person_ticket["version"]);

    // The Agent creates a Ticket in the Epic.
    let created = consumer
        .value(
            "command",
            "ticket.create",
            json!({"epicId":epic_id,"title":"Draft the announcement","body":"- [ ] headline"}),
        )
        .await;
    let ticket_id = created["ticketId"].as_str().unwrap().to_owned();
    let deadline = Instant::now() + LIVE;
    consumer
        .next_event(deadline.saturating_duration_since(Instant::now()))
        .await
        .expect("the Agent's create is observed");

    // The Agent updates its Ticket from the version it read.
    let read = consumer
        .value("query", "ticket.get", json!({"ticketId":ticket_id}))
        .await;
    assert_eq!(read["status"], "not_ready");
    assert_eq!(read["creator"], agent_key);
    let updated = consumer
        .value(
            "command",
            "ticket.update",
            json!({"ticketId":ticket_id,"baseVersion":read["version"],
                "status":"in_progress","body":"- [x] headline"}),
        )
        .await;
    assert_ne!(updated["version"], read["version"]);

    // A save on the old version is refused with the current Ticket and writes nothing.
    let stale = consumer
        .call(
            "command",
            "ticket.update",
            json!({"ticketId":ticket_id,"baseVersion":read["version"],"priority":"low"}),
        )
        .await;
    assert_eq!(provider_code(&stale), "backlog.stale_version", "{stale}");
    assert_eq!(stale["uncertain"], false);
    assert_eq!(
        stale["providerDetails"]["ticket"]["version"],
        updated["version"]
    );
    assert_eq!(stale["providerDetails"]["ticket"]["status"], "in_progress");
    let current = consumer
        .value("query", "ticket.get", json!({"ticketId":ticket_id}))
        .await;
    assert_eq!(current["version"], updated["version"]);
    assert_eq!(current["priority"], "medium");
    assert_eq!(current["body"], "- [x] headline");

    // The open UI subscription receives a snapshot with the Agent's update, and the UI shows the
    // Agent as the last editor.
    tokio::time::timeout(LIVE, async {
        loop {
            if let PackageHostNotification::UiEvent(event) = snapshots.changed().await.unwrap() {
                let snapshot = serde_json::to_value(&event.payload).unwrap();
                if snapshot["tickets"].as_array().is_some_and(|tickets| {
                    tickets.iter().any(|ticket| {
                        ticket["ticketId"] == ticket_id && ticket["version"] == updated["version"]
                    })
                }) {
                    return;
                }
            }
        }
    })
    .await
    .expect("the UI sees the Agent's update live");
    let shown = ui(&client, "backlog.ticket.get", json!({"ticketId":ticket_id})).await;
    assert_eq!(shown["lastEditor"], agent_key);
    assert_eq!(shown["status"], "in_progress");
    assert!(
        consumer.next_event(LIVE).await.is_some(),
        "backlog.changed fires after the update"
    );

    // Core holds the Person's records and the Agent's create and update, each Agent record
    // signed with the Agent's key.
    let mut records = core.subscribe_records(workspace_id, BACKLOG, None).unwrap();
    let page = records.next_page(256).unwrap();
    let authors = |purpose: &str| -> Vec<String> {
        page.events
            .iter()
            .filter(|event| event.purpose() == purpose)
            .map(|event| event.author().to_string())
            .collect()
    };
    assert_eq!(authors("epic.create"), vec![person_key.clone()]);
    assert_eq!(
        authors("ticket.create"),
        vec![person_key, agent_key.clone()],
        "the Person created the first Ticket and the Agent the second"
    );
    assert_eq!(authors("ticket.update"), vec![agent_key]);

    consumer.value("close", "backlog.changed", json!({})).await;
    shutdown(host).await;
    core.shutdown().await.unwrap();
}
