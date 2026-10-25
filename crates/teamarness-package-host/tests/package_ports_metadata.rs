use std::str::FromStr;
mod common;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::fs;
use teamarness_package_host::{
    OpenUiMount, PackageHost, PackageHostCommand, PackageHostOutcome, UiMountId, UiRequest,
    UiRequestId,
};

fn declarations(fixture: &common::Fixture) {
    let root = fixture.config.application_root();
    let mut contracts: Vec<_> = (0..30).map(|n| json!({
        "identity":format!("dev.example.contract{n:02}"),"major":1,"minor":2,
        "description":"😀".repeat(512),"ports":[
            {"name":"value.read","kind":"query","description":"Read value","request":{"type":"object","additionalProperties":false},"response":{"$ref":"#/$defs/large"}},
            {"name":"runtime.guide","kind":"query","description":"Guide","request":{"type":"object","additionalProperties":false},"response":{"type":"object","properties":{"markdown":{"type":"string","maxLength":100}},"required":["markdown"],"additionalProperties":false}}
        ]})).collect();
    contracts.push(json!({"identity":"dev.example.secret","major":1,"minor":0,"description":"Not declared by the consumer","ports":[{"name":"value.read","kind":"query","description":"Read","request":true,"response":true}]}));
    let bytes = serde_json::to_vec(&json!({"schemaVersion":1,"$defs":{"large":{"type":"string","description":"x".repeat(70_000)}},"contracts":contracts})).unwrap();
    fs::write(root.join("provider/ports.json"), &bytes).unwrap();
    for directory in ["provider", "consumer", "other"] {
        let path = root.join(directory).join("teamarness.package.json");
        let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        if directory == "provider" {
            manifest["portContract"] = json!({"path":"ports.json","sha256":common::digest(&bytes)});
            manifest["activation"] = json!("on_demand");
        } else {
            manifest["consumedPorts"] = json!((0..34).map(|n| json!({"identity":format!("dev.example.contract{n:02}"),"major":1,"minMinor":0,"maxMinor":2,"required":false,"ports":[{"name":"value.read","kind":"query"}]})).collect::<Vec<_>>());
        }
        fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    }
}

async fn mount(host: &PackageHost, package: &str) -> UiMountId {
    let key = host
        .snapshot()
        .await
        .instances
        .into_iter()
        .find(|i| i.key.package_identity.as_str() == package)
        .unwrap()
        .key;
    match host
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount {
            instance: key,
        }))
        .await
        .unwrap()
    {
        PackageHostOutcome::UiMountOpened { mount, .. } => mount,
        other => panic!("{other:?}"),
    }
}

async fn call(host: &PackageHost, mount: &UiMountId, method: &str, arguments: Value) -> Value {
    static SEQUENCE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let id = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let result = host.execute(PackageHostCommand::UiRequest(UiRequest {
        mount: mount.clone(), request: UiRequestId::new(format!("metadata-{id}")).unwrap(), operation: "fixture.capability".into(),
        payload: serde_json::from_value(json!({"capability":"teamarness.package-ports","method":method,"arguments":arguments})).unwrap(),
    })).await.unwrap();
    match result {
        PackageHostOutcome::UiResponse { payload } => serde_json::to_value(payload).unwrap(),
        other => panic!("{other:?}"),
    }
}

// Verifies: TEAM.SYS.PKG-R12, TEAM.SYS.PKG-R13
#[tokio::test]
async fn declared_metadata_is_complete_bounded_scoped_and_does_not_activate_provider() {
    let fixture = common::Fixture::node_with_capabilities(
        &[
            ("provider", "dev.example.provider"),
            ("consumer", "dev.example.consumer"),
            ("other", "dev.example.other"),
        ],
        &[json!({"identity":"teamarness.package-ports","major":2,"minMinor":0,"maxMinor":0})],
    )
    .await;
    declarations(&fixture);
    fixture.core.create_workspace("Second workspace").unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let mount = mount(&host, "dev.example.consumer").await;
    let first = call(&host, &mount, "available", json!({"access":"declared"})).await;
    assert!(first["result"]["contracts"].is_array(), "{first}");
    let cursor = first["result"]["nextCursor"].as_str().unwrap().to_owned();
    let mut page = first["result"].clone();
    let mut rows = Vec::new();
    loop {
        assert!(serde_json::to_vec(&page).unwrap().len() <= 32 * 1024);
        let entries = page["contracts"].as_array().unwrap();
        assert!(entries.len() <= 32);
        rows.extend(entries.clone());
        let Some(cursor) = page.get("nextCursor") else {
            break;
        };
        page = call(
            &host,
            &mount,
            "available",
            json!({"access":"declared","cursor":cursor}),
        )
        .await["result"]
            .clone();
    }
    assert_eq!(rows.len(), 34);
    assert_eq!(rows[30]["available"], false);
    assert_eq!(rows[30]["metadataSha256"], Value::Null);
    assert_eq!(rows[0]["enabled"], false);
    let contract = rows[0]["contract"].clone();
    let digest = rows[0]["metadataSha256"].clone();
    let mut offset = 0;
    let mut bytes = Vec::new();
    loop {
        let response = call(&host, &mount, "describe", json!({"access":"declared","contract":contract,"metadataSha256":digest,"offset":offset})).await;
        let chunk = &response["result"];
        assert_eq!(chunk["offset"], offset, "{response}");
        assert_eq!(chunk["metadataSha256"], digest);
        assert!(serde_json::to_vec(chunk).unwrap().len() <= 256 * 1024);
        let data = STANDARD.decode(chunk["data"].as_str().unwrap()).unwrap();
        assert!(data.len() <= 64 * 1024);
        bytes.extend(data);
        let Some(next) = chunk.get("nextOffset") else {
            assert_eq!(bytes.len() as u64, chunk["totalBytes"].as_u64().unwrap());
            break;
        };
        offset = next.as_u64().unwrap();
    }
    assert!(bytes.len() > 64 * 1024);
    assert_eq!(common::digest(&bytes), digest);
    let document: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        document["contracts"][0]["ports"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        document["contracts"][0]["ports"][0]["response"],
        json!({"$ref":"#/$defs/large"})
    );
    assert_eq!(
        document["$defs"]["large"]["description"]
            .as_str()
            .unwrap()
            .len(),
        70_000
    );
    for (contract, digest, offset, code) in [
        (
            "dev.example.contract00",
            digest.as_str().unwrap(),
            json!(0.5),
            "capability_request_invalid",
        ),
        (
            "dev.example.contract00",
            digest.as_str().unwrap(),
            json!(-1),
            "capability_request_invalid",
        ),
        (
            "dev.example.contract00",
            digest.as_str().unwrap(),
            json!(bytes.len()),
            "capability_request_invalid",
        ),
        (
            "dev.example.contract00",
            "0000000000000000000000000000000000000000000000000000000000000000",
            json!(0),
            "port_catalog_changed",
        ),
        (
            "dev.example.contract30",
            digest.as_str().unwrap(),
            json!(0),
            "port_unavailable",
        ),
        (
            "dev.example.secret",
            digest.as_str().unwrap(),
            json!(0),
            "ungranted_port",
        ),
    ] {
        let response = call(&host, &mount, "describe", json!({"access":"declared","contract":contract,"metadataSha256":digest,"offset":offset})).await;
        assert_eq!(response["error"], code, "{response}");
    }
    let slice = call(
        &host,
        &mount,
        "describe",
        json!({"access":"declared","contract":contract,"metadataSha256":digest,"offset":1}),
    )
    .await;
    assert_eq!(
        STANDARD
            .decode(slice["result"]["data"].as_str().unwrap())
            .unwrap(),
        bytes[1..1 + 64 * 1024]
    );
    let other = self::mount(&host, "dev.example.other").await;
    for (target, input) in [(&other, cursor.clone()), (&mount, "garbage".into())] {
        let response = call(
            &host,
            target,
            "available",
            json!({"access":"declared","cursor":input}),
        )
        .await;
        assert_eq!(response["error"], "port_catalog_changed", "{response}");
    }
    let keys: Vec<_> = host
        .snapshot()
        .await
        .instances
        .into_iter()
        .filter(|i| i.key.package_identity.as_str() == "dev.example.consumer")
        .map(|i| i.key)
        .collect();
    let PackageHostOutcome::UiMountOpened { mount: second, .. } = host
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount {
            instance: keys[1].clone(),
        }))
        .await
        .unwrap()
    else {
        panic!("mount")
    };
    let cross = call(
        &host,
        &second,
        "available",
        json!({"access":"declared","cursor":cursor}),
    )
    .await;
    assert_eq!(cross["error"], "port_catalog_changed", "{cross}");
    let unbound = call(&host, &mount, "invoke", json!({"access":"agent","contract":contract,"major":1,"port":"value.read","kind":"query","request":{}})).await;
    assert_eq!(unbound["error"], "agent_not_bound");
    let workspace = teamarness_core::WorkspaceId::from_str(keys[0].workspace_id.as_str()).unwrap();
    let agent = fixture.core.add_agent(workspace, "Metadata Agent").unwrap();
    fixture
        .core
        .bind_agent_package(workspace, agent, "dev.example.consumer")
        .unwrap();
    let denied = call(&host, &mount, "invoke", json!({"access":"declared","contract":contract,"major":1,"port":"value.read","kind":"query","request":{}})).await;
    assert_eq!(denied["error"], "port_not_enabled", "{denied}");
    assert!(
        host.snapshot()
            .await
            .instances
            .iter()
            .filter(|i| i.key.package_identity.as_str() == "dev.example.provider")
            .all(|i| i.process_generation.is_none())
    );
    host.execute(PackageHostCommand::Shutdown(
        teamarness_package_host::ShutdownHost,
    ))
    .await
    .unwrap();
    let replacement = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let target = self::mount(&replacement, "dev.example.consumer").await;
    let stale = call(
        &replacement,
        &target,
        "available",
        json!({"access":"declared","cursor":cursor}),
    )
    .await;
    assert_eq!(stale["error"], "port_catalog_changed", "{stale}");
    replacement
        .execute(PackageHostCommand::Shutdown(
            teamarness_package_host::ShutdownHost,
        ))
        .await
        .unwrap();
}

// Verifies: TEAM.SYS.CONV-R20, TEAM.SYS.PKG-R13
#[tokio::test]
async fn conversations_declared_metadata_exposes_guide_without_granting_agent_access() {
    let fixture = common::Fixture::node_with_capabilities(
        &[
            ("provider", "dev.example.provider"),
            ("consumer", "dev.example.consumer"),
        ],
        &[json!({"identity":"teamarness.package-ports","major":2,"minMinor":0,"maxMinor":0})],
    )
    .await;
    let root = fixture.config.application_root();
    let bytes = include_bytes!("../../../packages/conversations-package/ports.json");
    fs::write(root.join("provider/ports.json"), bytes).unwrap();
    for directory in ["provider", "consumer"] {
        let path = root.join(directory).join("teamarness.package.json");
        let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        if directory == "provider" {
            manifest["portContract"] = json!({"path":"ports.json","sha256":common::digest(bytes)});
            manifest["activation"] = json!("on_demand");
        } else {
            manifest["consumedPorts"] = json!([{
                "identity":"dev.teamarness.conversations","major":1,"minMinor":3,"maxMinor":3,
                "ports":[{"name":"runtime.guide","kind":"query"}]
            }]);
        }
        fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    }
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    assert!(
        host.snapshot()
            .await
            .instances
            .iter()
            .any(|i| i.key.package_identity.as_str() == "dev.example.consumer"),
        "Host must admit a consumer declaring Conversations 1.3 runtime.guide"
    );
    let target = mount(&host, "dev.example.consumer").await;
    let available = call(&host, &target, "available", json!({"access":"declared"})).await;
    let summary = &available["result"]["contracts"][0];
    assert_eq!(
        summary["contract"], "dev.teamarness.conversations",
        "{available}"
    );
    assert_eq!(summary["minor"], 3);
    let described = call(
        &host,
        &target,
        "describe",
        json!({
            "access":"declared","contract":"dev.teamarness.conversations",
            "metadataSha256":summary["metadataSha256"],"offset":0
        }),
    )
    .await;
    let projected = STANDARD
        .decode(described["result"]["data"].as_str().unwrap())
        .unwrap();
    let contract =
        teamarness_package_contract::port_contract::compile_port_contracts(&projected).unwrap();
    assert!(contract.has_guide("dev.teamarness.conversations"));
    assert_eq!(contract.contracts[0].ports.len(), 1);
    let key = host
        .snapshot()
        .await
        .instances
        .into_iter()
        .find(|i| i.key.package_identity.as_str() == "dev.example.consumer")
        .unwrap()
        .key;
    let workspace = teamarness_core::WorkspaceId::from_str(key.workspace_id.as_str()).unwrap();
    let agent = fixture.core.add_agent(workspace, "Guide Agent").unwrap();
    fixture
        .core
        .bind_agent_package(workspace, agent, "dev.example.consumer")
        .unwrap();
    for access in ["declared", "agent"] {
        let denied = call(
            &host,
            &target,
            "invoke",
            json!({
                "access":access,"contract":"dev.teamarness.conversations","major":1,
                "port":"runtime.guide","kind":"query","request":{}
            }),
        )
        .await;
        assert_eq!(denied["error"], "port_not_enabled", "{denied}");
    }
    assert!(
        host.snapshot()
            .await
            .instances
            .iter()
            .filter(|i| i.key.package_identity.as_str() == "dev.example.provider")
            .all(|i| i.process_generation.is_none())
    );
    host.execute(PackageHostCommand::Shutdown(
        teamarness_package_host::ShutdownHost,
    ))
    .await
    .unwrap();
}
