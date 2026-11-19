//! Real source-built major-3 Person and major-2 declared/Agent consumers share one Host.
#![cfg(unix)]
#[allow(dead_code)]
mod support;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, os::unix::fs::PermissionsExt, path::Path};
use support::harness::{
    self, Addressing, ConversationClient, PACKAGE_ID, QUALIFICATION, open_core, open_host,
    open_mount, shutdown, write_qualification_provider,
};
use teamarness_package_host::{
    InstallPersonal, PackageHost, PackageHostCommand, PackageHostOutcome, PackageInstanceKey,
    ReloadPersonal, RemovePersonal, UiMountId, WorkspaceId,
};

const PERSON: &str = "dev.example.person-consumer";
const DECLARED: &str = "dev.example.declared-consumer";
const AGENT: &str = "dev.example.agent-consumer";

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn write_client(root: &Path, identity: &str, major: u16, declared: bool) {
    fs::create_dir_all(root.join("backend")).unwrap();
    fs::create_dir_all(root.join("ui")).unwrap();
    let binary = fs::read(env!("CARGO_BIN_EXE_person-port-consumer-under-test")).unwrap();
    let ui = b"export function mount() { return () => {}; }";
    let contract = serde_json::to_vec(&json!({"schemaVersion":1,"$defs":{},"requests":{
        "proof.execute":{"request":{"type":"object"},"response":{"type":"object"}}
    },"subscriptions":{}}))
    .unwrap();
    fs::write(root.join("backend/consumer"), &binary).unwrap();
    fs::set_permissions(
        root.join("backend/consumer"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    fs::write(root.join("ui/index.js"), ui).unwrap();
    fs::write(root.join("ui-contract.json"), &contract).unwrap();
    let mut manifest = json!({"schemaVersion":1,"identity":identity,"version":"1.0.0","presentation":{"displayName":identity},
        "protocol":{"major":1,"minMinor":0,"maxMinor":0},
        "backends":[{"target":teamarness_package_host::NATIVE_BACKEND_TARGET,"path":"backend/consumer","size":binary.len(),"sha256":digest(&binary)}],
        "ui":{"entrypoint":{"path":"ui/index.js","size":ui.len(),"sha256":digest(ui)},"assets":[],"contract":{"path":"ui-contract.json","sha256":digest(&contract)}},
        "capabilities":[{"identity":"teamarness.package-ports","major":major,"minMinor":0,"maxMinor":0}]});
    if declared {
        manifest["consumedPorts"] = json!([
            {"identity":PACKAGE_ID,"major":1,"minMinor":0,"maxMinor":99,"ports":[{"name":"runtime.guide","kind":"query"}]},
            {"identity":"dev.example.optional","major":1,"minMinor":0,"maxMinor":0,"required":false,"ports":[{"name":"optional.read","kind":"query"}]}
        ]);
    }
    fs::write(
        root.join("teamarness.package.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}
async fn install(host: &PackageHost, directory: &Path, identity: &str) {
    let result = host
        .execute(PackageHostCommand::InstallPersonal(InstallPersonal {
            directory: directory.to_owned(),
            expected_identity: identity.parse().unwrap(),
            accepted_risks: vec!["filesystem".into(), "network".into(), "process".into()],
        }))
        .await
        .unwrap_or_else(|error| panic!("install {identity}: {error:?}"));
    assert!(matches!(
        result,
        PackageHostOutcome::PersonalInstalled { .. }
    ));
}
async fn execute(host: &PackageHost, mount: &UiMountId, input: Value) -> Value {
    serde_json::to_value(
        ConversationClient { host, mount }
            .request(
                "person-proof",
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
fn observed(row: &Value, action: &str) -> Value {
    json!({"action":action,"access":"person","contract":row["contract"],"major":row["major"],"routeRevision":row["routeRevision"],"metadataSha256":row["metadataSha256"]})
}
fn update_provider(root: &Path, identity: &str, major: u16) {
    let mut ports: Value =
        serde_json::from_slice(&fs::read(root.join("ports.json")).unwrap()).unwrap();
    ports["contracts"][0]["major"] = major.into();
    // Independent contracts force a real continuation cursor. No call names are hard-coded
    // into the consumer, and missing/malformed guides stay visible but ineligible.
    let mut extra = ports["contracts"][0].clone();
    extra["identity"] = "dev.example.guide-less".into();
    extra["ports"].as_array_mut().unwrap().remove(0);
    ports["contracts"].as_array_mut().unwrap().push(extra);
    let mut malformed = ports["contracts"][0].clone();
    malformed["identity"] = "dev.example.malformed-guide".into();
    malformed["ports"][0]["response"] = json!({"type":"object"});
    ports["contracts"].as_array_mut().unwrap().push(malformed);
    for index in 0..29 {
        let mut extra = ports["contracts"][0].clone();
        extra["identity"] = format!("dev.example.page-{index:02}").into();
        ports["contracts"].as_array_mut().unwrap().push(extra);
    }
    let bytes = serde_json::to_vec(&ports).unwrap();
    fs::write(root.join("ports.json"), &bytes).unwrap();
    let path = root.join("teamarness.package.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["identity"] = identity.into();
    manifest["portContract"]["sha256"] = digest(&bytes).into();
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
}
async fn catalog(host: &PackageHost, mount: &UiMountId) -> Vec<Value> {
    let mut rows = Vec::new();
    let mut cursor = Value::Null;
    for _ in 0..4 {
        let mut input = json!({"action":"page","access":"person"});
        if !cursor.is_null() {
            input["cursor"] = cursor;
        }
        let page = success(host, mount, input).await;
        rows.extend(page["contracts"].as_array().unwrap().iter().cloned());
        cursor = page["nextCursor"].clone();
        if cursor.is_null() {
            return rows;
        }
    }
    panic!("fixture catalog exceeded four bounded pages");
}

// Verifies: TEAM.SYS.PKG-R03, TEAM.SYS.PKG-R09, TEAM.SYS.PKG-R12, TEAM.SYS.PKG-R13
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn person_discovers_live_unknown_provider_beside_legacy_consumers_without_rebuild() {
    let app = harness::StagedApplication::new();
    for identity in [DECLARED, AGENT] {
        write_client(
            &app.application_path.join(identity),
            identity,
            2,
            identity == DECLARED,
        );
    }
    let registration = app.application_path.join("application-packages.v1.json");
    let mut registrations: Value =
        serde_json::from_slice(&fs::read(&registration).unwrap()).unwrap();
    for identity in [DECLARED, AGENT] {
        registrations["packages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"directory":identity}));
    }
    fs::write(registration, serde_json::to_vec(&registrations).unwrap()).unwrap();
    let root = app.root.path();
    let core = open_core(Addressing::Loopback, &root.join("core")).await;
    let workspace = core.create_workspace("Person discovery").unwrap().id();
    let key = |identity: &str| {
        PackageInstanceKey::new(
            identity.parse().unwrap(),
            WorkspaceId::new(workspace.to_string()).unwrap(),
        )
    };
    let host = open_host(&app.application_path, root.join("host"), core.clone()).await;
    let person_dir = root.join(PERSON);
    write_client(&person_dir, PERSON, 3, false);
    install(&host, &person_dir, PERSON).await;
    let person = open_mount(&host, &key(PERSON)).await;
    // Person works before any Agent exists. No model or external Package is installed.
    assert!(core.bound_agent(workspace, PERSON).unwrap().is_none());
    assert!(
        catalog(&host, &person)
            .await
            .iter()
            .all(|row| row["contract"] != QUALIFICATION)
    );
    let source_hash = digest(&fs::read(person_dir.join("backend/consumer")).unwrap());
    let agent = core.add_agent(workspace, "Compatibility control").unwrap();
    core.bind_agent_package(workspace, agent, AGENT).unwrap();
    let declared = open_mount(&host, &key(DECLARED)).await;
    let agent_mount = open_mount(&host, &key(AGENT)).await;
    let legacy_guide = json!({"action":"query","access":"declared","contract":PACKAGE_ID,"major":1,"port":"runtime.guide"});
    assert!(success(&host, &declared, legacy_guide.clone()).await["markdown"].is_string());
    success(
        &host,
        &agent_mount,
        json!({"action":"enable","access":"agent","contract":PACKAGE_ID,"major":1}),
    )
    .await;
    assert!(success(&host, &agent_mount, json!({"action":"query","access":"agent","contract":PACKAGE_ID,"major":1,"port":"runtime.guide"})).await["markdown"].is_string());
    let optional = execute(&host, &declared, json!({"action":"query","access":"declared","contract":"dev.example.optional","major":1,"port":"optional.read"})).await;
    assert_eq!(optional["error"], "port_unavailable");
    let generations: Vec<_> = host
        .snapshot()
        .await
        .instances
        .iter()
        .filter(|i| [DECLARED, AGENT, PACKAGE_ID].contains(&i.key.package_identity.as_str()))
        .map(|i| (i.key.clone(), i.process_generation))
        .collect();
    let provider = root.join("unknown-provider");
    write_qualification_provider(&provider);
    update_provider(&provider, QUALIFICATION, 3);
    install(&host, &provider, QUALIFICATION).await;
    let rows = catalog(&host, &person).await;
    let row = rows
        .iter()
        .find(|r| r["contract"] == QUALIFICATION)
        .unwrap();
    assert_eq!(row["enabled"], false);
    assert_eq!(row["provider"]["identity"], QUALIFICATION);
    for name in ["dev.example.guide-less", "dev.example.malformed-guide"] {
        let ineligible = rows.iter().find(|r| r["contract"] == name).unwrap();
        assert_eq!(ineligible["guideEligibility"], "missing");
        assert_eq!(
            execute(&host, &person, observed(ineligible, "enable")).await["error"],
            "port_guide_required"
        );
    }
    let metadata = success(&host, &person, observed(row, "describe")).await;
    assert_eq!(metadata["$defs"]["offset"]["minimum"], 7);
    let mut query = observed(row, "query");
    query["port"] = "sample.read".into();
    query["request"] = json!({"offset":7});
    assert_eq!(
        execute(&host, &person, query.clone()).await["error"],
        "port_not_enabled"
    );
    success(&host, &person, observed(row, "enable")).await;
    assert_eq!(
        success(&host, &person, query.clone()).await,
        json!({"answer":42,"actor":{"kind":"person","identity":core.public_identity().to_string()}})
    );
    let mut guide = observed(row, "query");
    guide["port"] = "runtime.guide".into();
    assert!(
        success(&host, &person, guide).await["markdown"]
            .as_str()
            .unwrap()
            .contains("Qualification")
    );
    let first_page = success(&host, &person, json!({"action":"page","access":"person"})).await;
    assert!(first_page["nextCursor"].is_string());
    host.execute(PackageHostCommand::ReloadPersonal(ReloadPersonal {
        identity: QUALIFICATION.parse().unwrap(),
    }))
    .await
    .unwrap();
    assert_eq!(
        execute(&host, &person, query.clone()).await["error"],
        "port_catalog_changed"
    );
    assert_eq!(
        execute(
            &host,
            &person,
            json!({"action":"page","access":"person","cursor":first_page["nextCursor"]})
        )
        .await["error"],
        "port_catalog_changed"
    );
    // A changed offered major does not upgrade the durable selection.
    write_qualification_provider(&provider);
    update_provider(&provider, QUALIFICATION, 4);
    host.execute(PackageHostCommand::ReloadPersonal(ReloadPersonal {
        identity: QUALIFICATION.parse().unwrap(),
    }))
    .await
    .unwrap();
    let rows = catalog(&host, &person).await;
    let upgraded = rows
        .iter()
        .find(|r| r["contract"] == QUALIFICATION)
        .unwrap();
    assert_eq!(upgraded["major"], 4);
    assert_eq!(upgraded["enabledMajor"], 3);
    assert_eq!(upgraded["available"], false);
    host.execute(PackageHostCommand::RemovePersonal(RemovePersonal {
        identity: QUALIFICATION.parse().unwrap(),
    }))
    .await
    .unwrap();
    let rows = catalog(&host, &person).await;
    let missing = rows
        .iter()
        .find(|r| r["contract"] == QUALIFICATION)
        .unwrap();
    assert_eq!(missing["provider"], Value::Null);
    assert_eq!(missing["enabledMajor"], 3);
    success(
        &host,
        &person,
        json!({"action":"disable","access":"person","contract":QUALIFICATION}),
    )
    .await;
    // A different Package can offer the same contract; the old revision is never a selector.
    let replacement = "dev.example.replacement";
    write_qualification_provider(&provider);
    update_provider(&provider, replacement, 3);
    install(&host, &provider, replacement).await;
    assert_eq!(
        execute(&host, &person, query).await["error"],
        "port_catalog_changed"
    );
    let rows = catalog(&host, &person).await;
    let replaced = rows
        .iter()
        .find(|r| r["contract"] == QUALIFICATION)
        .unwrap();
    assert_eq!(replaced["provider"]["identity"], replacement);
    assert_eq!(replaced["enabled"], false);
    success(&host, &person, observed(replaced, "enable")).await;
    let mut query = observed(replaced, "query");
    query["port"] = "sample.read".into();
    query["request"] = json!({"offset":7});
    assert_eq!(success(&host, &person, query).await["answer"], 42);
    assert_eq!(
        digest(&fs::read(person_dir.join("backend/consumer")).unwrap()),
        source_hash
    );
    for (key, generation) in generations {
        assert_eq!(
            host.snapshot()
                .await
                .instances
                .iter()
                .find(|i| i.key == key)
                .unwrap()
                .process_generation,
            generation
        );
    }
    assert!(success(&host, &declared, legacy_guide).await["markdown"].is_string());
    let team = success(
        &host,
        &agent_mount,
        json!({"action":"page","access":"agent"}),
    )
    .await;
    assert!(
        team["contracts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["contract"] != QUALIFICATION)
    );
    shutdown(host).await;
    core.shutdown().await.unwrap();
}
