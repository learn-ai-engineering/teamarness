//! Scripted generic-client qualification, not a model or standalone Agent UI test.
#![cfg(unix)]
#[allow(dead_code)]
mod support;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, os::unix::fs::PermissionsExt, path::Path, time::Duration};
use support::harness::{
    self, Addressing, ConversationClient, GENERAL_ID, PACKAGE_ID, QUALIFICATION, object, open_core,
    open_host, open_mount, shutdown, write_qualification_provider,
};
use teamarness_package_host::{
    InstallPersonal, PackageHost, PackageHostCommand, PackageHostOutcome, PackageInstanceKey,
    PackageTier, PortObservations, PortStage, UiMountId, WorkspaceId,
};

const CLIENT: &str = "dev.teamarness.generic-port-proof";

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn stage_client(application: &Path) {
    write_client(&application.join(CLIENT));
    let registration = application.join("application-packages.v1.json");
    let mut registered: Value = serde_json::from_slice(&fs::read(&registration).unwrap()).unwrap();
    registered["packages"]
        .as_array_mut()
        .unwrap()
        .push(json!({"directory":CLIENT}));
    fs::write(registration, serde_json::to_vec(&registered).unwrap()).unwrap();
    on_demand_conversations(application);
}

/// Exercise real Conversations with opt-in lazy activation, not a replacement Provider.
fn on_demand_conversations(application: &Path) {
    let path = application.join(PACKAGE_ID).join("teamarness.package.json");
    let mut provider: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    provider["activation"] = json!("on_demand");
    fs::write(path, serde_json::to_vec(&provider).unwrap()).unwrap();
}

/// Writes the generic Agent client's Package Directory at `root`.
fn write_client(root: &Path) {
    fs::create_dir_all(root.join("backend")).unwrap();
    fs::create_dir_all(root.join("ui")).unwrap();
    let binary = fs::read(env!("CARGO_BIN_EXE_generic-port-consumer-under-test")).unwrap();
    fs::write(root.join("backend/consumer"), &binary).unwrap();
    fs::set_permissions(
        root.join("backend/consumer"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let ui = b"export function mount() { return async () => {}; }";
    let contract = serde_json::to_vec(&json!({"schemaVersion":1,"$defs":{},"requests":{
        "proof.execute":{"request":{"type":"object"},"response":{"type":"object"}}
    },"subscriptions":{}}))
    .unwrap();
    fs::write(root.join("ui/index.js"), ui).unwrap();
    fs::write(root.join("ui-contract.json"), &contract).unwrap();
    let manifest = json!({
        "schemaVersion":1,"identity":CLIENT,"version":"1.0.0","presentation":{"displayName":"Generic Port proof"},
        "protocol":{"major":1,"minMinor":0,"maxMinor":0},
        "backends":[{"target":"aarch64-apple-darwin","path":"backend/consumer","size":binary.len(),"sha256":digest(&binary)}],
        "ui":{"entrypoint":{"path":"ui/index.js","size":ui.len(),"sha256":digest(ui)},"assets":[],
            "contract":{"path":"ui-contract.json","sha256":digest(&contract)}},
        "capabilities":[{"identity":"teamarness.package-ports","major":2,"minMinor":0,"maxMinor":0},
            {"identity":"teamarness.agent-binding","major":1,"minMinor":0,"maxMinor":0}]
    });
    assert!(manifest.get("consumedPorts").is_none());
    fs::write(
        root.join("teamarness.package.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}

async fn execute(host: &PackageHost, mount: &UiMountId, input: Value) -> Value {
    let client = ConversationClient { host, mount };
    serde_json::to_value(
        client
            .request(
                "generic",
                "proof.execute",
                serde_json::from_value(input).unwrap(),
            )
            .await,
    )
    .unwrap()
}

async fn success(host: &PackageHost, mount: &UiMountId, input: Value) -> Value {
    let output = execute(host, mount, input.clone()).await;
    assert_eq!(output["ok"], true, "{input}: {output}");
    output["value"].clone()
}

async fn subscribed(observations: &mut PortObservations) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = observations.recv().await.unwrap();
            if event.consumer_package == CLIENT && event.stage == PortStage::Subscribed {
                break;
            }
        }
    })
    .await
    .expect("generic subscription installed before initial read");
}

async fn terminal(host: &PackageHost, mount: &UiMountId, code: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let next = execute(host, mount, json!({"action":"next"})).await;
            if next["ok"] == true {
                // Revocation permits already accepted bounded invalidations to drain.
                assert_eq!(next["value"], json!({}));
            } else {
                assert_eq!(next["error"], code, "{next}");
                break;
            }
        }
    })
    .await
    .expect("bounded stream terminal after queued events");
}

fn assert_rows(page: &Value, expected: &[(&str, &str)]) {
    let rows = page["messages"].as_array().unwrap();
    assert_eq!(rows.len(), expected.len(), "no command replay: {page}");
    for (text, author) in expected {
        assert_eq!(
            rows.iter()
                .filter(|row| row["markdown"] == *text && row["author"] == *author)
                .count(),
            1,
            "{page}"
        );
    }
}

// Verifies: TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14, TEAM.SYS.CONV-R18, TEAM.SYS.CONV-R19, TEAM.SYS.CONV-R20
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn generic_client_discovers_guidance_posts_as_agent_and_reconciles_after_reconnect() {
    let app = harness::StagedApplication::new();
    stage_client(&app.application_path);
    let root = app.root.path();
    let core = open_core(Addressing::Loopback, &root.join("core")).await;
    let workspace = core.create_workspace("Generic Agent proof").unwrap();
    let a = core.add_agent(workspace.id(), "First").unwrap();
    let b = core.add_agent(workspace.id(), "Second").unwrap();
    let a_key = core.principal(workspace.id(), a).unwrap().unwrap().keys()[0].to_string();
    let b_key = core.principal(workspace.id(), b).unwrap().unwrap().keys()[0].to_string();
    let key = |id: &str| {
        PackageInstanceKey::new(
            id.parse().unwrap(),
            WorkspaceId::new(workspace.id().to_string()).unwrap(),
        )
    };
    let private = root.join("host");
    let host = open_host(&app.application_path, private.clone(), core.clone()).await;
    let mount = open_mount(&host, &key(CLIENT)).await;
    let unbound = execute(&host, &mount, json!({"action":"discover"})).await;
    assert_eq!(unbound["error"], "agent_not_bound");
    success(
        &host,
        &mount,
        json!({"action":"bind","principalId":a.to_string()}),
    )
    .await;
    let catalog = success(&host, &mount, json!({"action":"discover"})).await;
    assert_eq!(catalog.as_array().unwrap().len(), 1);
    let row = &catalog[0];
    assert_eq!(row["summary"]["contract"], PACKAGE_ID);
    assert_eq!(row["summary"]["enabled"], false);
    assert_eq!(row["summary"]["available"], true);
    let document = &row["document"];
    assert_eq!(document["contracts"][0]["minor"], 3);
    let ports = document["contracts"][0]["ports"].as_array().unwrap();
    assert_eq!(ports.len(), 6);
    for name in [
        "runtime.guide",
        "channel.list",
        "message.post",
        "message.page",
        "message.changes",
        "messages.changed",
    ] {
        assert!(ports.iter().any(|port| port["name"] == name
            && port["description"].as_str().is_some_and(|s| !s.is_empty())));
    }
    assert!(document["$defs"]["page"].is_object());
    let contract = row["summary"]["contract"].as_str().unwrap();
    let invoke = |port: &str, request: Value| json!({"action":"invoke","contract":contract,"port":port,"request":request});
    let denied = execute(&host, &mount, invoke("runtime.guide", json!({}))).await;
    assert_eq!(denied["error"], "port_not_enabled");
    assert_eq!(denied["uncertain"], false);
    success(
        &host,
        &mount,
        json!({"action":"enable","contract":contract}),
    )
    .await;
    assert!(
        host.snapshot()
            .await
            .instances
            .iter()
            .filter(|i| i.key.package_identity.as_str() == PACKAGE_ID)
            .all(|i| i.process_generation.is_none()),
        "metadata and enable must not start the Provider"
    );
    let guide = success(&host, &mount, invoke("runtime.guide", json!({}))).await;
    assert!(serde_json::to_vec(&guide).unwrap().len() <= 32 * 1024);
    let markdown = guide["markdown"].as_str().unwrap();
    assert!(markdown.contains("uncertain") && markdown.contains("epoch"));
    let channels = success(&host, &mount, invoke("channel.list", json!({}))).await;
    assert_eq!(channels["channels"][0]["id"], GENERAL_ID);
    let channel = channels["channels"][0]["id"].as_str().unwrap();
    let mut observations = host.observe_ports();
    success(
        &host,
        &mount,
        json!({"action":"subscribe","contract":contract,"port":"messages.changed"}),
    )
    .await;
    subscribed(&mut observations).await;
    let initial = success(
        &host,
        &mount,
        invoke("message.page", json!({"channelId":channel})),
    )
    .await;
    assert_rows(&initial, &[]);
    success(
        &host,
        &mount,
        invoke(
            "message.post",
            json!({"channelId":channel,"markdown":"generic-first"}),
        ),
    )
    .await;
    assert_eq!(
        success(&host, &mount, json!({"action":"next"})).await,
        json!({})
    );
    let first = success(
        &host,
        &mount,
        invoke("message.page", json!({"channelId":channel})),
    )
    .await;
    assert_rows(&first, &[("generic-first", &a_key)]);
    success(&host, &mount, json!({"action":"close"})).await;
    success(
        &host,
        &mount,
        invoke(
            "message.post",
            json!({"channelId":channel,"markdown":"generic-offline"}),
        ),
    )
    .await;
    success(
        &host,
        &mount,
        json!({"action":"subscribe","contract":contract,"port":"messages.changed"}),
    )
    .await;
    subscribed(&mut observations).await;
    let changes = success(&host, &mount, invoke("message.changes", json!({"channelId":channel,"epoch":first["epoch"],"after":first["revision"],"limit":32}))).await;
    assert!(
        changes.get("resyncRequired").is_none(),
        "normal recovery omits the optional resync flag: {changes}"
    );
    let recovered_events = changes["events"].as_array().unwrap();
    assert_eq!(recovered_events.len(), 1);
    assert_eq!(recovered_events[0]["kind"], "insert");
    assert_eq!(
        recovered_events[0]["message"]["markdown"],
        "generic-offline"
    );
    assert_eq!(recovered_events[0]["message"]["author"], a_key);
    assert_eq!(changes["continuation"], changes["highWater"]);
    let acknowledged = success(
        &host,
        &mount,
        invoke(
            "message.changes",
            json!({"channelId":channel,"epoch":first["epoch"],"after":changes["continuation"]}),
        ),
    )
    .await;
    assert_eq!(acknowledged["events"], json!([]));
    let stale = success(
        &host,
        &mount,
        invoke(
            "message.changes",
            json!({"channelId":channel,"epoch":"0".repeat(32),"after":changes["continuation"]}),
        ),
    )
    .await;
    assert_eq!(stale["resyncRequired"], true);
    success(
        &host,
        &mount,
        json!({"action":"disable","contract":contract}),
    )
    .await;
    terminal(&host, &mount, "port_not_enabled").await;
    let denied = execute(
        &host,
        &mount,
        invoke(
            "message.post",
            json!({"channelId":channel,"markdown":"must-not-post"}),
        ),
    )
    .await;
    assert_eq!(denied["error"], "port_not_enabled");
    assert_eq!(denied["uncertain"], false);
    success(
        &host,
        &mount,
        json!({"action":"enable","contract":contract}),
    )
    .await;
    success(
        &host,
        &mount,
        json!({"action":"subscribe","contract":contract,"port":"messages.changed"}),
    )
    .await;
    subscribed(&mut observations).await;
    success(
        &host,
        &mount,
        json!({"action":"bind","principalId":b.to_string()}),
    )
    .await;
    terminal(&host, &mount, "agent_binding_changed").await;
    success(
        &host,
        &mount,
        json!({"action":"subscribe","contract":contract,"port":"messages.changed"}),
    )
    .await;
    subscribed(&mut observations).await;
    success(
        &host,
        &mount,
        invoke(
            "message.post",
            json!({"channelId":channel,"markdown":"generic-rebound"}),
        ),
    )
    .await;
    success(&host, &mount, json!({"action":"next"})).await;
    success(&host, &mount, json!({"action":"unbind"})).await;
    terminal(&host, &mount, "agent_not_bound").await;
    assert_eq!(
        execute(
            &host,
            &mount,
            invoke("message.page", json!({"channelId":channel}))
        )
        .await["error"],
        "agent_not_bound"
    );
    success(
        &host,
        &mount,
        json!({"action":"bind","principalId":a.to_string()}),
    )
    .await;
    shutdown(host).await;
    let host = open_host(&app.application_path, private, core.clone()).await;
    let mount = open_mount(&host, &key(CLIENT)).await;
    let retained = success(&host, &mount, json!({"action":"discover"})).await;
    assert_eq!(retained[0]["summary"]["enabledMajor"], 1);
    assert_eq!(retained[0]["summary"]["enabled"], true);
    let recovered = success(
        &host,
        &mount,
        invoke("message.page", json!({"channelId":channel})),
    )
    .await;
    assert_rows(
        &recovered,
        &[
            ("generic-first", &a_key),
            ("generic-offline", &a_key),
            ("generic-rebound", &b_key),
        ],
    );
    // An ordinary non-Agent consumer remains usable after the generic harness revocations.
    let ui_mount = open_mount(&host, &key(PACKAGE_ID)).await;
    let ui = ConversationClient {
        host: &host,
        mount: &ui_mount,
    };
    let parity = serde_json::to_value(ui.channel_open("parity", channel, 64).await).unwrap();
    assert_eq!(parity["messages"], recovered["messages"]);
    ui.request("status", "conversations.snapshot", object([]))
        .await;
    shutdown(host).await;
    core.shutdown().await.unwrap();
}

/// Installs one Personal Package from `directory` into a running Host.
async fn install_personal(host: &PackageHost, directory: &Path, identity: &str) {
    let outcome = host
        .execute(PackageHostCommand::InstallPersonal(InstallPersonal {
            directory: directory.to_owned(),
            expected_identity: identity.parse().unwrap(),
            accepted_risks: vec!["filesystem".into(), "network".into(), "process".into()],
        }))
        .await
        .unwrap_or_else(|error| panic!("install {identity}: {error:?}"));
    assert!(
        matches!(outcome, PackageHostOutcome::PersonalInstalled { .. }),
        "{outcome:?}"
    );
}

// Verifies: TEAM.SYS.PKG-R09, TEAM.SYS.PKG-R13
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn personal_agent_harness_discovers_and_enables_team_and_personal_guides() {
    // Conversations is a Team guide Provider; the qualification Provider and the generic
    // Agent harness are Personal, installed live into the running Host.
    let app = harness::StagedApplication::new();
    on_demand_conversations(&app.application_path);
    let root = app.root.path();
    let personal = root.join("personal");
    write_client(&personal.join(CLIENT));
    write_qualification_provider(&personal.join(QUALIFICATION));
    let core = open_core(Addressing::Loopback, &root.join("core")).await;
    let workspace = core.create_workspace("Personal Agent harness").unwrap();
    let agent = core.add_agent(workspace.id(), "Personal").unwrap();
    let agent_key = core
        .principal(workspace.id(), agent)
        .unwrap()
        .unwrap()
        .keys()[0]
        .to_string();
    let key = |id: &str| {
        PackageInstanceKey::new(
            id.parse().unwrap(),
            WorkspaceId::new(workspace.id().to_string()).unwrap(),
        )
    };
    let host = open_host(&app.application_path, root.join("host"), core.clone()).await;
    install_personal(&host, &personal.join(QUALIFICATION), QUALIFICATION).await;
    install_personal(&host, &personal.join(CLIENT), CLIENT).await;
    let snapshot = host.snapshot().await;
    for identity in [CLIENT, QUALIFICATION] {
        let registration = snapshot
            .registrations
            .iter()
            .find(|registration| registration.expected_identity == identity)
            .unwrap();
        assert_eq!(registration.tier, PackageTier::Personal, "{identity}");
    }
    let mount = open_mount(&host, &key(CLIENT)).await;
    success(
        &host,
        &mount,
        json!({"action":"bind","principalId":agent.to_string()}),
    )
    .await;

    let catalog = success(&host, &mount, json!({"action":"discover"})).await;
    let mut contracts: Vec<_> = catalog
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            assert_eq!(row["summary"]["available"], true, "{row}");
            assert_eq!(row["summary"]["enabled"], false, "{row}");
            row["summary"]["contract"].as_str().unwrap().to_owned()
        })
        .collect();
    contracts.sort();
    assert_eq!(
        contracts,
        [PACKAGE_ID, QUALIFICATION],
        "both tiers: {catalog}"
    );
    for contract in [PACKAGE_ID, QUALIFICATION] {
        let invoke =
            json!({"action":"invoke","contract":contract,"port":"runtime.guide","request":{}});
        let denied = execute(&host, &mount, invoke.clone()).await;
        assert_eq!(denied["error"], "port_not_enabled", "{contract}: {denied}");
        success(
            &host,
            &mount,
            json!({"action":"enable","contract":contract}),
        )
        .await;
        let guide = success(&host, &mount, invoke).await;
        assert!(
            guide["markdown"]
                .as_str()
                .is_some_and(|markdown| !markdown.is_empty()),
            "{contract}: {guide}"
        );
    }
    let written = success(
        &host,
        &mount,
        json!({"action":"invoke","contract":QUALIFICATION,"port":"sample.write","request":{"value":"personal"}}),
    )
    .await;
    assert_eq!(written["actor"]["identity"], agent_key, "{written}");
    let channels = success(
        &host,
        &mount,
        json!({"action":"invoke","contract":PACKAGE_ID,"port":"channel.list","request":{}}),
    )
    .await;
    assert_eq!(channels["channels"][0]["id"], GENERAL_ID);
    shutdown(host).await;
    core.shutdown().await.unwrap();
}
