//! Explicit cross-repository proof. The external artifact must be freshly built by
//! scripts/verify-external-gateway.mjs; default suites never depend on its presence.
#![cfg(unix)]
#[allow(dead_code)]
mod support;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::Duration,
};
use support::harness::{
    Addressing, ConversationClient, open_core, open_host, open_mount, shutdown, stage_backlog,
};
use teamarness_package_host::{
    InstallPersonal, PackageHost, PackageHostCommand, PackageInstanceKey, UiMountId, WorkspaceId,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
};

const GATEWAY: &str = "dev.teamarness.external-gateway";
const BACKLOG: &str = "dev.teamarness.backlog";

fn gateway_directory() -> PathBuf {
    let path = PathBuf::from(
        std::env::var_os("TEAMARNESS_EXTERNAL_GATEWAY_PACKAGE_DIR")
            .expect("explicit qualification requires TEAMARNESS_EXTERNAL_GATEWAY_PACKAGE_DIR"),
    );
    assert!(
        path.join("teamarness.package.json").is_file(),
        "missing assembled gateway manifest"
    );
    path
}
async fn ui(host: &PackageHost, mount: &UiMountId, operation: &str, payload: Value) -> Value {
    serde_json::to_value(
        ConversationClient { host, mount }
            .request(
                "gateway-proof",
                operation,
                serde_json::from_value(payload).unwrap(),
            )
            .await,
    )
    .unwrap()
}
async fn install(host: &PackageHost, directory: PathBuf, identity: &str) {
    host.execute(PackageHostCommand::InstallPersonal(InstallPersonal {
        directory,
        expected_identity: identity.parse().unwrap(),
        accepted_risks: vec!["filesystem".into(), "network".into(), "process".into()],
    }))
    .await
    .unwrap();
}
async fn catalog(host: &PackageHost, mount: &UiMountId) -> Vec<Value> {
    let mut rows = Vec::new();
    let mut cursor = Value::Null;
    for _ in 0..4 {
        let mut input = json!({});
        if !cursor.is_null() {
            input["cursor"] = cursor;
        }
        let page = ui(host, mount, "external-gateway.catalog.page", input).await;
        rows.extend(page["contracts"].as_array().unwrap().iter().cloned());
        cursor = page["nextCursor"].clone();
        if cursor.is_null() {
            return rows;
        }
    }
    panic!("fixture catalog exceeded four bounded pages");
}
async fn select(host: &PackageHost, mount: &UiMountId, contract: &str, port: &str) {
    let rows = catalog(host, mount).await;
    let row = rows
        .iter()
        .find(|row| row["contract"] == contract)
        .expect("offered contract");
    let catalog = ui(
        host,
        mount,
        "external-gateway.catalog.page",
        json!({"contract":contract}),
    )
    .await;
    let tool = catalog["tools"]
        .as_array()
        .expect("tool page")
        .iter()
        .find(|tool| tool["port"] == port)
        .expect("offered tool");
    ui(
        host,
        mount,
        "external-gateway.tool.set",
        json!({"contract":contract,"major":row["major"],
        "provider":row["provider"],"port":port,"fingerprint":tool["fingerprint"],"enabled":true}),
    )
    .await;
}
async fn http(port: u16, bearer: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let body = body.map(|body| serde_json::to_vec(&body).unwrap());
    let mut wire = format!(
        "{} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {bearer}\r\n",
        if body.is_some() { "POST" } else { "GET" }
    );
    if let Some(body) = &body {
        wire.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    wire.push_str("\r\n");
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    socket.write_all(wire.as_bytes()).await.unwrap();
    if let Some(body) = body {
        socket.write_all(&body).await.unwrap();
    }
    let mut bytes = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(30),
        socket.take(2 * 1024 * 1024).read_to_end(&mut bytes),
    )
    .await
    .unwrap()
    .unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").expect("HTTP response");
    assert!(head.to_lowercase().contains("cache-control: no-store"));
    assert!(
        head.to_lowercase()
            .contains("x-content-type-options: nosniff")
    );
    let response = (
        head.split_whitespace().nth(1).unwrap().parse().unwrap(),
        serde_json::from_str(body).unwrap(),
    );
    // negative-wait: client pacing at the documented 8 requests/second, not a readiness guess.
    tokio::time::sleep(Duration::from_millis(125)).await;
    response
}
fn call(contract: &str, major: u16, port: &str, request: Value) -> Value {
    json!({"contract":contract,"major":major,"port":port,"request":request})
}

// Verifies: TEAM.SYS.PKG-R03, TEAM.SYS.PKG-R09, TEAM.SYS.PKG-R12, TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14, TEAM.SYS.PKG-R19
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "requires freshly built TEAMARNESS_EXTERNAL_GATEWAY_PACKAGE_DIR; run scripts/verify-external-gateway.mjs"]
async fn real_gateway_reads_backlog_without_agent_or_model_configuration() {
    let directory = gateway_directory();
    let root = tempfile::tempdir().unwrap();
    // macOS's temporary root may start with /var, a symlink. Supply a physical profile
    // path, as the gateway deliberately refuses symlink traversal for private storage.
    let profile = root.path().canonicalize().unwrap();
    let application = stage_backlog(&profile);
    let core = open_core(Addressing::Loopback, &profile.join("core")).await;
    let workspace = core.create_workspace("Gateway qualification").unwrap().id();
    let key = |identity: &str| {
        PackageInstanceKey::new(
            identity.parse().unwrap(),
            WorkspaceId::new(workspace.to_string()).unwrap(),
        )
    };
    let host = open_host(&application, profile.join("host"), core.clone()).await;
    install(&host, directory, GATEWAY).await;
    let gateway = open_mount(&host, &key(GATEWAY)).await;
    assert!(core.bound_agent(workspace, GATEWAY).unwrap().is_none());
    assert!(
        !host
            .authoring_catalog()
            .iter()
            .any(|entry| entry.package.identity.as_str() == "dev.teamarness.agent")
    );
    let initial = ui(&host, &gateway, "external-gateway.status.get", json!({})).await;
    assert_eq!(initial["running"], false);
    let backlog = open_mount(&host, &key(BACKLOG)).await;
    let epic = ui(
        &host,
        &backlog,
        "backlog.epic.create",
        json!({"title":"Launch"}),
    )
    .await;
    let ticket = ui(&host, &backlog, "backlog.ticket.create", json!({"epicId":epic["epicId"],"title":"Pick a date","body":"Ask the team.","priority":"high"})).await;
    for index in 1..65 {
        ui(&host, &backlog, "backlog.ticket.create", json!({"epicId":epic["epicId"],"title":format!("Fixture {index}"),"body":"Independent paging fixture.","priority":"medium"})).await;
    }
    select(&host, &gateway, BACKLOG, "ticket.list").await;
    select(&host, &gateway, BACKLOG, "ticket.get").await;
    let token = ui(
        &host,
        &gateway,
        "external-gateway.token.create",
        json!({"label":"qualification"}),
    )
    .await;
    let bearer = token["bearer"].as_str().unwrap();
    ui(
        &host,
        &gateway,
        "external-gateway.listener.set",
        json!({"enabled":true}),
    )
    .await;
    let status = ui(&host, &gateway, "external-gateway.status.get", json!({})).await;
    assert_eq!(status["running"], true);
    let port = status["port"].as_u64().unwrap() as u16;
    assert_eq!(http(port, "invalid", "/v1", None).await.0, 401);
    let manifest = http(port, bearer, "/v1", None).await;
    assert_eq!(manifest.0, 200);
    assert_eq!(manifest.1["workspace"]["id"], workspace.to_string());
    let read = http(
        port,
        bearer,
        "/v1/call",
        Some(call(
            BACKLOG,
            1,
            "ticket.get",
            json!({"ticketId":ticket["ticketId"]}),
        )),
    )
    .await;
    assert_eq!(read.0, 200, "read refusal: {}", read.1);
    assert_eq!(read.1["result"]["title"], "Pick a date");
    assert_eq!(read.1["result"]["body"], "Ask the team.");
    assert_eq!(
        read.1["result"]["creator"],
        core.public_identity().to_string()
    );
    let list = http(
        port,
        bearer,
        "/v1/call",
        Some(call(BACKLOG, 1, "ticket.list", json!({}))),
    )
    .await;
    assert_eq!(list.0, 200);
    assert_eq!(
        list.1["result"]["tickets"][0]["ticketId"],
        ticket["ticketId"]
    );
    assert_eq!(list.1["result"]["tickets"].as_array().unwrap().len(), 64);
    let next = http(
        port,
        bearer,
        "/v1/call",
        Some(call(
            BACKLOG,
            1,
            "ticket.list",
            json!({"cursor":list.1["result"]["nextCursor"]}),
        )),
    )
    .await;
    assert_eq!(next.0, 200);
    assert_eq!(next.1["result"]["tickets"].as_array().unwrap().len(), 1);
    assert_eq!(next.1["result"]["tickets"][0]["title"], "Fixture 64");
    assert!(next.1["result"].get("nextCursor").is_none());
    let missing = http(
        port,
        bearer,
        "/v1/call",
        Some(call(
            BACKLOG,
            1,
            "ticket.get",
            json!({"ticketId":"0".repeat(64)}),
        )),
    )
    .await;
    assert_eq!(missing.1["error"]["source"], "provider");
    assert_eq!(missing.1["error"]["code"], "backlog.not_found");
    assert_eq!(missing.1["error"]["retryable"], false);
    assert_eq!(missing.1["error"]["outcomeUncertain"], false);
    let schema = http(port, bearer, &format!("/v1/contracts/{BACKLOG}/1"), None).await;
    assert_eq!(schema.0, 200);
    assert_eq!(
        schema.1["projectionSha256"],
        digest(&serde_json::to_vec(&schema.1["schema"]).unwrap())
    );
    assert_eq!(
        schema.1["schema"]["$defs"]["recordId"]["pattern"],
        "^[0-9a-f]{64}$"
    );
    assert_eq!(
        schema.1["schema"]["contracts"][0]["ports"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["runtime.guide", "ticket.get", "ticket.list"]
    );
    assert_schema_references(&schema.1["schema"], &schema.1["schema"]);
    let control_path = profile.join("metadata-control");
    consumer(&control_path, "dev.example.metadata-control", 3, false);
    install(&host, control_path, "dev.example.metadata-control").await;
    let control_mount = open_mount(&host, &key("dev.example.metadata-control")).await;
    let available = ui(
        &host,
        &control_mount,
        "proof.execute",
        json!({"action":"page","access":"person"}),
    )
    .await;
    let observed = available["value"]["contracts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["contract"] == BACKLOG)
        .unwrap();
    let complete=ui(&host,&control_mount,"proof.execute",json!({"action":"describe","access":"person","contract":BACKLOG,"major":1,"routeRevision":observed["routeRevision"],"metadataSha256":observed["metadataSha256"]})).await;
    assert_eq!(complete["ok"], true);
    assert_eq!(
        schema.1["metadataSha256"],
        digest(&serde_json::to_vec(&complete["value"]).unwrap())
    );
    let guide = http(
        port,
        bearer,
        &format!("/v1/contracts/{BACKLOG}/1/guide"),
        None,
    )
    .await;
    assert_eq!(guide.0, 200);
    assert_eq!(
        guide.1["guideSha256"],
        digest(guide.1["markdown"].as_str().unwrap().as_bytes())
    );
    assert!(guide.1["markdown"].as_str().unwrap().contains("## Limits"));
    assert!(guide.1["preface"].as_str().unwrap().contains("ticket.get"));
    assert!(
        !guide.1["preface"]
            .as_str()
            .unwrap()
            .contains("ticket.create")
    );
    let mut observations = host.observe_ports();
    assert_eq!(
        http(
            port,
            bearer,
            "/v1/call",
            Some(call(BACKLOG, 1, "ticket.create", json!({})))
        )
        .await
        .0,
        403
    );
    assert_eq!(
        http(
            port,
            bearer,
            "/v1/call",
            Some(call(BACKLOG, 1, "backlog.changed", json!({})))
        )
        .await
        .0,
        403
    );
    assert!(
        observations.try_recv().is_err(),
        "refused writes/streams must not enqueue Provider work"
    );
    shutdown(host).await;
    assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err());
    core.shutdown().await.unwrap();
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn provider(root: &Path) {
    support::harness::write_qualification_provider(root);
    let binary = fs::read(env!(
        "CARGO_BIN_EXE_gateway-qualification-provider-under-test"
    ))
    .unwrap();
    fs::write(root.join("backend/provider"), &binary).unwrap();
    let contract = serde_json::to_vec(&json!({"schemaVersion":1,"$defs":{},"requests":{
        "proof.emit":{"request":{"type":"object","required":["value"],"properties":{"value":{}},"additionalProperties":false},"response":{"type":"object"}},
        "proof.end":{"request":{"type":"object","required":["error"],"properties":{"error":{"type":"boolean"}},"additionalProperties":false},"response":{"type":"object"}},
        "proof.command-release":{"request":{"type":"object","additionalProperties":false},"response":{"type":"object"}},
        "proof.guide":{"request":{"type":"object","properties":{"markdown":{"type":"string"}},"required":["markdown"],"additionalProperties":false},"response":{"type":"object"}},
        "proof.status":{"request":{"type":"object","additionalProperties":false},"response":{"type":"object"}},
        "proof.release":{"request":{"type":"object","additionalProperties":false},"response":{"type":"object"}}
    },"subscriptions":{}})).unwrap();
    fs::write(root.join("ui-contract.json"), &contract).unwrap();
    let path = root.join("teamarness.package.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["backends"][0]["size"] = binary.len().into();
    manifest["backends"][0]["sha256"] = digest(&binary).into();
    manifest["ui"]["contract"]["sha256"] = digest(&contract).into();
    manifest["protocol"] = json!({"major":1,"minMinor":1,"maxMinor":1});
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    update_ports(root, |ports| {
        ports["contracts"][0]["ports"].as_array_mut().unwrap().push(json!({"name":"sample.wait","kind":"query","description":"A controlled pending read.","request":{"type":"object","additionalProperties":false},"response":{"type":"object"}}));
        ports["contracts"][0]["ports"].as_array_mut().unwrap().extend([
            json!({"name":"sample.events","kind":"event","description":"Held listener setup and controlled values.","event":{}}),
            json!({"name":"sample.stream","kind":"query","description":"Arbitrary streamed JSON with controlled terminal.","request":{"type":"object","additionalProperties":false},"event":{}}),
        ]);
        for identity in [
            "dev.example.second",
            "dev.example.guide-less",
            "dev.teamarness.conversations",
        ] {
            let mut extra = ports["contracts"][0].clone();
            extra["identity"] = identity.into();
            if identity == "dev.example.guide-less" {
                extra["ports"].as_array_mut().unwrap().remove(0);
            }
            ports["contracts"].as_array_mut().unwrap().push(extra);
        }
    });
}
fn update_ports(root: &Path, change: impl FnOnce(&mut Value)) {
    let path = root.join("ports.json");
    let mut ports: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    change(&mut ports);
    let bytes = serde_json::to_vec(&ports).unwrap();
    fs::write(path, &bytes).unwrap();
    let path = root.join("teamarness.package.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["portContract"]["sha256"] = digest(&bytes).into();
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
}
async fn endpoint(host: &PackageHost, mount: &UiMountId) -> (u16, String) {
    let token = ui(
        host,
        mount,
        "external-gateway.token.create",
        json!({"label":"proof"}),
    )
    .await;
    ui(
        host,
        mount,
        "external-gateway.listener.set",
        json!({"enabled":true}),
    )
    .await;
    let status = ui(host, mount, "external-gateway.status.get", json!({})).await;
    assert_eq!(status["running"], true);
    (
        status["port"].as_u64().unwrap() as u16,
        token["bearer"].as_str().unwrap().to_owned(),
    )
}
async fn reload(host: &PackageHost, identity: &str) {
    host.execute(PackageHostCommand::ReloadPersonal(
        teamarness_package_host::ReloadPersonal {
            identity: identity.parse().unwrap(),
        },
    ))
    .await
    .unwrap();
}
async fn remove(host: &PackageHost, identity: &str) {
    host.execute(PackageHostCommand::RemovePersonal(
        teamarness_package_host::RemovePersonal {
            identity: identity.parse().unwrap(),
        },
    ))
    .await
    .unwrap();
}

fn consumer(root: &Path, identity: &str, major: u16, declared: bool) {
    fs::create_dir_all(root.join("backend")).unwrap();
    fs::create_dir_all(root.join("ui")).unwrap();
    let binary = fs::read(env!("CARGO_BIN_EXE_person-port-consumer-under-test")).unwrap();
    fs::write(root.join("backend/consumer"), &binary).unwrap();
    fs::set_permissions(
        root.join("backend/consumer"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let script = b"export function mount() { return () => {}; }";
    let contract = serde_json::to_vec(&json!({"schemaVersion":1,"$defs":{},"requests":{"proof.execute":{"request":{"type":"object"},"response":{"type":"object"}}},"subscriptions":{}})).unwrap();
    fs::write(root.join("ui/index.js"), script).unwrap();
    fs::write(root.join("ui-contract.json"), &contract).unwrap();
    let mut manifest = json!({"schemaVersion":1,"identity":identity,"version":"1.0.0","presentation":{"displayName":"Compatibility control"},"protocol":{"major":1,"minMinor":0,"maxMinor":0},
        "backends":[{"target":"aarch64-apple-darwin","path":"backend/consumer","size":binary.len(),"sha256":digest(&binary)}],
        "ui":{"entrypoint":{"path":"ui/index.js","size":script.len(),"sha256":digest(script)},"assets":[],"contract":{"path":"ui-contract.json","sha256":digest(&contract)}},
        "capabilities":[{"identity":"teamarness.package-ports","major":major,"minMinor":0,"maxMinor":0}]});
    if declared {
        manifest["consumedPorts"] = json!([{"identity":BACKLOG,"major":1,"minMinor":0,"maxMinor":0,"ports":[{"name":"runtime.guide","kind":"query"}]}]);
    }
    fs::write(
        root.join("teamarness.package.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}

// Verifies: TEAM.SYS.PKG-R03, TEAM.SYS.PKG-R12, TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "requires freshly built TEAMARNESS_EXTERNAL_GATEWAY_PACKAGE_DIR; run scripts/verify-external-gateway.mjs"]
async fn http_catalog_follows_exact_cursors_past_empty_page_without_gateway_rebuild() {
    const SELECTED: &str = support::harness::QUALIFICATION;
    const FILLER: &str = "dev.example.pagination-provider";
    let directory = gateway_directory();
    let manifest_before = fs::read(directory.join("teamarness.package.json")).unwrap();
    let manifest: Value = serde_json::from_slice(&manifest_before).unwrap();
    let artifact_paths = [
        manifest["backends"][0]["path"].as_str().unwrap(),
        manifest["ui"]["entrypoint"]["path"].as_str().unwrap(),
        manifest["ui"]["contract"]["path"].as_str().unwrap(),
    ];
    let before: Vec<_> = artifact_paths
        .iter()
        .map(|path| digest(&fs::read(directory.join(path)).unwrap()))
        .collect();
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().canonicalize().unwrap();
    let application = stage_backlog(&profile);
    let core = open_core(Addressing::Loopback, &profile.join("core")).await;
    let workspace = core
        .create_workspace("HTTP catalog pagination")
        .unwrap()
        .id();
    let key = PackageInstanceKey::new(
        GATEWAY.parse().unwrap(),
        WorkspaceId::new(workspace.to_string()).unwrap(),
    );
    let host = open_host(&application, profile.join("host"), core.clone()).await;
    install(&host, directory.clone(), GATEWAY).await;
    let gateway = open_mount(&host, &key).await;
    let (port, bearer) = endpoint(&host, &gateway).await;
    assert!(
        catalog(&host, &gateway)
            .await
            .iter()
            .all(|row| row["contract"] != SELECTED)
    );

    // Each Provider stays within the 32-contract admission bound. Together they
    // force actual Host pagination; these discovery-only filler routes stay off.
    let filler = profile.join("filler");
    provider(&filler);
    update_ports(&filler, |ports| {
        let template = ports["contracts"][0].clone();
        ports["contracts"] = (0..32)
            .map(|index| {
                let mut contract = template.clone();
                contract["identity"] = format!("dev.example.page-{index:02}").into();
                contract
            })
            .collect::<Vec<_>>()
            .into();
    });
    let path = filler.join("teamarness.package.json");
    let mut filler_manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    filler_manifest["identity"] = FILLER.into();
    fs::write(path, serde_json::to_vec(&filler_manifest).unwrap()).unwrap();
    install(&host, filler, FILLER).await;
    let selected = profile.join("selected");
    provider(&selected);
    install(&host, selected, SELECTED).await;
    let rows = catalog(&host, &gateway).await;
    assert!(rows.len() > 32, "must exercise real Host continuation");
    for index in 0..32 {
        assert!(rows.iter().any(
            |row| row["contract"] == format!("dev.example.page-{index:02}")
                && row["selected"] == false
        ));
    }
    assert!(
        rows.iter()
            .position(|row| row["contract"] == SELECTED)
            .unwrap()
            >= 32
    );
    select(&host, &gateway, SELECTED, "sample.read").await;

    let first = http(port, &bearer, "/v1/contracts", None).await;
    assert_eq!(first.0, 200);
    assert_eq!(
        first.1["contracts"],
        json!([]),
        "disabled first page must not leak metadata"
    );
    assert!(
        first.1["nextCursor"].is_string(),
        "empty page is not completion"
    );
    let mut cursor = first.1["nextCursor"].clone();
    let mut cursors = std::collections::BTreeSet::new();
    let mut discovered = Vec::new();
    for _ in 0..3 {
        let exact_cursor = cursor.as_str().expect("nonterminal cursor");
        assert!(
            cursors.insert(exact_cursor.to_owned()),
            "cursor must advance"
        );
        // Use the returned string verbatim: no synthetic cursor or client repair.
        let page = http(
            port,
            &bearer,
            &format!("/v1/contracts?cursor={exact_cursor}"),
            None,
        )
        .await;
        assert_eq!(page.0, 200, "returned HTTP cursor must be usable");
        discovered.extend(page.1["contracts"].as_array().unwrap().iter().cloned());
        cursor = page.1["nextCursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    assert!(
        cursor.is_null(),
        "fixture catalog must terminate within four pages"
    );
    assert_eq!(
        discovered.len(),
        1,
        "only the explicitly selected contract is exposed"
    );
    assert_eq!(discovered[0]["contract"], SELECTED);
    assert_eq!(discovered[0]["major"], 3);
    assert_eq!(
        http(
            port,
            &bearer,
            "/v1/call",
            Some(call(SELECTED, 3, "sample.read", json!({"offset":7})))
        )
        .await,
        (
            200,
            json!({"result":{"answer":42,"actor":{"kind":"person","identity":core.public_identity().to_string()}}})
        )
    );
    assert_eq!(
        fs::read(directory.join("teamarness.package.json")).unwrap(),
        manifest_before
    );
    for (path, expected) in artifact_paths.iter().zip(before) {
        assert_eq!(digest(&fs::read(directory.join(path)).unwrap()), expected);
    }
    shutdown(host).await;
    core.shutdown().await.unwrap();
}

// Verifies: TEAM.SYS.PKG-R02, TEAM.SYS.PKG-R03, TEAM.SYS.PKG-R09, TEAM.SYS.PKG-R12, TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "requires freshly built TEAMARNESS_EXTERNAL_GATEWAY_PACKAGE_DIR; run scripts/verify-external-gateway.mjs"]
async fn live_discovery_freshness_and_legacy_controls_do_not_rebuild_gateway() {
    const UNKNOWN: &str = support::harness::QUALIFICATION;
    let directory = gateway_directory();
    let manifest_before = fs::read(directory.join("teamarness.package.json")).unwrap();
    let manifest: Value = serde_json::from_slice(&manifest_before).unwrap();
    let backend_before = digest(
        &fs::read(directory.join(manifest["backends"][0]["path"].as_str().unwrap())).unwrap(),
    );
    let ui_before = digest(
        &fs::read(directory.join(manifest["ui"]["entrypoint"]["path"].as_str().unwrap())).unwrap(),
    );
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().canonicalize().unwrap();
    let application = stage_backlog(&profile);
    let core = open_core(Addressing::Loopback, &profile.join("core")).await;
    let workspace = core.create_workspace("Discovery").unwrap().id();
    let key = |identity: &str| {
        PackageInstanceKey::new(
            identity.parse().unwrap(),
            WorkspaceId::new(workspace.to_string()).unwrap(),
        )
    };
    let host = open_host(&application, profile.join("host"), core.clone()).await;
    install(&host, directory.clone(), GATEWAY).await;
    let gateway = open_mount(&host, &key(GATEWAY)).await;
    let (port, bearer) = endpoint(&host, &gateway).await;
    let before = ui(&host, &gateway, "external-gateway.catalog.page", json!({})).await;
    assert!(
        before["contracts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["contract"] != UNKNOWN)
    );
    assert_eq!(
        http(port, &bearer, "/v1/contracts", None).await.1["contracts"],
        json!([])
    );
    // Source-built legacy consumers are controls, not an installed Agent Package or a
    // precondition of gateway setup. They use independent caller selection namespaces.
    for (identity, major, declared) in [
        ("dev.example.legacy-declared", 2, true),
        ("dev.example.legacy-agent", 2, false),
        ("dev.example.revision-control", 3, false),
    ] {
        let path = profile.join(identity);
        consumer(&path, identity, major, declared);
        install(&host, path, identity).await;
    }
    let declared = open_mount(&host, &key("dev.example.legacy-declared")).await;
    let agent = core.add_agent(workspace, "Compatibility only").unwrap();
    core.bind_agent_package(workspace, agent, "dev.example.legacy-agent")
        .unwrap();
    let agent_mount = open_mount(&host, &key("dev.example.legacy-agent")).await;
    let revision_mount = open_mount(&host, &key("dev.example.revision-control")).await;
    let legacy_guide = json!({"action":"query","access":"declared","contract":BACKLOG,"major":1,"port":"runtime.guide"});
    assert_eq!(
        ui(&host, &declared, "proof.execute", legacy_guide.clone()).await["ok"],
        true
    );
    assert_eq!(
        ui(
            &host,
            &agent_mount,
            "proof.execute",
            json!({"action":"enable","access":"agent","contract":BACKLOG,"major":1})
        )
        .await["ok"],
        true
    );
    assert_eq!(ui(&host,&agent_mount,"proof.execute",json!({"action":"query","access":"agent","contract":BACKLOG,"major":1,"port":"runtime.guide"})).await["ok"],true);
    let path = profile.join("unknown");
    provider(&path);
    install(&host, path.clone(), UNKNOWN).await;
    let rows = ui(&host, &gateway, "external-gateway.catalog.page", json!({})).await;
    for contract in [UNKNOWN, "dev.example.second"] {
        let row = rows["contracts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["contract"] == contract)
            .unwrap();
        assert_eq!(row["provider"], UNKNOWN);
        assert_eq!(row["selected"], false);
        select(&host, &gateway, contract, "sample.read").await;
    }
    let row = rows["contracts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["contract"] == "dev.example.guide-less")
        .unwrap();
    assert_eq!(row["guideEligibility"], "missing");
    for contract in ["dev.example.guide-less", "dev.teamarness.conversations"] {
        assert_eq!(
            http(
                port,
                &bearer,
                "/v1/call",
                Some(call(contract, 3, "sample.read", json!({"offset":7})))
            )
            .await
            .0,
            403
        );
    }
    // Contract identity is selectable only after exact explicit opt-in.
    select(
        &host,
        &gateway,
        "dev.teamarness.conversations",
        "sample.read",
    )
    .await;
    assert_eq!(
        http(
            port,
            &bearer,
            "/v1/call",
            Some(call(
                "dev.teamarness.conversations",
                3,
                "sample.read",
                json!({"offset":7})
            ))
        )
        .await
        .0,
        200
    );
    let query = call(UNKNOWN, 3, "sample.read", json!({"offset":7}));
    let expected = json!({"result":{"answer":42,"actor":{"kind":"person","identity":core.public_identity().to_string()}}});
    assert_eq!(
        http(port, &bearer, "/v1/call", Some(query.clone())).await,
        (200, expected.clone())
    );
    assert_eq!(
        http(
            port,
            &bearer,
            "/v1/call",
            Some(call(
                "dev.example.second",
                3,
                "sample.read",
                json!({"offset":7})
            ))
        )
        .await,
        (200, expected.clone())
    );
    for (field, value) in [
        ("actor", json!({"kind":"agent","identity":"untrusted"})),
        ("workspace", json!("other")),
        ("provider", json!("other")),
        ("grant", json!("other")),
    ] {
        let mut malicious = query.clone();
        malicious[field] = value;
        assert_eq!(
            http(port, &bearer, "/v1/call", Some(malicious)).await.0,
            400
        );
    }
    let provider_mount = open_mount(&host, &key(UNKNOWN)).await;
    let guide_path = format!("/v1/contracts/{UNKNOWN}/3/guide");
    let first_guide = http(port, &bearer, &guide_path, None).await;
    let changed_guide = "# Changed live guide\nDo not enable sample.write.\n";
    ui(
        &host,
        &provider_mount,
        "proof.guide",
        json!({"markdown":changed_guide}),
    )
    .await;
    let second_guide = http(port, &bearer, &guide_path, None).await;
    assert_eq!(second_guide.1["markdown"], changed_guide);
    assert_eq!(
        second_guide.1["guideSha256"],
        digest(changed_guide.as_bytes())
    );
    assert_ne!(first_guide.1["guideSha256"], second_guide.1["guideSha256"]);
    assert_eq!(second_guide.1["major"], 3);
    assert_eq!(second_guide.1["minor"], 0);

    assert_eq!(
        http(
            port,
            &bearer,
            "/v1/call",
            Some(call(UNKNOWN, 3, "sample.write", json!({"value":"denied"})))
        )
        .await
        .0,
        403
    );
    assert_eq!(
        ui(&host, &provider_mount, "proof.status", json!({})).await["writes"],
        0
    );
    let control = ui(
        &host,
        &revision_mount,
        "proof.execute",
        json!({"action":"page","access":"person"}),
    )
    .await;
    let old = control["value"]["contracts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["contract"] == UNKNOWN)
        .unwrap()
        .clone();
    update_ports(&path, |p| {
        p["contracts"][0]["description"] = "Description-only and additive evolution".into();
        p["contracts"][0]["ports"].as_array_mut().unwrap().push(json!({"name":"added.read","kind":"query","description":"New read stays off.","request":{"type":"object","additionalProperties":false},"response":{"type":"object"}}));
    });
    reload(&host, UNKNOWN).await;
    assert_eq!(
        http(port, &bearer, "/v1/call", Some(query.clone())).await,
        (200, expected)
    );
    assert_eq!(
        http(
            port,
            &bearer,
            "/v1/call",
            Some(call(UNKNOWN, 3, "added.read", json!({})))
        )
        .await
        .0,
        403
    );
    let stale = ui(&host,&revision_mount,"proof.execute",json!({"action":"query","access":"person","contract":UNKNOWN,"major":3,"port":"sample.read","request":{"offset":7},"routeRevision":old["routeRevision"]})).await;
    assert_eq!(stale["error"], "port_catalog_changed");
    update_ports(&path, |p| {
        p["$defs"]["offset"]["maximum"] = 8.into();
    });
    reload(&host, UNKNOWN).await;
    assert_eq!(
        http(port, &bearer, "/v1/call", Some(query.clone())).await.0,
        403
    );
    let review = ui(
        &host,
        &gateway,
        "external-gateway.catalog.page",
        json!({"contract":UNKNOWN}),
    )
    .await;
    assert_eq!(
        review["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["port"] == "sample.read")
            .unwrap()["reviewRequired"],
        true
    );
    select(&host, &gateway, UNKNOWN, "sample.read").await;
    assert_eq!(
        http(port, &bearer, "/v1/call", Some(query.clone())).await.0,
        200
    );
    update_ports(&path, |p| {
        p["contracts"][0]["ports"]
            .as_array_mut()
            .unwrap()
            .retain(|p| p["name"] != "sample.read");
    });
    reload(&host, UNKNOWN).await;
    assert_eq!(
        http(port, &bearer, "/v1/call", Some(query.clone())).await.0,
        403
    );
    remove(&host, UNKNOWN).await;
    // A retained missing choice is removable without a live Provider identity.
    ui(
        &host,
        &gateway,
        "external-gateway.package.set",
        json!({"provider":null,"enabled":false,"contracts":[{"contract":UNKNOWN,"major":3}]}),
    )
    .await;
    provider(&path);
    let manifest_path = path.join("teamarness.package.json");
    let mut replacement: Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    replacement["identity"] = "dev.example.replacement".into();
    fs::write(&manifest_path, serde_json::to_vec(&replacement).unwrap()).unwrap();
    install(&host, path.clone(), "dev.example.replacement").await;
    // The second contract was not disabled above: Provider substitution still requires review.
    assert_eq!(
        http(
            port,
            &bearer,
            "/v1/call",
            Some(call(
                "dev.example.second",
                3,
                "sample.read",
                json!({"offset":7})
            ))
        )
        .await
        .0,
        409
    );
    select(&host, &gateway, UNKNOWN, "sample.read").await;
    assert_eq!(http(port, &bearer, "/v1/call", Some(query)).await.0, 200);
    ui(&host,&gateway,"external-gateway.package.set",json!({"provider":"dev.example.replacement","enabled":false,"contracts":[{"contract":UNKNOWN,"major":3},{"contract":"dev.example.second","major":3}]})).await;
    assert_eq!(
        http(
            port,
            &bearer,
            "/v1/call",
            Some(call(UNKNOWN, 3, "sample.read", json!({"offset":7})))
        )
        .await
        .0,
        403
    );
    remove(&host, "dev.example.replacement").await;
    provider(&path);
    let mut conversations_provider: Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    conversations_provider["identity"] = "dev.teamarness.conversations".into();
    fs::write(
        &manifest_path,
        serde_json::to_vec(&conversations_provider).unwrap(),
    )
    .unwrap();
    install(&host, path.clone(), "dev.teamarness.conversations").await;
    let conversations = ui(&host, &gateway, "external-gateway.catalog.page", json!({})).await;
    assert_eq!(
        conversations["contracts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["contract"] == UNKNOWN)
            .unwrap()["selected"],
        false
    );
    assert_eq!(
        http(
            port,
            &bearer,
            "/v1/call",
            Some(call(UNKNOWN, 3, "sample.read", json!({"offset":7})))
        )
        .await
        .0,
        403
    );
    // Provider identity is not a denylist either. Existing off state grants nothing.
    select(&host, &gateway, UNKNOWN, "sample.read").await;
    assert_eq!(
        http(
            port,
            &bearer,
            "/v1/call",
            Some(call(UNKNOWN, 3, "sample.read", json!({"offset":7})))
        )
        .await
        .0,
        200
    );
    assert_eq!(
        ui(&host, &declared, "proof.execute", legacy_guide).await["ok"],
        true
    );
    assert_eq!(
        fs::read(directory.join("teamarness.package.json")).unwrap(),
        manifest_before
    );
    assert_eq!(
        digest(
            &fs::read(directory.join(manifest["backends"][0]["path"].as_str().unwrap())).unwrap()
        ),
        backend_before
    );
    assert_eq!(
        digest(
            &fs::read(directory.join(manifest["ui"]["entrypoint"]["path"].as_str().unwrap()))
                .unwrap()
        ),
        ui_before
    );
    shutdown(host).await;
    core.shutdown().await.unwrap();
}

// Verifies: TEAM.SYS.PKG-R02, TEAM.SYS.PKG-R03, TEAM.SYS.PKG-R05, TEAM.SYS.PKG-R09, TEAM.SYS.PKG-R13
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "requires freshly built TEAMARNESS_EXTERNAL_GATEWAY_PACKAGE_DIR; run scripts/verify-external-gateway.mjs"]
async fn real_host_lifecycle_retains_only_committed_workspace_setup() {
    use teamarness_package_host::{CloseUiMount, RecoverPackage, RestartPersonal, SuspendPackage};
    let directory = gateway_directory();
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().canonicalize().unwrap();
    let application = stage_backlog(&profile);
    let core = open_core(Addressing::Loopback, &profile.join("core")).await;
    let first = core.create_workspace("First").unwrap().id();
    let second = core.create_workspace("Second").unwrap().id();
    let key = |workspace: teamarness_core::WorkspaceId| {
        PackageInstanceKey::new(
            GATEWAY.parse().unwrap(),
            WorkspaceId::new(workspace.to_string()).unwrap(),
        )
    };
    let host = open_host(&application, profile.join("host"), core.clone()).await;
    install(&host, directory.clone(), GATEWAY).await;
    let first_mount = open_mount(&host, &key(first)).await;
    let second_mount = open_mount(&host, &key(second)).await;
    let (port, bearer) = endpoint(&host, &first_mount).await;
    let (other_port, other_bearer) = endpoint(&host, &second_mount).await;
    assert_ne!(port, other_port);
    assert_eq!(http(other_port, &bearer, "/v1", None).await.0, 401);
    assert_eq!(http(port, &other_bearer, "/v1", None).await.0, 401);
    let second_token = ui(
        &host,
        &first_mount,
        "external-gateway.token.create",
        json!({"label":"second client"}),
    )
    .await;
    let second_bearer = second_token["bearer"].as_str().unwrap();
    select(&host, &first_mount, BACKLOG, "ticket.list").await;
    for token in [&bearer, second_bearer] {
        assert_eq!(
            http(
                port,
                token,
                "/v1/call",
                Some(call(BACKLOG, 1, "ticket.list", json!({})))
            )
            .await
            .0,
            200
        );
    }
    ui(
        &host,
        &first_mount,
        "external-gateway.package.set",
        json!({"provider":BACKLOG,"enabled":false,"contracts":[{"contract":BACKLOG,"major":1}]}),
    )
    .await;
    for token in [&bearer, second_bearer] {
        assert_eq!(
            http(
                port,
                token,
                "/v1/call",
                Some(call(BACKLOG, 1, "ticket.list", json!({})))
            )
            .await
            .0,
            403
        );
    }
    let new_token = ui(
        &host,
        &first_mount,
        "external-gateway.token.regenerate",
        json!({"id":second_token["id"]}),
    )
    .await;
    assert_eq!(http(port, second_bearer, "/v1", None).await.0, 401);
    assert_eq!(
        http(port, new_token["bearer"].as_str().unwrap(), "/v1", None)
            .await
            .0,
        200
    );
    ui(
        &host,
        &first_mount,
        "external-gateway.token.revoke",
        json!({"id":second_token["id"]}),
    )
    .await;
    assert_eq!(
        http(port, new_token["bearer"].as_str().unwrap(), "/v1", None)
            .await
            .0,
        401
    );
    host.execute(PackageHostCommand::CloseUiMount(CloseUiMount {
        mount: first_mount.clone(),
    }))
    .await
    .unwrap();
    assert_eq!(
        http(port, &bearer, "/v1", None).await.0,
        200,
        "hiding UI does not stop backend"
    );
    host.execute(PackageHostCommand::Suspend(SuspendPackage {
        instance: key(first),
    }))
    .await
    .unwrap();
    assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err());
    assert_eq!(http(other_port, &other_bearer, "/v1", None).await.0, 200);
    host.execute(PackageHostCommand::Recover(RecoverPackage {
        instance: key(first),
    }))
    .await
    .unwrap();
    assert_eq!(http(port, &bearer, "/v1", None).await.0, 200);
    host.execute(PackageHostCommand::RestartPersonal(RestartPersonal {
        instance: key(first),
    }))
    .await
    .unwrap();
    assert_eq!(http(port, &bearer, "/v1", None).await.0, 200);
    reload(&host, GATEWAY).await;
    assert_eq!(http(port, &bearer, "/v1", None).await.0, 200);
    assert_eq!(http(other_port, &other_bearer, "/v1", None).await.0, 200);
    remove(&host, GATEWAY).await;
    assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err());
    assert!(TcpStream::connect(("127.0.0.1", other_port)).await.is_err());
    let occupied = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    install(&host, directory.clone(), GATEWAY).await;
    let mount = open_mount(&host, &key(first)).await;
    let status = ui(&host, &mount, "external-gateway.status.get", json!({})).await;
    assert_eq!(status["running"], false);
    assert_eq!(status["desiredEnabled"], true);
    assert_eq!(status["port"], port);
    assert!(status["warning"].is_string());
    let free = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let new_port = free.local_addr().unwrap().port();
    drop(free);
    ui(
        &host,
        &mount,
        "external-gateway.port.set",
        json!({"port":new_port}),
    )
    .await;
    assert_eq!(http(new_port, &bearer, "/v1", None).await.0, 200);
    drop(occupied);
    assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err());
    shutdown(host).await;
    assert!(TcpStream::connect(("127.0.0.1", new_port)).await.is_err());
    let host = open_host(&application, profile.join("host"), core.clone()).await;
    assert_eq!(http(new_port, &bearer, "/v1", None).await.0, 200);
    let mount = open_mount(&host, &key(first)).await;
    ui(
        &host,
        &mount,
        "external-gateway.disable-and-revoke-all",
        json!({}),
    )
    .await;
    assert!(TcpStream::connect(("127.0.0.1", new_port)).await.is_err());
    host.execute(PackageHostCommand::RestartPersonal(RestartPersonal {
        instance: key(first),
    }))
    .await
    .unwrap();
    let mount = open_mount(&host, &key(first)).await;
    let status = ui(&host, &mount, "external-gateway.status.get", json!({})).await;
    assert_eq!(status["desiredEnabled"], false);
    assert_eq!(status["tokens"], json!([]));
    ui(
        &host,
        &mount,
        "external-gateway.listener.set",
        json!({"enabled":true}),
    )
    .await;
    assert_eq!(http(new_port, &bearer, "/v1", None).await.0, 401);
    shutdown(host).await;
    core.shutdown().await.unwrap();
}

// Verifies: TEAM.SYS.PKG-R02, TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "requires freshly built TEAMARNESS_EXTERNAL_GATEWAY_PACKAGE_DIR; run scripts/verify-external-gateway.mjs"]
async fn host_overflow_and_earlier_sdk_terminals_release_local_capacity_without_replay() {
    use support::harness::{QUALIFICATION, poll_short};
    use teamarness_package_host::{RecoverPackage, SuspendPackage};
    let directory = gateway_directory();
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().canonicalize().unwrap();
    let application = stage_backlog(&profile);
    let core = open_core(Addressing::Loopback, &profile.join("core")).await;
    let workspace = core.create_workspace("Owned reads").unwrap().id();
    let key = |identity: &str| {
        PackageInstanceKey::new(
            identity.parse().unwrap(),
            WorkspaceId::new(workspace.to_string()).unwrap(),
        )
    };
    let host = open_host(&application, profile.join("host"), core.clone()).await;
    install(&host, directory, GATEWAY).await;
    let gateway = open_mount(&host, &key(GATEWAY)).await;
    let path = profile.join("provider");
    provider(&path);
    install(&host, path, QUALIFICATION).await;
    let provider_mount = open_mount(&host, &key(QUALIFICATION)).await;
    select(&host, &gateway, QUALIFICATION, "sample.wait").await;
    select(&host, &gateway, QUALIFICATION, "sample.read").await;
    let (port, bearer) = endpoint(&host, &gateway).await;
    let request = call(QUALIFICATION, 3, "sample.wait", json!({}));
    let body = serde_json::to_string(&request).unwrap();
    let mut observer = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    observer.write_all(format!("POST /v1/call HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {bearer}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
    assert!(
        poll_short(|| async {
            ui(&host, &provider_mount, "proof.status", json!({})).await["waiting"] == 1
        })
        .await
    );
    drop(observer); // Observer loss alone is not the SDK terminal and must not replay work.
    let start = std::time::Instant::now();
    let mut calls = Vec::new();
    for _ in 0..11 {
        let bearer = bearer.clone();
        let request = request.clone();
        calls.push(tokio::spawn(async move {
            http(port, &bearer, "/v1/call", Some(request)).await
        }));
    }
    let mut codes = Vec::new();
    for task in calls {
        let response = task.await.unwrap();
        assert_ne!(response.0, 200);
        codes.push(response.1["error"]["code"].as_str().unwrap().to_owned());
    }
    assert!(
        codes.iter().any(|c| c == "overloaded"),
        "unchanged Host resource limits: {codes:?}"
    );
    assert!(
        codes.iter().any(|c| c == "deadline_exceeded"),
        "real Host/SDK terminal: {codes:?}"
    );
    assert!(
        start.elapsed() < Duration::from_secs(25),
        "the HTTP upper budget does not extend upstream deadlines"
    );
    let status = ui(&host, &provider_mount, "proof.status", json!({})).await;
    let waiting = status["waiting"].as_u64().unwrap();
    assert!(
        (1..=7).contains(&waiting),
        "Host byte pool admits at most seven nonempty ordinary calls"
    );
    host.execute(PackageHostCommand::Suspend(SuspendPackage {
        instance: key(GATEWAY),
    }))
    .await
    .unwrap();
    assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err());
    host.execute(PackageHostCommand::Recover(RecoverPackage {
        instance: key(GATEWAY),
    }))
    .await
    .unwrap();
    assert_eq!(
        http(
            port,
            &bearer,
            "/v1/call",
            Some(call(QUALIFICATION, 3, "sample.read", json!({"offset":7})))
        )
        .await
        .0,
        200,
        "SDK timeout is terminal; no generation-long quarantine"
    );
    assert_eq!(
        ui(&host, &provider_mount, "proof.status", json!({})).await["waiting"],
        waiting,
        "no automatic replay after timeout or disconnect"
    );
    // A positively observed already-authorized read may finish after a scope transition.
    let bearer_copy = bearer.clone();
    let accepted =
        tokio::spawn(async move { http(port, &bearer_copy, "/v1/call", Some(request)).await });
    assert!(
        poll_short(|| async {
            ui(&host, &provider_mount, "proof.status", json!({})).await["waiting"] == waiting + 1
        })
        .await
    );
    let gateway = open_mount(&host, &key(GATEWAY)).await;
    ui(&host,&gateway,"external-gateway.package.set",json!({"provider":QUALIFICATION,"enabled":false,"contracts":[{"contract":QUALIFICATION,"major":3}]})).await;
    ui(&host, &provider_mount, "proof.release", json!({})).await;
    assert_eq!(
        accepted.await.unwrap(),
        (200, json!({"result":{"released":true}}))
    );
    assert_eq!(
        http(
            port,
            &bearer,
            "/v1/call",
            Some(call(QUALIFICATION, 3, "sample.wait", json!({})))
        )
        .await
        .0,
        403
    );
    assert_eq!(
        ui(&host, &provider_mount, "proof.status", json!({})).await["waiting"],
        waiting + 1
    );
    shutdown(host).await;
    core.shutdown().await.unwrap();
}

// Verifies: TEAM.SYS.PKG-R03, TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "requires freshly built TEAMARNESS_EXTERNAL_GATEWAY_PACKAGE_DIR; run scripts/verify-external-gateway.mjs"]
async fn bound_gateway_refuses_person_dispatch_without_actor_fallback() {
    use support::harness::QUALIFICATION;
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().canonicalize().unwrap();
    let application = stage_backlog(&profile);
    let core = open_core(Addressing::Loopback, &profile.join("core")).await;
    let workspace = core.create_workspace("Bound caller").unwrap().id();
    // Create the control before listener startup: Agent creation also changes canonical
    // membership, which correctly pauses the listener while its watcher reconciles.
    let agent = core.add_agent(workspace, "Caller-mode control").unwrap();
    let key = |identity: &str| {
        PackageInstanceKey::new(
            identity.parse().unwrap(),
            WorkspaceId::new(workspace.to_string()).unwrap(),
        )
    };
    let host = open_host(&application, profile.join("host"), core.clone()).await;
    install(&host, gateway_directory(), GATEWAY).await;
    let gateway = open_mount(&host, &key(GATEWAY)).await;
    let path = profile.join("provider");
    provider(&path);
    install(&host, path, QUALIFICATION).await;
    let provider_mount = open_mount(&host, &key(QUALIFICATION)).await;
    select(&host, &gateway, QUALIFICATION, "sample.read").await;
    let (port, bearer) = endpoint(&host, &gateway).await;
    core.bind_agent_package(workspace, agent, GATEWAY).unwrap();
    let refused = http(
        port,
        &bearer,
        "/v1/call",
        Some(call(QUALIFICATION, 3, "sample.read", json!({"offset":7}))),
    )
    .await;
    assert_eq!(refused.1["error"]["code"], "port_actor_mode_invalid");
    assert_eq!(
        ui(&host, &provider_mount, "proof.status", json!({})).await["reads"],
        0
    );
    shutdown(host).await;
    core.shutdown().await.unwrap();
}

// Verifies: TEAM.SYS.PKG-R02, TEAM.SYS.PKG-R03, TEAM.SYS.PKG-R05, TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "requires freshly built TEAMARNESS_EXTERNAL_GATEWAY_PACKAGE_DIR; run scripts/verify-external-gateway.mjs"]
async fn canonical_membership_loss_stops_idle_routes_and_connections_and_retained_restart() {
    use support::harness::{QUALIFICATION, create_invitation, introduce};
    use teamarness_package_host::{
        OpenUiSubscription, PackageHostNotification, PackageHostSubscriptionSelector, PackageValue,
        RestartPersonal, UiSubscriptionId,
    };
    let directory = gateway_directory();
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().canonicalize().unwrap();
    let application = stage_backlog(&profile);
    let manager = open_core(Addressing::Loopback, &profile.join("manager")).await;
    let core = open_core(Addressing::Loopback, &profile.join("member")).await;
    let workspace = manager.create_workspace("Membership").unwrap().id();
    core.join_workspace(&create_invitation(&manager, workspace).await)
        .await
        .unwrap();
    introduce(Addressing::Loopback, &[&core, &manager]);
    let other_workspace = core.create_workspace("Unaffected").unwrap().id();
    let key = |identity: &str| {
        PackageInstanceKey::new(
            identity.parse().unwrap(),
            WorkspaceId::new(workspace.to_string()).unwrap(),
        )
    };
    let other_key = PackageInstanceKey::new(
        GATEWAY.parse().unwrap(),
        WorkspaceId::new(other_workspace.to_string()).unwrap(),
    );
    let host = open_host(&application, profile.join("host"), core.clone()).await;
    install(&host, directory.clone(), GATEWAY).await;
    let gateway = open_mount(&host, &key(GATEWAY)).await;
    let other_mount = open_mount(&host, &other_key).await;
    let path = profile.join("provider");
    provider(&path);
    install(&host, path, QUALIFICATION).await;
    let provider_mount = open_mount(&host, &key(QUALIFICATION)).await;
    select(&host, &gateway, QUALIFICATION, "sample.read").await;
    let (port, bearer) = endpoint(&host, &gateway).await;
    let (other_port, other_bearer) = endpoint(&host, &other_mount).await;
    assert_ne!(port, other_port);
    let routes = [
        "/v1".to_owned(),
        "/v1/onboarding".into(),
        "/v1/contracts".into(),
        format!("/v1/contracts/{QUALIFICATION}/3"),
        format!("/v1/contracts/{QUALIFICATION}/3/guide"),
        "/v1/call".into(),
    ];
    // Exercise every surface and prime complete schema metadata before canonical removal.
    for route in &routes {
        let body = (route == "/v1/call")
            .then(|| call(QUALIFICATION, 3, "sample.read", json!({"offset":7})));
        assert_eq!(http(port, &bearer, route, body).await.0, 200);
    }
    let saved = ui(&host, &gateway, "external-gateway.status.get", json!({})).await;
    let subscription = UiSubscriptionId::new("membership-stop-status").unwrap();
    host.execute(PackageHostCommand::OpenUiSubscription(OpenUiSubscription {
        mount: gateway.clone(),
        subscription: subscription.clone(),
        operation: "external-gateway.status.watch".into(),
        payload: PackageValue::Object(Default::default()),
    }))
    .await
    .unwrap();
    let mut events = host
        .subscribe(PackageHostSubscriptionSelector::UiEvents {
            mount: gateway.clone(),
            subscription,
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let PackageHostNotification::UiEvent(event) = events.changed().await.unwrap() {
                let status = serde_json::to_value(event.payload).unwrap();
                if status["running"] == true {
                    break;
                }
            }
        }
    })
    .await
    .expect("passive status subscription ready before canonical loss");
    let mut old_connections = Vec::new();
    for route in &routes {
        let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        // Leave the final header terminator unsent: no request may drive the idle stop.
        socket.write_all(format!("{} {route} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {bearer}\r\n",if route=="/v1/call" {"POST"} else {"GET"}).as_bytes()).await.unwrap();
        old_connections.push(socket);
    }
    manager
        .revoke_member(workspace, core.public_identity())
        .unwrap();
    for operation in manager.export_membership_operations_for_tests(workspace) {
        core.apply_membership_operation_for_tests(&operation)
            .unwrap();
    }
    assert!(
        core.workspace_person(workspace, core.public_identity())
            .unwrap()
            .is_none()
    );
    // Observe the pre-existing push stream, not status polling or triggering HTTP requests.
    // This includes membership delivery/SDK observation; the five-second drain starts later.
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let PackageHostNotification::UiEvent(event) = events.changed().await.unwrap() {
                let status = serde_json::to_value(event.payload).unwrap();
                if status["running"] == false && status["warning"] == "membership_removed" {
                    break;
                }
            }
        }
    })
    .await
    .expect("canonical-loss notification stops the idle listener");
    for (route, mut socket) in routes.iter().zip(old_connections) {
        let mut bytes = Vec::new();
        let result = tokio::time::timeout(Duration::from_secs(6), socket.read_to_end(&mut bytes))
            .await
            .expect("old connection retires within bounded drain");
        assert!(
            result.is_ok()
                || result.is_err_and(|e| e.kind() == std::io::ErrorKind::ConnectionReset)
        );
        assert!(
            bytes.is_empty(),
            "retired {route} must not return protected data"
        );
        assert!(
            TcpStream::connect(("127.0.0.1", port)).await.is_err(),
            "{route} has no listener after removal"
        );
    }
    assert_eq!(
        ui(&host, &provider_mount, "proof.status", json!({})).await["reads"],
        1,
        "no post-removal Provider dispatch"
    );
    assert_eq!(http(other_port, &other_bearer, "/v1", None).await.0, 200);
    assert_retained_removed(&host, &gateway, port, &saved).await;
    host.execute(PackageHostCommand::RestartPersonal(RestartPersonal {
        instance: key(GATEWAY),
    }))
    .await
    .unwrap();
    let gateway = open_mount(&host, &key(GATEWAY)).await;
    assert_retained_removed(&host, &gateway, port, &saved).await;
    reload(&host, GATEWAY).await;
    let gateway = open_mount(&host, &key(GATEWAY)).await;
    assert_retained_removed(&host, &gateway, port, &saved).await;
    assert_eq!(http(other_port, &other_bearer, "/v1", None).await.0, 200);
    remove(&host, GATEWAY).await;
    install(&host, directory, GATEWAY).await;
    let gateway = open_mount(&host, &key(GATEWAY)).await;
    assert_retained_removed(&host, &gateway, port, &saved).await;
    assert_eq!(http(other_port, &other_bearer, "/v1", None).await.0, 200);
    assert_eq!(
        ui(&host, &provider_mount, "proof.status", json!({})).await["reads"],
        1
    );
    shutdown(host).await;
    core.shutdown().await.unwrap();
    manager.shutdown().await.unwrap();
}

async fn assert_retained_removed(host: &PackageHost, mount: &UiMountId, port: u16, saved: &Value) {
    let status = ui(host, mount, "external-gateway.status.get", json!({})).await;
    assert_eq!(status["running"], false);
    assert_eq!(status["warning"], "membership_removed");
    for field in [
        "desiredEnabled",
        "port",
        "tokens",
        "selectedContracts",
        "selectedTools",
    ] {
        assert_eq!(
            status[field], saved[field],
            "membership loss preserves {field}"
        );
    }
    assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err());
}

fn assert_schema_references(document: &Value, node: &Value) {
    match node {
        Value::Object(fields) => {
            if let Some(reference) = fields.get("$ref") {
                let reference = reference.as_str().unwrap();
                assert!(reference.starts_with("#/$defs/"));
                assert!(
                    document.pointer(&reference[1..]).is_some(),
                    "missing complete schema reference {reference}"
                );
            }
            for value in fields.values() {
                assert_schema_references(document, value);
            }
        }
        Value::Array(values) => {
            for value in values {
                assert_schema_references(document, value);
            }
        }
        _ => {}
    }
}

/// A disposable real Host, source Backlog and external gateway. No normal profile is touched.
struct GatewayProof {
    _root: tempfile::TempDir,
    profile: PathBuf,
    core: std::sync::Arc<teamarness_core::CoreRuntime>,
    workspace: teamarness_core::WorkspaceId,
    host: PackageHost,
    gateway: UiMountId,
    port: u16,
    bearer: String,
}
impl GatewayProof {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().canonicalize().unwrap();
        let application = stage_backlog(&profile);
        let core = open_core(Addressing::Loopback, &profile.join("core")).await;
        let workspace = core
            .create_workspace("Confirmed gateway proof")
            .unwrap()
            .id();
        let host = open_host(&application, profile.join("host"), core.clone()).await;
        install(&host, gateway_directory(), GATEWAY).await;
        let key = PackageInstanceKey::new(
            GATEWAY.parse().unwrap(),
            WorkspaceId::new(workspace.to_string()).unwrap(),
        );
        let gateway = open_mount(&host, &key).await;
        let (port, bearer) = endpoint(&host, &gateway).await;
        Self {
            _root: root,
            profile,
            core,
            workspace,
            host,
            gateway,
            port,
            bearer,
        }
    }
    fn key(&self, identity: &str) -> PackageInstanceKey {
        PackageInstanceKey::new(
            identity.parse().unwrap(),
            WorkspaceId::new(self.workspace.to_string()).unwrap(),
        )
    }
    async fn select(&self, contract: &str, ports: &[&str]) {
        for port in ports {
            select(&self.host, &self.gateway, contract, port).await;
        }
    }
    async fn call(&self, contract: &str, major: u16, port: &str, request: Value) -> (u16, Value) {
        http(
            self.port,
            &self.bearer,
            "/v1/call",
            Some(call(contract, major, port, request)),
        )
        .await
    }
    async fn result(&self, contract: &str, major: u16, port: &str, request: Value) -> Value {
        let (status, body) = self.call(contract, major, port, request).await;
        assert_eq!(status, 200, "{port}: {body}");
        body["result"].clone()
    }
    async fn finish(self) {
        shutdown(self.host).await;
        self.core.shutdown().await.unwrap();
    }
}

async fn post_socket(port: u16, bearer: &str, route: &str, body: Value) -> TcpStream {
    let body = serde_json::to_string(&body).unwrap();
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    socket.write_all(format!("POST {route} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {bearer}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
    socket
}
struct Ndjson {
    socket: BufReader<TcpStream>,
    pending: Vec<u8>,
}
impl Ndjson {
    async fn established(socket: TcpStream) -> Self {
        tokio::time::timeout(Duration::from_secs(25), async {
            let mut socket = BufReader::new(socket);
            let mut head = String::new();
            loop {
                let mut line = String::new();
                assert!(socket.read_line(&mut line).await.unwrap() > 0);
                head.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            assert!(head.starts_with("HTTP/1.1 200"), "opening refusal: {head}");
            for header in [
                "content-type: application/x-ndjson",
                "cache-control: no-store",
                "x-content-type-options: nosniff",
                "transfer-encoding: chunked",
            ] {
                assert!(head.to_lowercase().contains(header), "missing {header}");
            }
            Self {
                socket,
                pending: Vec::new(),
            }
        })
        .await
        .expect("bounded confirmed HTTP opening")
    }
    async fn next(&mut self) -> Option<Value> {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
                    let line: Vec<_> = self.pending.drain(..=end).collect();
                    return Some(serde_json::from_slice(&line).unwrap());
                }
                let mut line = String::new();
                match self.socket.read_line(&mut line).await {
                    Ok(0) => return None,
                    Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => return None,
                    result => {
                        result.unwrap();
                    }
                }
                let size = usize::from_str_radix(line.trim(), 16).unwrap();
                if size == 0 {
                    return None;
                }
                assert!(size <= 256 * 1024 + 4096);
                let mut bytes = vec![0; size + 2];
                self.socket.read_exact(&mut bytes).await.unwrap();
                assert_eq!(&bytes[size..], b"\r\n");
                self.pending.extend_from_slice(&bytes[..size]);
            }
        })
        .await
        .expect("bounded stream progress")
    }
    async fn ready(&mut self, contract: &str, port: &str, kind: &str) {
        assert_eq!(
            self.next().await,
            Some(
                json!({"type":"ready","contract":contract,"major":if contract == BACKLOG {1} else {3},"port":port,"kind":kind,"delivery":"stream"})
            )
        );
    }
}

// Verifies: TEAM.SYS.PKG-R09, TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "requires freshly built TEAMARNESS_EXTERNAL_GATEWAY_PACKAGE_DIR; run scripts/verify-external-gateway.mjs"]
async fn unknown_provider_confirmed_setup_is_held_until_listener_registration() {
    use support::harness::{QUALIFICATION, poll_short};
    let proof = GatewayProof::new().await;
    let path = proof.profile.join("unknown");
    provider(&path);
    install(&proof.host, path, QUALIFICATION).await;
    let control = open_mount(&proof.host, &proof.key(QUALIFICATION)).await;
    proof.select(QUALIFICATION, &["sample.events"]).await;
    let mut opening = post_socket(
        proof.port,
        &proof.bearer,
        "/v1/subscribe",
        call(QUALIFICATION, 3, "sample.events", Value::Null),
    )
    .await;
    assert!(
        poll_short(|| async {
            ui(&proof.host, &control, "proof.status", json!({})).await["setupWaiting"] == 1
        })
        .await,
        "Provider handler must enter held setup"
    );
    let mut byte = [0];
    assert!(
        tokio::time::timeout(Duration::from_millis(200), opening.read(&mut byte))
            .await
            .is_err(),
        "no HTTP 200 or ready before real setup returns"
    );
    ui(&proof.host, &control, "proof.release", json!({})).await;
    let mut stream = Ndjson::established(opening).await;
    stream.ready(QUALIFICATION, "sample.events", "event").await;
    ui(
        &proof.host,
        &control,
        "proof.emit",
        json!({"value":{"first":"post-ready change"}}),
    )
    .await;
    assert_eq!(
        stream.next().await,
        Some(json!({"type":"value","value":{"first":"post-ready change"}}))
    );
    drop(stream);
    proof.finish().await;
}

// Verifies: TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14, TEAM.SYS.BACKLOG-R09
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "requires freshly built TEAMARNESS_EXTERNAL_GATEWAY_PACKAGE_DIR; run scripts/verify-external-gateway.mjs"]
async fn real_ticket_writes_conflicts_and_backlog_event_follow_confirmed_ready() {
    let proof = GatewayProof::new().await;
    let native = open_mount(&proof.host, &proof.key(BACKLOG)).await;
    // Epic creation remains native, not a gateway business operation.
    let epic = ui(
        &proof.host,
        &native,
        "backlog.epic.create",
        json!({"title":"Gateway Epic"}),
    )
    .await;
    proof
        .select(
            BACKLOG,
            &[
                "ticket.create",
                "ticket.get",
                "ticket.update",
                "backlog.changed",
            ],
        )
        .await;
    let created = proof
        .result(
            BACKLOG,
            1,
            "ticket.create",
            json!({"epicId":epic["epicId"],"title":"Outside Ticket","body":"Initial text"}),
        )
        .await;
    let socket = post_socket(
        proof.port,
        &proof.bearer,
        "/v1/subscribe",
        call(BACKLOG, 1, "backlog.changed", Value::Null),
    )
    .await;
    let mut stream = Ndjson::established(socket).await;
    stream.ready(BACKLOG, "backlog.changed", "event").await;
    // Initial read follows confirmed ready. The next change needs no prior event.
    let current = proof
        .result(
            BACKLOG,
            1,
            "ticket.get",
            json!({"ticketId":created["ticketId"]}),
        )
        .await;
    assert_eq!(current["version"], created["version"]);
    assert_eq!(current["creator"], proof.core.public_identity().to_string());
    assert_eq!(current["body"], "Initial text");
    let updated = proof.result(BACKLOG, 1, "ticket.update", json!({"ticketId":created["ticketId"],"baseVersion":current["version"],"body":"Maintained text","status":"in_progress"})).await;
    assert_ne!(updated["version"], current["version"]);
    assert_eq!(
        stream.next().await,
        Some(json!({"type":"value","value":{}}))
    );
    let maintained = proof
        .result(
            BACKLOG,
            1,
            "ticket.get",
            json!({"ticketId":created["ticketId"]}),
        )
        .await;
    assert_eq!(maintained["version"], updated["version"]);
    assert_eq!(maintained["body"], "Maintained text");
    assert_eq!(
        maintained["lastEditor"],
        proof.core.public_identity().to_string()
    );
    let stale = proof.call(BACKLOG, 1, "ticket.update", json!({"ticketId":created["ticketId"],"baseVersion":current["version"],"body":"Must not replace"})).await;
    assert_eq!(
        stale.1["error"],
        json!({"source":"provider","code":"backlog.stale_version","retryable":false,"outcomeUncertain":false,"details":{"ticket":maintained}})
    );
    assert_ne!(stale.0, 200);
    assert_eq!(
        proof
            .result(
                BACKLOG,
                1,
                "ticket.get",
                json!({"ticketId":created["ticketId"]})
            )
            .await,
        maintained
    );
    // Exact event disable closes the idle stream while the read stays selected.
    let page = ui(
        &proof.host,
        &proof.gateway,
        "external-gateway.catalog.page",
        json!({"contract":BACKLOG}),
    )
    .await;
    let event = page["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["port"] == "backlog.changed")
        .unwrap();
    ui(&proof.host, &proof.gateway, "external-gateway.tool.set", json!({"contract":BACKLOG,"major":1,"provider":BACKLOG,"port":"backlog.changed","fingerprint":event["fingerprint"],"enabled":false})).await;
    assert_stream_closed(&mut stream).await;
    assert_eq!(
        proof
            .result(
                BACKLOG,
                1,
                "ticket.get",
                json!({"ticketId":created["ticketId"]})
            )
            .await,
        maintained
    );
    let denied = http(
        proof.port,
        &proof.bearer,
        "/v1/subscribe",
        Some(call(BACKLOG, 1, "backlog.changed", Value::Null)),
    )
    .await;
    assert_eq!(denied.0, 403);
    proof.select(BACKLOG, &["backlog.changed"]).await;
    let socket = post_socket(
        proof.port,
        &proof.bearer,
        "/v1/subscribe",
        call(BACKLOG, 1, "backlog.changed", Value::Null),
    )
    .await;
    let mut reconnected = Ndjson::established(socket).await;
    reconnected.ready(BACKLOG, "backlog.changed", "event").await;
    assert_eq!(
        proof
            .result(
                BACKLOG,
                1,
                "ticket.get",
                json!({"ticketId":created["ticketId"]})
            )
            .await,
        maintained,
        "explicit reconnect reconciles rather than replaying"
    );
    drop(reconnected);
    proof.finish().await;
}

async fn assert_stream_closed(stream: &mut Ndjson) {
    tokio::time::timeout(Duration::from_secs(6), async {
        // Already-admitted bounded frames/socket bytes may drain after denial.
        while let Some(frame) = stream.next().await {
            assert!(matches!(frame["type"].as_str(), Some("value" | "end")));
            if frame["type"] == "end" {
                assert_eq!(frame["reason"], "error");
                assert!(stream.next().await.is_none());
                return;
            }
        }
    })
    .await
    .expect("idle denial closes within bounded listener drain");
}

// Verifies: TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "requires freshly built TEAMARNESS_EXTERNAL_GATEWAY_PACKAGE_DIR; run scripts/verify-external-gateway.mjs"]
async fn real_document_create_then_revise_preserves_opaque_versions_and_person_records() {
    use support::harness::{PackageStage, stage_package_with};
    const DOCUMENTS: &str = "dev.teamarness.documents";
    let proof = GatewayProof::new().await;
    let app = stage_package_with(&proof.profile.join("documents"), &PackageStage::documents());
    install(&proof.host, app.join(DOCUMENTS), DOCUMENTS).await;
    let native = open_mount(&proof.host, &proof.key(DOCUMENTS)).await;
    let collection = ui(
        &proof.host,
        &native,
        "documents.collection.create",
        json!({"name":"Gateway Collection"}),
    )
    .await;
    proof
        .select(
            DOCUMENTS,
            &["document.create", "document.revise", "document.markdown"],
        )
        .await;
    let created = proof
        .result(
            DOCUMENTS,
            1,
            "document.create",
            json!({"collectionId":collection["id"],"title":"Outside Document"}),
        )
        .await;
    let empty = proof
        .result(
            DOCUMENTS,
            1,
            "document.markdown",
            json!({"documentId":created["documentId"]}),
        )
        .await;
    assert_eq!(empty["markdown"], "");
    assert_eq!(empty["version"], created["version"]);
    assert!(empty["version"].is_string());
    let first = proof.result(DOCUMENTS, 1, "document.revise", json!({"documentId":created["documentId"],"baseVersion":empty["version"],"markdown":"First body\n"})).await;
    assert_eq!(first["changed"], true);
    let read = proof
        .result(
            DOCUMENTS,
            1,
            "document.markdown",
            json!({"documentId":created["documentId"]}),
        )
        .await;
    assert_eq!(read["markdown"], "First body\n");
    assert_eq!(read["version"], first["version"]);
    assert_ne!(read["version"], empty["version"]);
    let second = proof.result(DOCUMENTS, 1, "document.revise", json!({"documentId":created["documentId"],"baseVersion":read["version"],"markdown":"Maintained body\n"})).await;
    assert_eq!(second["changed"], true);
    let final_read = proof
        .result(
            DOCUMENTS,
            1,
            "document.markdown",
            json!({"documentId":created["documentId"]}),
        )
        .await;
    assert_eq!(
        final_read,
        json!({"markdown":"Maintained body\n","version":second["version"]})
    );
    let mut records = proof
        .core
        .subscribe_records(proof.workspace, DOCUMENTS, None)
        .unwrap();
    let page = records.next_page(256).unwrap();
    for (purpose, count) in [("document.create", 1), ("document.edit", 2)] {
        let records: Vec<_> = page
            .events
            .iter()
            .filter(|e| e.purpose() == purpose)
            .collect();
        assert_eq!(records.len(), count);
        assert!(
            records
                .iter()
                .all(|r| r.author().to_string() == proof.core.public_identity().to_string()),
            "Host-derived Person signs every Document write"
        );
    }
    proof.finish().await;
}

// Verifies: TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14, TEAM.SYS.PKG-R18
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "requires freshly built TEAMARNESS_EXTERNAL_GATEWAY_PACKAGE_DIR; run scripts/verify-external-gateway.mjs"]
async fn real_draft_text_and_128kib_binary_updates_preserve_revision_conflict_details() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    const EXHIBITS: &str = "dev.teamarness.exhibits";
    let proof = GatewayProof::new().await;
    support::harness::stage_exhibits(&proof.profile.join("application"));
    install(
        &proof.host,
        proof.profile.join("application").join(EXHIBITS),
        EXHIBITS,
    )
    .await;
    proof
        .select(
            EXHIBITS,
            &[
                "file.write",
                "file.put",
                "files.missing",
                "draft.update",
                "exhibit.read",
                "file.text",
                "file.read",
            ],
        )
        .await;
    let text = proof
        .result(
            EXHIBITS,
            1,
            "file.write",
            json!({"text":"<h1>First Draft</h1>\n"}),
        )
        .await;
    let bytes: Vec<u8> = (0..128 * 1024).map(|i| (i % 251) as u8).collect();
    let sha = digest(&bytes);
    let missing = proof
        .result(EXHIBITS, 1, "files.missing", json!({"sha256s":[sha]}))
        .await;
    assert_eq!(missing["missing"][0]["sha256"], sha);
    let upload = json!({"sha256":sha,"size":bytes.len(),"chunkIndex":0,"chunkCount":1,"data":STANDARD.encode(&bytes)});
    assert!(
        serde_json::to_vec(&call(EXHIBITS, 1, "file.put", upload.clone()))
            .unwrap()
            .len()
            < 256 * 1024
    );
    assert_eq!(
        proof.result(EXHIBITS, 1, "file.put", upload).await,
        json!({"complete":true})
    );
    assert_eq!(
        proof
            .result(
                EXHIBITS,
                1,
                "files.missing",
                json!({"sha256s":[sha,text["sha256"]]})
            )
            .await,
        json!({"missing":[]})
    );
    let files =
        json!([{"path":"index.html","sha256":text["sha256"]},{"path":"data.wasm","sha256":sha}]);
    let first = proof.result(EXHIBITS, 1, "draft.update", json!({"exhibitId":"gateway-proof","title":"Outside Draft","entry":"index.html","files":files,"expectedRevision":0})).await;
    assert_eq!(first["revision"], 1);
    let new_text = proof
        .result(
            EXHIBITS,
            1,
            "file.write",
            json!({"text":"<h1>Maintained Draft</h1>\n"}),
        )
        .await;
    let files = json!([{"path":"index.html","sha256":new_text["sha256"]},{"path":"data.wasm","sha256":sha}]);
    let second = proof.result(EXHIBITS, 1, "draft.update", json!({"exhibitId":"gateway-proof","title":"Maintained Draft","entry":"index.html","files":files,"expectedRevision":first["revision"]})).await;
    assert_eq!(second["revision"], 2);
    let read = proof
        .result(
            EXHIBITS,
            1,
            "exhibit.read",
            json!({"exhibitId":"gateway-proof"}),
        )
        .await;
    assert_eq!(read["revision"], second["revision"]);
    assert_eq!(read["title"], "Maintained Draft");
    assert_eq!(read["fileCount"], 2);
    assert_eq!(
        read["updatedBy"],
        json!({"kind":"person","identity":proof.core.public_identity().to_string()})
    );
    assert_eq!(
        proof
            .result(
                EXHIBITS,
                1,
                "file.text",
                json!({"sha256":new_text["sha256"]})
            )
            .await["text"],
        "<h1>Maintained Draft</h1>\n"
    );
    let binary = proof
        .result(
            EXHIBITS,
            1,
            "file.read",
            json!({"sha256":sha,"chunkIndex":0}),
        )
        .await;
    assert_eq!(
        STANDARD.decode(binary["data"].as_str().unwrap()).unwrap(),
        bytes
    );
    let stale = proof.call(EXHIBITS, 1, "draft.update", json!({"exhibitId":"gateway-proof","title":"Stale Draft","entry":"index.html","files":[{"path":"index.html","sha256":text["sha256"]}],"expectedRevision":first["revision"]})).await;
    assert_ne!(stale.0, 200);
    assert_eq!(
        stale.1["error"],
        json!({"source":"provider","code":"exhibits.revision_conflict","retryable":false,"outcomeUncertain":false,"details":{"revision":2}})
    );
    assert_eq!(
        proof
            .result(
                EXHIBITS,
                1,
                "exhibit.read",
                json!({"exhibitId":"gateway-proof"})
            )
            .await,
        read
    );
    proof.finish().await;
}

fn gateway_artifacts() -> Vec<(String, String)> {
    let directory = gateway_directory();
    let manifest: Value =
        serde_json::from_slice(&fs::read(directory.join("teamarness.package.json")).unwrap())
            .unwrap();
    [
        "teamarness.package.json",
        manifest["backends"][0]["path"].as_str().unwrap(),
        manifest["ui"]["entrypoint"]["path"].as_str().unwrap(),
        manifest["ui"]["contract"]["path"].as_str().unwrap(),
    ]
    .into_iter()
    .map(|p| (p.to_owned(), digest(&fs::read(directory.join(p)).unwrap())))
    .collect()
}

// Verifies: TEAM.SYS.PKG-R03, TEAM.SYS.PKG-R09, TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "requires freshly built TEAMARNESS_EXTERNAL_GATEWAY_PACKAGE_DIR; run scripts/verify-external-gateway.mjs"]
async fn unknown_live_provider_four_shapes_values_terminals_generation_and_legacy_refusal() {
    use support::harness::QUALIFICATION;
    let before = gateway_artifacts();
    let proof = GatewayProof::new().await;
    assert!(
        !catalog(&proof.host, &proof.gateway)
            .await
            .iter()
            .any(|r| r["contract"] == QUALIFICATION)
    );
    let path = proof.profile.join("unknown");
    provider(&path);
    install(&proof.host, path.clone(), QUALIFICATION).await;
    let control = open_mount(&proof.host, &proof.key(QUALIFICATION)).await;
    assert_eq!(
        proof
            .call(QUALIFICATION, 3, "sample.write", json!({"value":"off"}))
            .await
            .0,
        403
    );
    proof
        .select(
            QUALIFICATION,
            &[
                "sample.read",
                "sample.write",
                "sample.stream",
                "sample.events",
            ],
        )
        .await;
    assert_eq!(
        proof
            .result(QUALIFICATION, 3, "sample.read", json!({"offset":7}))
            .await,
        json!({"answer":42,"actor":{"kind":"person","identity":proof.core.public_identity().to_string()}})
    );
    assert_eq!(
        proof
            .result(QUALIFICATION, 3, "sample.write", json!({"value":"once"}))
            .await,
        json!({"written":true})
    );
    assert_eq!(
        proof
            .call(QUALIFICATION, 3, "sample.stream", json!({}))
            .await
            .0,
        400
    );
    let schema = http(
        proof.port,
        &proof.bearer,
        &format!("/v1/contracts/{QUALIFICATION}/3"),
        None,
    )
    .await;
    assert_eq!(schema.0, 200);
    let ports = schema.1["schema"]["contracts"][0]["ports"]
        .as_array()
        .unwrap();
    assert_eq!(
        ports
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "runtime.guide",
            "sample.events",
            "sample.read",
            "sample.stream",
            "sample.write"
        ]
    );
    assert_schema_references(&schema.1["schema"], &schema.1["schema"]);
    let socket = post_socket(
        proof.port,
        &proof.bearer,
        "/v1/subscribe",
        call(QUALIFICATION, 3, "sample.stream", json!({})),
    )
    .await;
    let mut stream = Ndjson::established(socket).await;
    stream.ready(QUALIFICATION, "sample.stream", "query").await;
    for value in [
        Value::Null,
        json!(false),
        json!(42),
        json!("scalar"),
        json!([1,null,{"nested":true}]),
        json!({"type":"ready","value":{"result":7}}),
    ] {
        ui(&proof.host, &control, "proof.emit", json!({"value":value})).await;
        assert_eq!(
            stream.next().await,
            Some(json!({"type":"value","value":value}))
        );
    }
    ui(&proof.host, &control, "proof.end", json!({"error":false})).await;
    assert_eq!(
        stream.next().await,
        Some(json!({"type":"end","reason":"completed"}))
    );
    assert!(stream.next().await.is_none());
    let socket = post_socket(
        proof.port,
        &proof.bearer,
        "/v1/subscribe",
        call(QUALIFICATION, 3, "sample.stream", json!({})),
    )
    .await;
    let mut stream = Ndjson::established(socket).await;
    stream.ready(QUALIFICATION, "sample.stream", "query").await;
    reload(&proof.host, QUALIFICATION).await;
    assert_stream_closed(&mut stream).await;
    // An actual selected 1.0 Provider still serves reads, but cannot confirm a stream.
    let manifest_path = path.join("teamarness.package.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["protocol"] = json!({"major":1,"minMinor":0,"maxMinor":0});
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    reload(&proof.host, QUALIFICATION).await;
    assert_eq!(
        proof
            .result(QUALIFICATION, 3, "sample.read", json!({"offset":7}))
            .await["answer"],
        42
    );
    let refused = http(
        proof.port,
        &proof.bearer,
        "/v1/subscribe",
        Some(call(QUALIFICATION, 3, "sample.stream", json!({}))),
    )
    .await;
    assert_ne!(refused.0, 200);
    assert_eq!(refused.1["error"]["code"], "capability_version_unsupported");
    manifest["protocol"] = json!({"major":1,"minMinor":1,"maxMinor":1});
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    reload(&proof.host, QUALIFICATION).await;
    let socket = post_socket(
        proof.port,
        &proof.bearer,
        "/v1/subscribe",
        call(QUALIFICATION, 3, "sample.stream", json!({})),
    )
    .await;
    let mut fresh = Ndjson::established(socket).await;
    fresh.ready(QUALIFICATION, "sample.stream", "query").await;
    let control = open_mount(&proof.host, &proof.key(QUALIFICATION)).await;
    ui(
        &proof.host,
        &control,
        "proof.emit",
        json!({"value":"explicit reconnect"}),
    )
    .await;
    assert_eq!(
        fresh.next().await,
        Some(json!({"type":"value","value":"explicit reconnect"}))
    );
    drop(fresh);
    assert_eq!(
        gateway_artifacts(),
        before,
        "live install and every shape leave gateway artifact bytes unchanged"
    );
    let socket = post_socket(
        proof.port,
        &proof.bearer,
        "/v1/subscribe",
        call(QUALIFICATION, 3, "sample.stream", json!({})),
    )
    .await;
    let mut stream = Ndjson::established(socket).await;
    stream.ready(QUALIFICATION, "sample.stream", "query").await;
    ui(&proof.host, &control, "proof.end", json!({"error":true})).await;
    let end = stream.next().await.unwrap();
    assert_eq!(end["type"], "end");
    assert_eq!(end["reason"], "error");
    assert_eq!(end["error"]["code"], "proof.stream_failed");
    assert!(stream.next().await.is_none());
    proof.finish().await;
}

// Verifies: TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "requires freshly built TEAMARNESS_EXTERNAL_GATEWAY_PACKAGE_DIR; run scripts/verify-external-gateway.mjs"]
async fn held_command_observer_loss_and_policy_change_execute_once_with_host_person() {
    use support::harness::{QUALIFICATION, poll_short};
    let proof = GatewayProof::new().await;
    let path = proof.profile.join("unknown");
    provider(&path);
    install(&proof.host, path, QUALIFICATION).await;
    let control = open_mount(&proof.host, &proof.key(QUALIFICATION)).await;
    proof
        .select(QUALIFICATION, &["sample.write", "sample.read"])
        .await;
    let observer = post_socket(
        proof.port,
        &proof.bearer,
        "/v1/call",
        call(QUALIFICATION, 3, "sample.write", json!({"value":"held"})),
    )
    .await;
    assert!(
        poll_short(|| async {
            ui(&proof.host, &control, "proof.status", json!({})).await["commandWaiting"] == 1
        })
        .await
    );
    drop(observer);
    ui(&proof.host, &proof.gateway, "external-gateway.package.set", json!({"provider":QUALIFICATION,"enabled":false,"contracts":[{"contract":QUALIFICATION,"major":3}]})).await;
    assert_eq!(
        proof
            .call(
                QUALIFICATION,
                3,
                "sample.write",
                json!({"value":"later denied"})
            )
            .await
            .0,
        403
    );
    ui(&proof.host, &control, "proof.command-release", json!({})).await;
    assert!(
        poll_short(|| async {
            ui(&proof.host, &control, "proof.status", json!({})).await["rows"]
                .as_array()
                .unwrap()
                .len()
                == 1
        })
        .await
    );
    let status = ui(&proof.host, &control, "proof.status", json!({})).await;
    assert_eq!(status["writes"], 1);
    assert_eq!(
        status["rows"],
        json!([{"value":"held","actor":{"kind":"person","identity":proof.core.public_identity().to_string()}}])
    );
    proof.select(QUALIFICATION, &["sample.read"]).await;
    assert_eq!(
        proof
            .result(QUALIFICATION, 3, "sample.read", json!({"offset":7}))
            .await["answer"],
        42
    );
    assert_eq!(
        ui(&proof.host, &control, "proof.status", json!({})).await["writes"],
        1,
        "reconciliation never replays the command"
    );
    proof.finish().await;
}
