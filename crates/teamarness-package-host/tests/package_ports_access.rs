mod common;

use serde_json::{Value, json};
use std::{fs, time::Duration};
use teamarness_package_host::{
    OpenUiMount, PackageHost, PackageHostCommand, PackageHostOutcome, PackageInstanceKey,
    UiMountId, UiRequest, UiRequestId,
};

const CONTRACT: &str = "com.example.messages";

async fn fixture() -> common::Fixture {
    let fixture = common::Fixture::node_with_capabilities(
        &[
            ("provider", "com.example.provider"),
            ("consumer", "com.example.consumer"),
            ("other", "com.example.other"),
        ],
        &[
            json!({"identity":"teamarness.package-ports","major":2,"minMinor":0,"maxMinor":0}),
            json!({"identity":"teamarness.agent-binding","major":1,"minMinor":0,"maxMinor":0}),
        ],
    )
    .await;
    let root = fixture.config.application_root();
    let ports = json!([
        {"name":"runtime.guide","kind":"query","description":"Guide","request":{"type":"object","additionalProperties":false},"response":{"type":"object","properties":{"markdown":{"type":"string","maxLength":100}},"required":["markdown"],"additionalProperties":false}},
        {"name":"message.post","kind":"command","description":"Post","request":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false},"response":{"type":"object","properties":{"accepted":{"type":"boolean"}},"required":["accepted"],"additionalProperties":false}},
        {"name":"message.list","kind":"query","description":"List","request":{"type":"object","additionalProperties":false},"response":{"type":"object","properties":{"accepted":{"type":"boolean"},"actor":{"type":"object"}},"required":["accepted","actor"],"additionalProperties":false}},
        {"name":"message.idle_watch","kind":"query","description":"Idle watch","request":{"type":"object","additionalProperties":false},"event":{"type":"object","properties":{"order":{"type":"integer"}},"required":["order"],"additionalProperties":false}},
        {"name":"message.actor_watch","kind":"query","description":"Watch actor","request":{"type":"object","additionalProperties":false},"event":{"type":"object","properties":{"actor":{"type":"object"}},"required":["actor"],"additionalProperties":false}},
        {"name":"message.created","kind":"event","description":"Created","event":{"type":"object","properties":{"order":{"type":"integer"}},"required":["order"],"additionalProperties":false}}
    ]);
    let bytes = serde_json::to_vec(&json!({"schemaVersion":1,"$defs":{},"contracts":[{"identity":CONTRACT,"major":1,"minor":0,"description":"Messages","ports":ports}, {"identity":"com.example.ordinary","major":1,"minor":0,"description":"No guide","ports":[{"name":"value.read","kind":"query","description":"Read","request":true,"response":true}]}]})).unwrap();
    fs::write(root.join("provider/ports.json"), &bytes).unwrap();
    for directory in ["provider", "consumer"] {
        let path = root.join(directory).join("teamarness.package.json");
        let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        if directory == "provider" {
            manifest["portContract"] = json!({"path":"ports.json","sha256":common::digest(&bytes)});
            manifest["activation"] = json!("on_demand");
        } else {
            manifest["consumedPorts"] = json!([{"identity":CONTRACT,"major":1,"minMinor":0,"maxMinor":0,"required":false,"ports":[{"name":"message.post","kind":"command"},{"name":"message.list","kind":"query"},{"name":"message.actor_watch","kind":"query"},{"name":"message.idle_watch","kind":"query"},{"name":"message.created","kind":"event"}]}]);
        }
        fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    }
    fixture
}

async fn mount(host: &PackageHost, package: &str, workspace: &str) -> UiMountId {
    let key = PackageInstanceKey::new(
        package.parse().unwrap(),
        teamarness_package_host::WorkspaceId::new(workspace).unwrap(),
    );
    let PackageHostOutcome::UiMountOpened { mount, .. } = host
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount {
            instance: key,
        }))
        .await
        .unwrap()
    else {
        panic!("mount")
    };
    mount
}

async fn request(host: &PackageHost, mount: &UiMountId, operation: &str, payload: Value) -> Value {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let PackageHostOutcome::UiResponse { payload } = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: mount.clone(),
            request: UiRequestId::new(format!("access-{id}")).unwrap(),
            operation: operation.into(),
            payload: serde_json::from_value(payload).unwrap(),
        }))
        .await
        .unwrap()
    else {
        panic!("response")
    };
    serde_json::to_value(payload).unwrap()
}

async fn call(host: &PackageHost, mount: &UiMountId, method: &str, arguments: Value) -> Value {
    request(
        host,
        mount,
        "fixture.capability",
        json!({"capability":"teamarness.package-ports","method":method,"arguments":arguments}),
    )
    .await
}
fn invoke(access: &str, port: &str, kind: &str, request: Value) -> Value {
    json!({"access":access,"contract":CONTRACT,"major":1,"port":port,"kind":kind,"request":request})
}
async fn shutdown(host: &PackageHost) {
    host.execute(PackageHostCommand::Shutdown(
        teamarness_package_host::ShutdownHost,
    ))
    .await
    .unwrap();
}

// Verifies: TEAM.SYS.PKG-R13
#[tokio::test]
async fn reference_sibling_guide_is_discovered_without_wake_and_declared_agent_is_default_denied() {
    let fixture = fixture().await;
    let root = fixture.config.application_root();
    let path = root.join("provider/ports.json");
    let mut doc: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    doc["$defs"] = json!({"text":{"type":"string"},"nested":{"$ref":"#/$defs/text","minLength":1}});
    doc["contracts"][0]["ports"][0]["response"]["properties"]["markdown"] =
        json!({"$ref":"#/$defs/nested","maxLength":100});
    let bytes = serde_json::to_vec(&doc).unwrap();
    fs::write(path, &bytes).unwrap();
    let path = root.join("provider/teamarness.package.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["portContract"]["sha256"] = json!(common::digest(&bytes));
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let workspace = fixture.core.list_workspaces().unwrap()[0].id();
    let agent = fixture
        .core
        .add_agent(workspace, "Sibling Guide Agent")
        .unwrap();
    fixture
        .core
        .bind_agent_package(workspace, agent, "com.example.consumer")
        .unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let consumer = mount(&host, "com.example.consumer", &workspace.to_string()).await;
    // Assert the authority boundary before discovery: the old classifier dispatched this command.
    for access in ["declared", "agent"] {
        let denied = call(
            &host,
            &consumer,
            "invoke",
            invoke(access, "message.post", "command", json!({"text":"denied"})),
        )
        .await;
        assert_eq!(denied["error"], "port_not_enabled", "{access}: {denied}");
    }
    let available = call(&host, &consumer, "available", json!({"access":"agent"})).await;
    let rows = available["result"]["contracts"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{available}");
    assert_eq!(rows[0]["contract"], CONTRACT);
    assert_eq!(rows[0]["enabled"], false);
    let metadata = call(&host, &consumer, "describe", json!({"access":"agent","contract":CONTRACT,"metadataSha256":rows[0]["metadataSha256"],"offset":0})).await;
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(metadata["result"]["data"].as_str().unwrap())
        .unwrap();
    let described: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(described["$defs"], doc["$defs"]);
    let guide = described["contracts"][0]["ports"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "runtime.guide")
        .unwrap();
    assert_eq!(
        guide["response"],
        doc["contracts"][0]["ports"][0]["response"]
    );
    assert_eq!(
        call(
            &host,
            &consumer,
            "enable",
            json!({"contract":CONTRACT,"major":1})
        )
        .await["result"]["enabled"],
        true
    );
    assert!(
        host.snapshot()
            .await
            .instances
            .iter()
            .filter(|i| i.key.package_identity.as_str() == "com.example.provider")
            .all(|i| i.process_generation.is_none()),
        "neither refused dispatch, discovery nor enable may start the Provider"
    );
    for access in ["declared", "agent"] {
        let posted = call(
            &host,
            &consumer,
            "invoke",
            invoke(access, "message.post", "command", json!({"text":"enabled"})),
        )
        .await;
        assert_eq!(posted["result"]["accepted"], true, "{posted}");
    }
    shutdown(&host).await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let consumer = mount(&host, "com.example.consumer", &workspace.to_string()).await;
    assert_eq!(
        call(&host, &consumer, "available", json!({"access":"agent"})).await["result"]["contracts"]
            [0]["enabledMajor"],
        1
    );
    assert_eq!(
        call(
            &host,
            &consumer,
            "invoke",
            invoke(
                "declared",
                "message.post",
                "command",
                json!({"text":"retained"})
            )
        )
        .await["result"]["accepted"],
        true
    );
    shutdown(&host).await;
}

// Verifies: TEAM.SYS.PKG-R13
#[tokio::test]
async fn conflicting_reference_sibling_guide_is_not_advertised_and_keeps_ordinary_composition() {
    let fixture = fixture().await;
    let root = fixture.config.application_root();
    let path = root.join("provider/ports.json");
    let mut doc: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    doc["$defs"] = json!({"text":{"type":"string","maxLength":100}});
    doc["contracts"][0]["ports"][0]["response"]["properties"]["markdown"] =
        json!({"$ref":"#/$defs/text","type":"integer"});
    let bytes = serde_json::to_vec(&doc).unwrap();
    fs::write(path, &bytes).unwrap();
    let path = root.join("provider/teamarness.package.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["portContract"]["sha256"] = json!(common::digest(&bytes));
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let workspace = fixture.core.list_workspaces().unwrap()[0].id();
    let agent = fixture.core.add_agent(workspace, "No Guide Agent").unwrap();
    fixture
        .core
        .bind_agent_package(workspace, agent, "com.example.consumer")
        .unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let consumer = mount(&host, "com.example.consumer", &workspace.to_string()).await;
    let available = call(&host, &consumer, "available", json!({"access":"agent"})).await;
    assert_eq!(available["result"]["contracts"], json!([]), "{available}");
    assert!(
        call(
            &host,
            &consumer,
            "enable",
            json!({"contract":CONTRACT,"major":1})
        )
        .await
        .get("error")
        .is_some()
    );
    assert!(
        host.snapshot()
            .await
            .instances
            .iter()
            .filter(|i| i.key.package_identity.as_str() == "com.example.provider")
            .all(|i| i.process_generation.is_none())
    );
    let posted = call(
        &host,
        &consumer,
        "invoke",
        invoke(
            "declared",
            "message.post",
            "command",
            json!({"text":"ordinary"}),
        ),
    )
    .await;
    assert_eq!(posted["result"]["accepted"], true, "{posted}");
    shutdown(&host).await;
}

// Verifies: TEAM.SYS.PKG-R13
#[tokio::test]
async fn bound_harness_discovers_and_enables_only_its_own_contracts_without_starting_provider() {
    let fixture = fixture().await;
    let workspace = fixture.core.list_workspaces().unwrap()[0].id();
    let agent = fixture.core.add_agent(workspace, "Agent").unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let consumer = mount(&host, "com.example.consumer", &workspace.to_string()).await;
    assert_eq!(
        call(&host, &consumer, "available", json!({"access":"agent"})).await["error"],
        "agent_not_bound"
    );
    for package in ["com.example.consumer", "com.example.other"] {
        fixture
            .core
            .bind_agent_package(workspace, agent, package)
            .unwrap();
    }
    let available = call(&host, &consumer, "available", json!({"access":"agent"})).await;
    assert_eq!(
        available["result"]["contracts"].as_array().unwrap().len(),
        1,
        "{available}"
    );
    let row = &available["result"]["contracts"][0];
    assert_eq!(row["contract"], CONTRACT);
    assert_eq!(row["enabled"], false);
    let metadata = call(&host, &consumer, "describe", json!({"access":"agent","contract":CONTRACT,"metadataSha256":row["metadataSha256"],"offset":0})).await;
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(metadata["result"]["data"].as_str().unwrap())
        .unwrap();
    let document: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        document["contracts"][0]["ports"].as_array().unwrap().len(),
        6
    );
    for access in ["agent", "declared"] {
        assert_eq!(
            call(
                &host,
                &consumer,
                "invoke",
                invoke(access, "message.post", "command", json!({"text":"denied"}))
            )
            .await["error"],
            "port_not_enabled"
        );
    }
    for _ in 0..2 {
        let enabled = call(
            &host,
            &consumer,
            "enable",
            json!({"contract":CONTRACT,"major":1}),
        )
        .await;
        assert_eq!(
            enabled["result"],
            json!({"contract":CONTRACT,"major":1,"enabled":true}),
            "{enabled}"
        );
    }
    assert!(
        host.snapshot()
            .await
            .instances
            .iter()
            .filter(|i| i.key.package_identity.as_str() == "com.example.provider")
            .all(|i| i.process_generation.is_none())
    );
    for access in ["declared", "agent"] {
        let posted = call(
            &host,
            &consumer,
            "invoke",
            invoke(access, "message.post", "command", json!({"text":"hello"})),
        )
        .await;
        assert_eq!(posted["result"]["accepted"], true, "{posted}");
        let listed = call(
            &host,
            &consumer,
            "invoke",
            invoke(access, "message.list", "query", json!({})),
        )
        .await;
        assert_eq!(
            listed["result"]["actor"]["principal"],
            agent.to_string(),
            "{listed}"
        );
    }
    let other = mount(&host, "com.example.other", &workspace.to_string()).await;
    assert_eq!(
        call(
            &host,
            &other,
            "invoke",
            invoke("agent", "message.list", "query", json!({}))
        )
        .await["error"],
        "port_not_enabled"
    );
    assert_eq!(
        call(
            &host,
            &other,
            "enable",
            json!({"contract":CONTRACT,"major":1})
        )
        .await["result"]["enabled"],
        true
    );
    assert_eq!(
        call(
            &host,
            &other,
            "invoke",
            invoke("agent", "message.list", "query", json!({}))
        )
        .await["result"]["actor"]["principal"],
        agent.to_string()
    );
    shutdown(&host).await;
}

// Verifies: TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test]
async fn enabled_undeclared_streams_use_shared_dispatch_and_reject_invalid_requests_before_receipt()
{
    let fixture = fixture().await;
    let workspace = fixture.core.list_workspaces().unwrap()[0].id();
    let agent = fixture.core.add_agent(workspace, "Stream Agent").unwrap();
    fixture
        .core
        .bind_agent_package(workspace, agent, "com.example.other")
        .unwrap();
    fixture
        .core
        .bind_agent_package(workspace, agent, "com.example.consumer")
        .unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let other = mount(&host, "com.example.other", &workspace.to_string()).await;
    assert_eq!(
        call(
            &host,
            &other,
            "enable",
            json!({"contract":CONTRACT,"major":1})
        )
        .await["result"]["enabled"],
        true
    );
    for (input, code) in [
        (
            invoke("agent", "message.actor_watch", "query", json!({})),
            "ungranted_port",
        ),
        (
            invoke("agent", "wrong.name", "query", json!({})),
            "ungranted_port",
        ),
        (
            invoke("agent", "message.list", "command", json!({})),
            "ungranted_port",
        ),
        (
            json!({"access":"agent","contract":CONTRACT,"major":2,"port":"message.list","kind":"query","request":{}}),
            "port_not_enabled",
        ),
        (
            invoke("agent", "message.post", "command", json!({"text":1})),
            "port_request_invalid",
        ),
        (
            json!({"access":"agent","contract":CONTRACT,"major":1,"port":"message.post","kind":"command","request":{"text":"hello"},"workspace":"elsewhere"}),
            "capability_request_invalid",
        ),
    ] {
        let refused = call(&host, &other, "invoke", input).await;
        assert_eq!(refused["error"], code, "{refused}");
    }
    assert!(
        host.snapshot()
            .await
            .instances
            .iter()
            .filter(|i| i.key.package_identity.as_str() == "com.example.provider")
            .all(|i| i.process_generation.is_none())
    );
    let declared = mount(&host, "com.example.consumer", &workspace.to_string()).await;
    assert_eq!(
        call(
            &host,
            &declared,
            "enable",
            json!({"contract":CONTRACT,"major":1})
        )
        .await["result"]["enabled"],
        true
    );
    for (access, target) in [("agent", other.clone()), ("declared", declared)] {
        request(
            &host,
            &target,
            "fixture.port_open",
            json!({"access":access,"port":"message.actor_watch"}),
        )
        .await;
        common::eventually(
            Duration::from_secs(5),
            "Agent live query completes",
            || async {
                request(&host, &target, "fixture.port_events", json!({})).await["closeCode"]
                    == "completed"
            },
        )
        .await;
        assert_eq!(
            request(&host, &target, "fixture.port_events", json!({})).await["events"][0]["actor"]["principal"],
            agent.to_string()
        );
        request(
            &host,
            &target,
            "fixture.port_event_open",
            json!({"access":access}),
        )
        .await;
        let provider = mount(&host, "com.example.provider", &workspace.to_string()).await;
        common::eventually(
            Duration::from_secs(5),
            "Agent event upstream opens",
            || async {
                request(&host, &provider, "fixture.port_event_stats", json!({})).await["active"]
                    == 1
            },
        )
        .await;
        assert_eq!(
            request(
                &host,
                &provider,
                "fixture.port_event_emit",
                json!({"order":42})
            )
            .await["emitted"],
            1
        );
        common::eventually(Duration::from_secs(5), "Agent event delivered", || async {
        request(&host, &target, "fixture.port_event_events", json!({})).await["events"][0]["order"]
            == 42
    })
    .await;
        request(&host, &target, "fixture.port_event_close", json!({})).await;
    }
    shutdown(&host).await;
}

// Verifies: TEAM.SYS.PKG-R13
#[tokio::test]
async fn settings_refuse_scope_selectors_and_enabled_major_does_not_rewrite_a_declaration() {
    let fixture = fixture().await;
    let workspace = fixture.core.list_workspaces().unwrap()[0].id();
    let agent = fixture.core.add_agent(workspace, "Settings Agent").unwrap();
    fixture
        .core
        .bind_agent_package(workspace, agent, "com.example.consumer")
        .unwrap();
    let root = fixture.config.application_root();
    let path = root.join("consumer/teamarness.package.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    // The real declared dependency remains major 2, absent and optional under Provider major 1.
    manifest["consumedPorts"][0]["major"] = json!(2);
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let target = mount(&host, "com.example.consumer", &workspace.to_string()).await;
    for selector in ["package", "workspace", "provider", "actor", "caller"] {
        let mut input = json!({"contract":CONTRACT,"major":1});
        input[selector] = json!("elsewhere");
        assert_eq!(
            call(&host, &target, "enable", input).await["error"],
            "capability_request_invalid"
        );
        let mut input = json!({"contract":CONTRACT});
        input[selector] = json!("elsewhere");
        assert_eq!(
            call(&host, &target, "disable", input).await["error"],
            "capability_request_invalid"
        );
    }
    for (input, code) in [
        (
            json!({"contract":"com.example.ordinary","major":1}),
            "ungranted_port",
        ),
        (
            json!({"contract":"com.example.missing","major":1}),
            "port_unavailable",
        ),
        (
            json!({"contract":CONTRACT,"major":2}),
            "port_version_mismatch",
        ),
    ] {
        assert_eq!(call(&host, &target, "enable", input).await["error"], code);
    }
    assert_eq!(
        call(
            &host,
            &target,
            "enable",
            json!({"contract":CONTRACT,"major":1})
        )
        .await["result"]["enabled"],
        true
    );
    let mut declared = invoke("declared", "message.list", "query", json!({}));
    declared["major"] = json!(2);
    assert_eq!(
        call(&host, &target, "invoke", declared).await["error"],
        "port_unavailable"
    );
    assert_eq!(
        call(
            &host,
            &target,
            "invoke",
            invoke("agent", "message.list", "query", json!({}))
        )
        .await["result"]["actor"]["principal"],
        agent.to_string()
    );
    shutdown(&host).await;
}

// Verifies: TEAM.SYS.PKG-R13
#[tokio::test]
async fn selections_are_durable_workspace_scoped_and_pin_majors_across_reopen_and_reinstall() {
    let fixture = fixture().await;
    let first = fixture.core.list_workspaces().unwrap()[0].id();
    let second = fixture.core.create_workspace("Second").unwrap().id();
    let a = fixture.core.add_agent(first, "First Agent").unwrap();
    let b = fixture.core.add_agent(first, "Replacement Agent").unwrap();
    let c = fixture
        .core
        .add_agent(second, "Other workspace Agent")
        .unwrap();
    fixture
        .core
        .bind_agent_package(first, a, "com.example.other")
        .unwrap();
    fixture
        .core
        .bind_agent_package(second, c, "com.example.other")
        .unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let target = mount(&host, "com.example.other", &first.to_string()).await;
    let independent = mount(&host, "com.example.other", &second.to_string()).await;
    assert_eq!(
        call(
            &host,
            &target,
            "enable",
            json!({"contract":CONTRACT,"major":1})
        )
        .await["result"]["enabled"],
        true
    );
    assert_eq!(
        call(
            &host,
            &independent,
            "invoke",
            invoke("agent", "message.list", "query", json!({}))
        )
        .await["error"],
        "port_not_enabled"
    );
    fixture
        .core
        .bind_agent_package(first, b, "com.example.other")
        .unwrap();
    assert_eq!(
        call(
            &host,
            &target,
            "invoke",
            invoke("agent", "message.list", "query", json!({}))
        )
        .await["result"]["actor"]["principal"],
        b.to_string()
    );
    fixture
        .core
        .unbind_agent_package(first, "com.example.other")
        .unwrap();
    for method in ["available", "enable", "disable", "invoke"] {
        let arguments = match method {
            "available" => json!({"access":"agent"}),
            "enable" => json!({"contract":CONTRACT,"major":1}),
            "disable" => json!({"contract":CONTRACT}),
            _ => invoke("agent", "message.list", "query", json!({})),
        };
        assert_eq!(
            call(&host, &target, method, arguments).await["error"],
            "agent_not_bound",
            "{method}"
        );
    }
    fixture
        .core
        .bind_agent_package(first, a, "com.example.other")
        .unwrap();
    shutdown(&host).await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let target = mount(&host, "com.example.other", &first.to_string()).await;
    let available = call(&host, &target, "available", json!({"access":"agent"})).await;
    let old_digest = available["result"]["contracts"][0]["metadataSha256"].clone();
    assert_eq!(available["result"]["contracts"][0]["enabledMajor"], 1);
    assert_eq!(
        call(
            &host,
            &target,
            "invoke",
            invoke("agent", "message.list", "query", json!({}))
        )
        .await["result"]["actor"]["principal"],
        a.to_string()
    );
    shutdown(&host).await;
    let root = fixture.config.application_root();
    let index = root.join("application-packages.v1.json");
    let original_index = fs::read(&index).unwrap();
    fs::write(
        &index,
        serde_json::to_vec(
            &json!({"schemaVersion":1,"packages":[{"directory":"consumer"},{"directory":"other"}]}),
        )
        .unwrap(),
    )
    .unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let target = mount(&host, "com.example.other", &first.to_string()).await;
    let missing = call(&host, &target, "available", json!({"access":"agent"})).await;
    assert_eq!(
        missing["result"]["contracts"][0],
        json!({"contract":CONTRACT,"major":1,"minor":null,"description":null,"available":false,"enabled":true,"enabledMajor":1,"metadataSha256":null}),
        "{missing}"
    );
    assert_eq!(
        call(
            &host,
            &target,
            "describe",
            json!({"access":"agent","contract":CONTRACT,"metadataSha256":old_digest,"offset":0})
        )
        .await["error"],
        "port_unavailable"
    );
    for _ in 0..2 {
        assert_eq!(
            call(&host, &target, "disable", json!({"contract":CONTRACT})).await["result"]["enabled"],
            false
        );
    }
    assert_eq!(
        call(&host, &target, "available", json!({"access":"agent"})).await["result"]["contracts"],
        json!([])
    );
    shutdown(&host).await;
    fs::write(&index, &original_index).unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let target = mount(&host, "com.example.other", &first.to_string()).await;
    assert_eq!(
        call(
            &host,
            &target,
            "enable",
            json!({"contract":CONTRACT,"major":1})
        )
        .await["result"]["enabled"],
        true
    );
    shutdown(&host).await;
    // Reinstall both identities: first reopen without the harness, then replace the Provider.
    fs::write(&index, serde_json::to_vec(&json!({"schemaVersion":1,"packages":[{"directory":"provider"},{"directory":"consumer"}]})).unwrap()).unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    shutdown(&host).await;
    fs::write(&index, &original_index).unwrap();
    let document_path = root.join("provider/ports.json");
    let mut document: Value = serde_json::from_slice(&fs::read(&document_path).unwrap()).unwrap();
    let manifest_path = root.join("provider/teamarness.package.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["identity"] = json!("com.example.replacement");
    for major in [2, 1] {
        document["contracts"][0]["major"] = json!(major);
        document["contracts"][0]["description"] = json!("Replacement metadata");
        let bytes = serde_json::to_vec(&document).unwrap();
        fs::write(&document_path, &bytes).unwrap();
        manifest["portContract"]["sha256"] = json!(common::digest(&bytes));
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
            .await
            .unwrap();
        let target = mount(&host, "com.example.other", &first.to_string()).await;
        let page = call(&host, &target, "available", json!({"access":"agent"})).await;
        let row = &page["result"]["contracts"][0];
        assert_eq!(row["enabledMajor"], 1, "{page}");
        assert_eq!(row["major"], major);
        assert_eq!(row["available"], major == 1);
        let described = call(
            &host,
            &target,
            "describe",
            json!({"access":"agent","contract":CONTRACT,"metadataSha256":old_digest,"offset":0}),
        )
        .await;
        if major == 2 {
            assert_eq!(row["enabled"], true);
            assert!(row["metadataSha256"].is_string());
            assert_eq!(described["error"], "port_catalog_changed");
            let current = call(&host, &target, "describe", json!({"access":"agent","contract":CONTRACT,"metadataSha256":row["metadataSha256"],"offset":0})).await;
            use base64::Engine as _;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(current["result"]["data"].as_str().unwrap())
                .unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes).unwrap()["contracts"][0]["major"],
                2
            );
            assert!(
                host.snapshot()
                    .await
                    .instances
                    .iter()
                    .filter(|i| i.key.package_identity.as_str() == "com.example.replacement")
                    .all(|i| i.process_generation.is_none())
            );
            assert_eq!(
                call(&host, &target, "available", json!({"access":"agent"})).await["result"]["contracts"]
                    [0]["enabledMajor"],
                1
            );
            assert_eq!(
                call(
                    &host,
                    &target,
                    "invoke",
                    invoke("agent", "message.list", "query", json!({}))
                )
                .await["error"],
                "port_unavailable"
            );
        } else {
            assert_ne!(row["metadataSha256"], old_digest);
            assert_eq!(described["error"], "port_catalog_changed");
            assert_eq!(
                call(
                    &host,
                    &target,
                    "invoke",
                    invoke("agent", "message.list", "query", json!({}))
                )
                .await["result"]["actor"]["principal"],
                a.to_string()
            );
        }
        shutdown(&host).await;
    }
    document["contracts"][0]["major"] = json!(2);
    let bytes = serde_json::to_vec(&document).unwrap();
    fs::write(&document_path, &bytes).unwrap();
    manifest["portContract"]["sha256"] = json!(common::digest(&bytes));
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let target = mount(&host, "com.example.other", &first.to_string()).await;
    assert_eq!(
        call(&host, &target, "available", json!({"access":"agent"})).await["result"]["contracts"]
            [0]["enabledMajor"],
        1
    );
    assert_eq!(
        call(
            &host,
            &target,
            "enable",
            json!({"contract":CONTRACT,"major":2})
        )
        .await["result"]["major"],
        2
    );
    let page = call(&host, &target, "available", json!({"access":"agent"})).await;
    assert_eq!(page["result"]["contracts"][0]["enabledMajor"], 2);
    assert_eq!(page["result"]["contracts"][0]["available"], true);
    let mut upgraded = invoke("agent", "message.list", "query", json!({}));
    upgraded["major"] = json!(2);
    assert_eq!(
        call(&host, &target, "invoke", upgraded).await["result"]["actor"]["principal"],
        a.to_string()
    );
    shutdown(&host).await;
}

// Verifies: TEAM.SYS.PKG-R13
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_storage_never_claims_success_and_lost_completion_is_reread() {
    let fixture = fixture().await;
    let workspace = fixture.core.list_workspaces().unwrap()[0].id();
    let agent = fixture.core.add_agent(workspace, "Storage Agent").unwrap();
    fixture
        .core
        .bind_agent_package(workspace, agent, "com.example.other")
        .unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let target = mount(&host, "com.example.other", &workspace.to_string()).await;
    let database = fixture._private.join("packages/package-host.sqlite3");
    let connection = rusqlite::Connection::open(database).unwrap();
    connection.execute_batch("CREATE TRIGGER refuse_selection BEFORE INSERT ON agent_port_selections BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;").unwrap();
    let failed = call(
        &host,
        &target,
        "enable",
        json!({"contract":CONTRACT,"major":1}),
    )
    .await;
    assert_eq!(failed["error"], "capability_unavailable", "{failed}");
    assert_eq!(
        call(
            &host,
            &target,
            "invoke",
            invoke("agent", "message.list", "query", json!({}))
        )
        .await["error"],
        "port_not_enabled"
    );
    connection
        .execute_batch("DROP TRIGGER refuse_selection;")
        .unwrap();
    #[cfg(feature = "test-hooks")]
    {
        let pause = host.pause_next_port_selection_for_tests("com.example.other".parse().unwrap());
        let _release = pause.resume_on_drop();
        let running_host = host.clone();
        let running_target = target.clone();
        let running = tokio::spawn(async move {
            call(
                &running_host,
                &running_target,
                "enable",
                json!({"contract":CONTRACT,"major":1}),
            )
            .await
        });
        pause.paused().await;
        pause.fail_completion();
        assert_eq!(running.await.unwrap()["error"], "capability_unavailable");
        assert_eq!(
            call(&host, &target, "available", json!({"access":"agent"})).await["result"]["contracts"]
                [0]["enabledMajor"],
            1
        );
        assert_eq!(
            call(
                &host,
                &target,
                "invoke",
                invoke("agent", "message.list", "query", json!({}))
            )
            .await["result"]["actor"]["principal"],
            agent.to_string()
        );
    }
    shutdown(&host).await;
}

#[cfg(feature = "test-hooks")]
// Verifies: TEAM.SYS.PKG-R13
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_storage_waiter_retains_gate_and_generation_until_actual_work_finishes() {
    let fixture = fixture().await;
    let workspace = fixture.core.list_workspaces().unwrap()[0].id();
    let agent = fixture
        .core
        .add_agent(workspace, "Cancellation Agent")
        .unwrap();
    fixture
        .core
        .bind_agent_package(workspace, agent, "com.example.other")
        .unwrap();
    let host = PackageHost::open(
        fixture
            .config
            .clone()
            .with_capability_call_timeout_for_tests(Duration::from_millis(200)),
        fixture.core.clone(),
    )
    .await
    .unwrap();
    let target = mount(&host, "com.example.other", &workspace.to_string()).await;
    let key = PackageInstanceKey::new(
        "com.example.other".parse().unwrap(),
        teamarness_package_host::WorkspaceId::new(workspace.to_string()).unwrap(),
    );
    let old_grant = request(
        &host,
        &target,
        "fixture.grant",
        json!({"capability":"teamarness.package-ports"}),
    )
    .await["grant"]
        .clone();
    let pause = host.pause_next_port_selection_for_tests(key.package_identity.clone());
    let _release = pause.resume_on_drop();
    let running_host = host.clone();
    let running_target = target.clone();
    let running = tokio::spawn(async move {
        call(
            &running_host,
            &running_target,
            "enable",
            json!({"contract":CONTRACT,"major":1}),
        )
        .await
    });
    pause.paused().await;
    assert_eq!(running.await.unwrap()["error"], "deadline_exceeded");
    let disable_host = host.clone();
    let disable_target = target.clone();
    let mut disable = tokio::spawn(async move {
        call(
            &disable_host,
            &disable_target,
            "disable",
            json!({"contract":CONTRACT}),
        )
        .await
    });
    let overtook = tokio::time::timeout(Duration::from_millis(100), &mut disable)
        .await
        .is_ok();
    pause.resume();
    assert!(
        !overtook,
        "disable overtook still-running cancelled storage work"
    );
    assert_eq!(disable.await.unwrap()["result"]["enabled"], false);
    assert_eq!(
        call(
            &host,
            &target,
            "invoke",
            invoke("agent", "message.list", "query", json!({}))
        )
        .await["error"],
        "port_not_enabled"
    );
    // A second timed-out write fences replacement of the crashed harness generation.
    let pause = host.pause_next_port_selection_for_tests(key.package_identity.clone());
    let _release = pause.resume_on_drop();
    let running_host = host.clone();
    let running_target = target.clone();
    let running = tokio::spawn(async move {
        call(
            &running_host,
            &running_target,
            "enable",
            json!({"contract":CONTRACT,"major":1}),
        )
        .await
    });
    pause.paused().await;
    assert_eq!(running.await.unwrap()["error"], "deadline_exceeded");
    let replacement_host = host.clone();
    let crash_target = target.clone();
    let mut replacement = tokio::spawn(async move {
        let _ = replacement_host
            .execute(PackageHostCommand::UiRequest(UiRequest {
                mount: crash_target,
                request: UiRequestId::new("crash-storage-generation").unwrap(),
                operation: "fixture.abort".into(),
                payload: serde_json::from_value(json!({})).unwrap(),
            }))
            .await;
        common::eventually(
            Duration::from_secs(8),
            "replacement harness generation",
            || async {
                replacement_host.snapshot().await.instances.iter().any(|i| {
                    i.key == key
                        && i.lifecycle == teamarness_package_host::LifecycleState::Active
                        && i.process_generation.is_some_and(|g| g >= 2)
                })
            },
        )
        .await;
        mount(
            &replacement_host,
            "com.example.other",
            &workspace.to_string(),
        )
        .await
    });
    let overtook = tokio::time::timeout(Duration::from_millis(100), &mut replacement)
        .await
        .is_ok();
    pause.resume();
    assert!(
        !overtook,
        "replacement overtook still-running storage write"
    );
    let target = replacement.await.unwrap();
    assert_eq!(
        call(&host, &target, "disable", json!({"contract":CONTRACT})).await["result"]["enabled"],
        false
    );
    assert_eq!(
        call(
            &host,
            &target,
            "invoke",
            invoke("agent", "message.list", "query", json!({}))
        )
        .await["error"],
        "port_not_enabled"
    );
    let stale = request(
        &host,
        &target,
        "fixture.capability",
        json!({"grant":old_grant,"method":"enable","arguments":{"contract":CONTRACT,"major":1}}),
    )
    .await;
    assert_eq!(stale["error"], "capability_request_invalid");
    assert_eq!(
        call(&host, &target, "available", json!({"access":"agent"})).await["result"]["contracts"]
            [0]["enabled"],
        false
    );
    shutdown(&host).await;
}

async fn wait_terminal(host: &PackageHost, target: &UiMountId, operation: &str, code: &str) {
    common::eventually(Duration::from_secs(5), code, || async {
        request(host, target, operation, json!({})).await["closeCode"] == code
    })
    .await;
}

async fn open_idle_streams(
    host: &PackageHost,
    target: &UiMountId,
    provider: &UiMountId,
    access: &str,
) {
    let mut observations = host.observe_ports();
    request(
        host,
        target,
        "fixture.port_open",
        json!({"access":access,"port":"message.idle_watch"}),
    )
    .await;
    common::eventually(
        Duration::from_secs(5),
        "idle live query received",
        || async {
            request(host, provider, "fixture.port_query_stats", json!({})).await["active"]
                .as_u64()
                .unwrap()
                > 0
        },
    )
    .await;
    request(
        host,
        target,
        "fixture.port_event_open",
        json!({"access":access}),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if observations.recv().await.unwrap().stage
                == teamarness_package_host::PortStage::Subscribed
            {
                break;
            }
        }
    })
    .await
    .unwrap();
}

// Verifies: TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test]
async fn disable_revokes_idle_agent_and_declared_streams_without_stopping_shared_peer() {
    for access in ["agent", "declared"] {
        let fixture = fixture().await;
        let workspace = fixture.core.list_workspaces().unwrap()[0].id();
        let agent = fixture.core.add_agent(workspace, "Shared Agent").unwrap();
        for package in ["com.example.consumer", "com.example.other"] {
            fixture
                .core
                .bind_agent_package(workspace, agent, package)
                .unwrap();
        }
        let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
            .await
            .unwrap();
        let a = mount(&host, "com.example.consumer", &workspace.to_string()).await;
        let b = mount(&host, "com.example.other", &workspace.to_string()).await;
        let provider = mount(&host, "com.example.provider", &workspace.to_string()).await;
        for target in [&a, &b] {
            assert_eq!(
                call(
                    &host,
                    target,
                    "enable",
                    json!({"contract":CONTRACT,"major":1})
                )
                .await["result"]["enabled"],
                true
            );
        }
        open_idle_streams(&host, &a, &provider, access).await;
        open_idle_streams(&host, &b, &provider, "agent").await;
        assert_eq!(
            request(&host, &provider, "fixture.port_event_stats", json!({})).await["opens"],
            1
        );
        request(
            &host,
            &provider,
            "fixture.port_event_emit",
            json!({"order":1}),
        )
        .await;
        for target in [&a, &b] {
            common::eventually(Duration::from_secs(5),"first event",|| async {
                request(&host,target,"fixture.port_event_events",json!({})).await["events"][0]["order"] == 1
            }).await;
        }
        assert_eq!(
            call(&host, &a, "disable", json!({"contract":CONTRACT})).await["result"]["enabled"],
            false
        );
        wait_terminal(&host, &a, "fixture.port_events", "port_not_enabled").await;
        wait_terminal(&host, &a, "fixture.port_event_events", "port_not_enabled").await;
        assert_eq!(
            call(
                &host,
                &a,
                "invoke",
                invoke(access, "message.post", "command", json!({"text":"denied"}))
            )
            .await["error"],
            "port_not_enabled"
        );
        request(
            &host,
            &provider,
            "fixture.port_event_emit",
            json!({"order":2}),
        )
        .await;
        common::eventually(
            Duration::from_secs(5),
            "unrelated peer continues",
            || async {
                request(&host, &b, "fixture.port_event_events", json!({})).await["events"]
                    == json!([{"order":1},{"order":2}])
            },
        )
        .await;
        assert_eq!(
            request(&host, &a, "fixture.port_event_events", json!({})).await["events"],
            json!([{"order":1}])
        );
        assert_eq!(
            request(&host, &provider, "fixture.port_event_stats", json!({})).await["active"],
            1
        );
        shutdown(&host).await;
    }
}

async fn binding(host: &PackageHost, target: &UiMountId, method: &str, arguments: Value) -> Value {
    request(
        host,
        target,
        "fixture.capability",
        json!({"capability":"teamarness.agent-binding","method":method,"arguments":arguments}),
    )
    .await
}

// Verifies: TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test]
async fn binding_transitions_and_idle_membership_removal_revoke_both_access_views() {
    for access in ["agent", "declared"] {
        let fixture = fixture().await;
        let workspace = fixture.core.list_workspaces().unwrap()[0].id();
        let a = fixture.core.add_agent(workspace, "First").unwrap();
        let b = fixture.core.add_agent(workspace, "Second").unwrap();
        fixture
            .core
            .bind_agent_package(workspace, a, "com.example.consumer")
            .unwrap();
        let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
            .await
            .unwrap();
        let target = mount(&host, "com.example.consumer", &workspace.to_string()).await;
        let provider = mount(&host, "com.example.provider", &workspace.to_string()).await;
        call(
            &host,
            &target,
            "enable",
            json!({"contract":CONTRACT,"major":1}),
        )
        .await;
        open_idle_streams(&host, &target, &provider, access).await;
        assert!(
            binding(&host, &target, "bind", json!({"principalId":a.to_string()}))
                .await
                .get("result")
                .is_some()
        );
        request(
            &host,
            &provider,
            "fixture.port_query_emit",
            json!({"order":1}),
        )
        .await;
        common::eventually(Duration::from_secs(5), "emitting live query", || async {
            request(&host, &target, "fixture.port_events", json!({})).await["events"]
                == json!([{"order":1}])
        })
        .await;
        // A subsequent Provider event proves an idempotent bind kept the existing stream.
        request(
            &host,
            &provider,
            "fixture.port_event_emit",
            json!({"order":1}),
        )
        .await;
        common::eventually(Duration::from_secs(5),"same Agent bind keeps stream",|| async {
            request(&host,&target,"fixture.port_event_events",json!({})).await["events"][0]["order"] == 1
        }).await;
        assert!(
            binding(&host, &target, "bind", json!({"principalId":b.to_string()}))
                .await
                .get("result")
                .is_some()
        );
        wait_terminal(
            &host,
            &target,
            "fixture.port_events",
            "agent_binding_changed",
        )
        .await;
        wait_terminal(
            &host,
            &target,
            "fixture.port_event_events",
            "agent_binding_changed",
        )
        .await;
        assert_eq!(
            call(
                &host,
                &target,
                "invoke",
                invoke(access, "message.list", "query", json!({}))
            )
            .await["result"]["actor"]["principal"],
            b.to_string()
        );
        common::eventually(Duration::from_secs(5), "old query closed", || async {
            request(&host, &provider, "fixture.port_query_stats", json!({})).await["active"] == 0
        })
        .await;
        open_idle_streams(&host, &target, &provider, access).await;
        binding(&host, &target, "unbind", json!({})).await;
        // Immediately bind the same Agent again. The old lease must still end.
        binding(&host, &target, "bind", json!({"principalId":b.to_string()})).await;
        wait_terminal(&host, &target, "fixture.port_events", "agent_not_bound").await;
        wait_terminal(
            &host,
            &target,
            "fixture.port_event_events",
            "agent_not_bound",
        )
        .await;
        common::eventually(
            Duration::from_secs(5),
            "old query closed after unbind",
            || async {
                request(&host, &provider, "fixture.port_query_stats", json!({})).await["active"]
                    == 0
            },
        )
        .await;
        open_idle_streams(&host, &target, &provider, access).await;
        fixture.core.remove_agent(workspace, b).unwrap();
        // No Provider payload or subsequent authorization drives reconciliation.
        wait_terminal(&host, &target, "fixture.port_events", "agent_not_bound").await;
        wait_terminal(
            &host,
            &target,
            "fixture.port_event_events",
            "agent_not_bound",
        )
        .await;
        assert_eq!(
            call(
                &host,
                &target,
                "invoke",
                invoke("agent", "message.list", "query", json!({}))
            )
            .await["error"],
            "agent_not_bound"
        );
        shutdown(&host).await;
    }
}

// Verifies: TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test]
async fn provider_suspend_recover_failure_and_dispose_end_streams_without_replay() {
    for access in ["agent", "declared"] {
        let fixture = fixture().await;
        let workspace = fixture.core.list_workspaces().unwrap()[0].id();
        let a = fixture.core.add_agent(workspace, "Lifecycle").unwrap();
        fixture
            .core
            .bind_agent_package(workspace, a, "com.example.consumer")
            .unwrap();
        let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
            .await
            .unwrap();
        let target = mount(&host, "com.example.consumer", &workspace.to_string()).await;
        let mut provider = mount(&host, "com.example.provider", &workspace.to_string()).await;
        let key = PackageInstanceKey::new(
            "com.example.provider".parse().unwrap(),
            teamarness_package_host::WorkspaceId::new(workspace.to_string()).unwrap(),
        );
        call(
            &host,
            &target,
            "enable",
            json!({"contract":CONTRACT,"major":1}),
        )
        .await;
        open_idle_streams(&host, &target, &provider, access).await;
        host.execute(PackageHostCommand::Suspend(
            teamarness_package_host::SuspendPackage {
                instance: key.clone(),
            },
        ))
        .await
        .unwrap();
        wait_terminal(&host, &target, "fixture.port_events", "provider_exited").await;
        wait_terminal(
            &host,
            &target,
            "fixture.port_event_events",
            "provider_exited",
        )
        .await;
        let refused = call(
            &host,
            &target,
            "invoke",
            invoke(access, "message.list", "query", json!({})),
        )
        .await;
        assert_eq!(refused["error"], "port_unavailable", "{refused}");
        host.execute(PackageHostCommand::Recover(
            teamarness_package_host::RecoverPackage {
                instance: key.clone(),
            },
        ))
        .await
        .unwrap();
        // Recovery alone does not revive an old stream; explicit reconnect does.
        assert_eq!(
            request(&host, &target, "fixture.port_event_events", json!({})).await["closeCode"],
            "provider_exited"
        );
        open_idle_streams(&host, &target, &provider, access).await;
        request(
            &host,
            &provider,
            "fixture.port_event_emit",
            json!({"order":7}),
        )
        .await;
        common::eventually(
            Duration::from_secs(5),
            "explicit reconnect delivers",
            || async {
                request(&host, &target, "fixture.port_event_events", json!({})).await["events"]
                    == json!([{"order":7}])
            },
        )
        .await;
        let _ = host
            .execute(PackageHostCommand::UiRequest(UiRequest {
                mount: provider.clone(),
                request: UiRequestId::new("crash-provider").unwrap(),
                operation: "fixture.abort".into(),
                payload: serde_json::from_value(json!({})).unwrap(),
            }))
            .await;
        wait_terminal(&host, &target, "fixture.port_events", "provider_exited").await;
        wait_terminal(
            &host,
            &target,
            "fixture.port_event_events",
            "provider_exited",
        )
        .await;
        common::eventually(
            Duration::from_secs(8),
            "Provider replacement active",
            || async {
                host.snapshot().await.instances.iter().any(|i| {
                    i.key == key
                        && i.lifecycle == teamarness_package_host::LifecycleState::Active
                        && i.process_generation.is_some_and(|g| g >= 2)
                })
            },
        )
        .await;
        provider = mount(&host, "com.example.provider", &workspace.to_string()).await;
        assert_eq!(
            request(&host, &provider, "fixture.port_event_stats", json!({})).await["active"],
            0
        );
        open_idle_streams(&host, &target, &provider, access).await;
        host.execute(PackageHostCommand::Dispose(
            teamarness_package_host::DisposePackage {
                instance: key.clone(),
            },
        ))
        .await
        .unwrap();
        wait_terminal(&host, &target, "fixture.port_events", "provider_exited").await;
        wait_terminal(
            &host,
            &target,
            "fixture.port_event_events",
            "provider_exited",
        )
        .await;
        assert_eq!(
            call(&host, &target, "available", json!({"access":"agent"})).await["result"]["contracts"]
                [0]["enabledMajor"],
            1
        );
        shutdown(&host).await;
    }
}

// Verifies: TEAM.SYS.PKG-R14
#[tokio::test]
async fn revocation_does_not_wait_for_credit_from_an_exhausted_consumer() {
    let fixture = fixture().await;
    let workspace = fixture.core.list_workspaces().unwrap()[0].id();
    let agent = fixture.core.add_agent(workspace, "Credit").unwrap();
    fixture
        .core
        .bind_agent_package(workspace, agent, "com.example.other")
        .unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let target = mount(&host, "com.example.other", &workspace.to_string()).await;
    let provider = mount(&host, "com.example.provider", &workspace.to_string()).await;
    call(
        &host,
        &target,
        "enable",
        json!({"contract":CONTRACT,"major":1}),
    )
    .await;
    open_idle_streams(&host, &target, &provider, "agent").await;
    for order in 1..=32 {
        request(
            &host,
            &provider,
            "fixture.port_query_emit",
            json!({"order":order}),
        )
        .await;
        request(
            &host,
            &provider,
            "fixture.port_event_emit",
            json!({"order":order}),
        )
        .await;
        common::eventually(
            Duration::from_secs(5),
            "consumer exhausts credit",
            || async {
                request(
                    &host,
                    &target,
                    "fixture.port_event_events",
                    json!({"summary":true}),
                )
                .await["count"]
                    == order
                    && request(
                        &host,
                        &target,
                        "fixture.port_events",
                        json!({"summary":true}),
                    )
                    .await["count"]
                        == order
            },
        )
        .await;
    }
    request(
        &host,
        &provider,
        "fixture.port_query_emit",
        json!({"order":33}),
    )
    .await;
    request(
        &host,
        &provider,
        "fixture.port_event_emit",
        json!({"order":33}),
    )
    .await;
    call(&host, &target, "disable", json!({"contract":CONTRACT})).await;
    wait_terminal(&host, &target, "fixture.port_events", "port_not_enabled").await;
    wait_terminal(
        &host,
        &target,
        "fixture.port_event_events",
        "port_not_enabled",
    )
    .await;
    common::eventually(
        Duration::from_secs(5),
        "all upstream permits released",
        || async {
            request(&host, &provider, "fixture.port_query_stats", json!({})).await["active"] == 0
                && request(&host, &provider, "fixture.port_event_stats", json!({})).await["active"]
                    == 0
        },
    )
    .await;
    shutdown(&host).await;
}

#[cfg(feature = "test-hooks")]
// Verifies: TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authorization_races_keep_original_command_actor_and_revoke_late_stream_opens() {
    let fixture = fixture().await;
    let workspace = fixture.core.list_workspaces().unwrap()[0].id();
    let a = fixture.core.add_agent(workspace, "Before").unwrap();
    let b = fixture.core.add_agent(workspace, "After").unwrap();
    fixture
        .core
        .bind_agent_package(workspace, a, "com.example.consumer")
        .unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let target = mount(&host, "com.example.consumer", &workspace.to_string()).await;
    let provider = mount(&host, "com.example.provider", &workspace.to_string()).await;
    for access in ["agent", "declared"] {
        let before =
            request(&host, &provider, "fixture.port_last_actor", json!({})).await["commands"]
                .as_u64()
                .unwrap();
        binding(&host, &target, "bind", json!({"principalId":a.to_string()})).await;
        call(
            &host,
            &target,
            "enable",
            json!({"contract":CONTRACT,"major":1}),
        )
        .await;
        let pause =
            host.pause_next_port_authorized_for_tests("com.example.consumer".parse().unwrap());
        let _release = pause.resume_on_drop();
        let h = host.clone();
        let t = target.clone();
        let command = tokio::spawn(async move {
            call(
                &h,
                &t,
                "invoke",
                invoke(
                    access,
                    "message.post",
                    "command",
                    json!({"text":"observe_actor"}),
                ),
            )
            .await
        });
        pause.paused().await;
        call(&host, &target, "disable", json!({"contract":CONTRACT})).await;
        binding(&host, &target, "bind", json!({"principalId":b.to_string()})).await;
        pause.resume();
        assert_eq!(command.await.unwrap()["result"]["accepted"], true);
        // The Provider stores the command's observed actor, independently of the next query actor.
        assert_eq!(
            request(&host, &provider, "fixture.port_last_actor", json!({})).await["actor"]["principal"],
            a.to_string()
        );
        assert_eq!(
            call(
                &host,
                &target,
                "invoke",
                invoke(access, "message.post", "command", json!({"text":"denied"}))
            )
            .await["error"],
            "port_not_enabled"
        );
        call(
            &host,
            &target,
            "enable",
            json!({"contract":CONTRACT,"major":1}),
        )
        .await;
        assert_eq!(
            call(
                &host,
                &target,
                "invoke",
                invoke(access, "message.list", "query", json!({}))
            )
            .await["result"]["actor"]["principal"],
            b.to_string()
        );
        // A command already authorized still uses the common uncertainty classifier, without retry.
        let pause =
            host.pause_next_port_authorized_for_tests("com.example.consumer".parse().unwrap());
        let _release = pause.resume_on_drop();
        let h = host.clone();
        let t = target.clone();
        let command = tokio::spawn(async move {
            call(
                &h,
                &t,
                "invoke",
                invoke(
                    access,
                    "message.post",
                    "command",
                    json!({"text":"committed_error"}),
                ),
            )
            .await
        });
        pause.paused().await;
        call(&host, &target, "disable", json!({"contract":CONTRACT})).await;
        pause.resume();
        let error = command.await.unwrap();
        assert_eq!(error["error"], "provider_error");
        assert_eq!(error["outcomeUncertain"], true);
        assert_eq!(error["retryable"], false);
        assert_eq!(
            request(&host, &provider, "fixture.port_last_actor", json!({})).await["commands"],
            before + 2
        );
        for (open, events) in [
            ("fixture.port_open", "fixture.port_events"),
            ("fixture.port_event_open", "fixture.port_event_events"),
        ] {
            for (transition, code) in [
                ("disable", "port_not_enabled"),
                ("unbind", "agent_not_bound"),
                ("bind", "agent_binding_changed"),
            ] {
                binding(&host, &target, "bind", json!({"principalId":a.to_string()})).await;
                call(
                    &host,
                    &target,
                    "enable",
                    json!({"contract":CONTRACT,"major":1}),
                )
                .await;
                let pause = host
                    .pause_next_port_authorized_for_tests("com.example.consumer".parse().unwrap());
                let _release = pause.resume_on_drop();
                request(&host,&target,open,json!({"access":access,"port":if open == "fixture.port_open" {"message.idle_watch"} else {"message.created"}})).await;
                pause.paused().await;
                match transition {
                    "disable" => {
                        call(&host, &target, "disable", json!({"contract":CONTRACT})).await;
                    }
                    "unbind" => {
                        binding(&host, &target, "unbind", json!({})).await;
                    }
                    _ => {
                        binding(&host, &target, "bind", json!({"principalId":b.to_string()})).await;
                    }
                }
                pause.resume();
                wait_terminal(&host, &target, events, code).await;
            }
        }
    }
    shutdown(&host).await;
}

#[cfg(feature = "test-hooks")]
// Verifies: TEAM.SYS.PKG-R14
#[tokio::test]
async fn accepted_event_drains_before_terminal_but_post_disable_events_do_not_enqueue() {
    let fixture = fixture().await;
    let workspace = fixture.core.list_workspaces().unwrap()[0].id();
    let agent = fixture.core.add_agent(workspace, "Queue").unwrap();
    fixture
        .core
        .bind_agent_package(workspace, agent, "com.example.other")
        .unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let target = mount(&host, "com.example.other", &workspace.to_string()).await;
    let provider = mount(&host, "com.example.provider", &workspace.to_string()).await;
    call(
        &host,
        &target,
        "enable",
        json!({"contract":CONTRACT,"major":1}),
    )
    .await;
    open_idle_streams(&host, &target, &provider, "agent").await;
    let pause = host.pause_next_port_delivery_for_tests("com.example.other".parse().unwrap());
    let _release = pause.resume_on_drop();
    request(
        &host,
        &provider,
        "fixture.port_event_emit",
        json!({"order":1}),
    )
    .await;
    pause.paused().await;
    call(&host, &target, "disable", json!({"contract":CONTRACT})).await;
    request(
        &host,
        &provider,
        "fixture.port_event_emit",
        json!({"order":2}),
    )
    .await;
    pause.resume();
    wait_terminal(
        &host,
        &target,
        "fixture.port_event_events",
        "port_not_enabled",
    )
    .await;
    assert_eq!(
        request(&host, &target, "fixture.port_event_events", json!({})).await["events"],
        json!([{"order":1}])
    );
    shutdown(&host).await;
}

// Verifies: TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test]
async fn remote_manager_removal_ends_idle_streams_through_principals_notifications() {
    let fixture = fixture().await;
    let manager = common::open_core(fixture._root.path().join("manager")).await;
    let workspace = manager.create_workspace("Remote membership").unwrap().id();
    fixture
        .core
        .join_workspace(&common::invite(&manager, workspace).await)
        .await
        .unwrap();
    common::introduce(&fixture.core, &manager);
    let agent = fixture.core.add_agent(workspace, "Remote removal").unwrap();
    fixture
        .core
        .bind_agent_package(workspace, agent, "com.example.consumer")
        .unwrap();
    common::eventually(Duration::from_secs(15), "manager accepts Agent", || async {
        manager.principal(workspace, agent).unwrap().is_some()
    })
    .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let target = mount(&host, "com.example.consumer", &workspace.to_string()).await;
    let provider = mount(&host, "com.example.provider", &workspace.to_string()).await;
    call(
        &host,
        &target,
        "enable",
        json!({"contract":CONTRACT,"major":1}),
    )
    .await;
    open_idle_streams(&host, &target, &provider, "declared").await;
    manager.remove_agent(workspace, agent).unwrap();
    common::eventually(
        Duration::from_secs(15),
        "Steward accepts remote removal",
        || async { fixture.core.principal(workspace, agent).unwrap().is_none() },
    )
    .await;
    wait_terminal(&host, &target, "fixture.port_events", "agent_not_bound").await;
    wait_terminal(
        &host,
        &target,
        "fixture.port_event_events",
        "agent_not_bound",
    )
    .await;
    shutdown(&host).await;
    manager.shutdown().await.unwrap();
}

#[cfg(feature = "test-hooks")]
// Verifies: TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retired_harness_open_cannot_replace_or_close_a_new_generation_subscription() {
    let fixture = fixture().await;
    let workspace = fixture.core.list_workspaces().unwrap()[0].id();
    let agent = fixture
        .core
        .add_agent(workspace, "Harness generation")
        .unwrap();
    fixture
        .core
        .bind_agent_package(workspace, agent, "com.example.other")
        .unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let target = mount(&host, "com.example.other", &workspace.to_string()).await;
    let provider = mount(&host, "com.example.provider", &workspace.to_string()).await;
    call(
        &host,
        &target,
        "enable",
        json!({"contract":CONTRACT,"major":1}),
    )
    .await;
    let pause = host.pause_next_port_authorized_for_tests("com.example.other".parse().unwrap());
    let _release = pause.resume_on_drop();
    request(
        &host,
        &target,
        "fixture.port_event_open",
        json!({"access":"agent"}),
    )
    .await;
    pause.paused().await;
    let _ = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: target,
            request: UiRequestId::new("crash-harness").unwrap(),
            operation: "fixture.abort".into(),
            payload: serde_json::from_value(json!({})).unwrap(),
        }))
        .await;
    common::eventually(
        Duration::from_secs(8),
        "harness generation replaced",
        || async {
            host.snapshot().await.instances.iter().any(|i| {
                i.key.package_identity.as_str() == "com.example.other"
                    && i.lifecycle == teamarness_package_host::LifecycleState::Active
                    && i.process_generation.is_some_and(|g| g >= 2)
            })
        },
    )
    .await;
    let target = mount(&host, "com.example.other", &workspace.to_string()).await;
    open_idle_streams(&host, &target, &provider, "agent").await;
    let mut observations = host.observe_ports();
    pause.resume();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = observations.recv().await.unwrap();
            if event.consumer_package == "com.example.other"
                && event.consumer_generation == 1
                && event.stage == teamarness_package_host::PortStage::Closed
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    request(
        &host,
        &provider,
        "fixture.port_event_emit",
        json!({"order":9}),
    )
    .await;
    common::eventually(
        Duration::from_secs(5),
        "replacement retains its own subscription",
        || async {
            request(&host, &target, "fixture.port_event_events", json!({})).await["events"]
                == json!([{"order":9}])
        },
    )
    .await;
    assert_eq!(
        request(&host, &target, "fixture.port_event_events", json!({})).await["closeCode"],
        Value::Null
    );
    shutdown(&host).await;
}

#[cfg(feature = "test-hooks")]
// Verifies: TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn provider_replacement_revokes_previously_authorized_stream_work() {
    let fixture = fixture().await;
    let workspace = fixture.core.list_workspaces().unwrap()[0].id();
    let agent = fixture
        .core
        .add_agent(workspace, "Provider generation")
        .unwrap();
    fixture
        .core
        .bind_agent_package(workspace, agent, "com.example.consumer")
        .unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let target = mount(&host, "com.example.consumer", &workspace.to_string()).await;
    let mut provider = mount(&host, "com.example.provider", &workspace.to_string()).await;
    call(
        &host,
        &target,
        "enable",
        json!({"contract":CONTRACT,"major":1}),
    )
    .await;
    for (access, open, events) in [
        ("agent", "fixture.port_open", "fixture.port_events"),
        (
            "declared",
            "fixture.port_event_open",
            "fixture.port_event_events",
        ),
    ] {
        let old = host
            .snapshot()
            .await
            .instances
            .iter()
            .find(|i| i.key.package_identity.as_str() == "com.example.provider")
            .unwrap()
            .process_generation
            .unwrap();
        let pause =
            host.pause_next_port_authorized_for_tests("com.example.consumer".parse().unwrap());
        let _release = pause.resume_on_drop();
        request(&host,&target,open,json!({"access":access,"port":if open == "fixture.port_open" {"message.idle_watch"} else {"message.created"}})).await;
        pause.paused().await;
        let _ = host
            .execute(PackageHostCommand::UiRequest(UiRequest {
                mount: provider.clone(),
                request: UiRequestId::new(format!("provider-replace-{old}")).unwrap(),
                operation: "fixture.abort".into(),
                payload: serde_json::from_value(json!({})).unwrap(),
            }))
            .await;
        common::eventually(
            Duration::from_secs(8),
            "Provider generation replaced",
            || async {
                host.snapshot().await.instances.iter().any(|i| {
                    i.key.package_identity.as_str() == "com.example.provider"
                        && i.lifecycle == teamarness_package_host::LifecycleState::Active
                        && i.process_generation.is_some_and(|g| g > old)
                })
            },
        )
        .await;
        pause.resume();
        wait_terminal(&host, &target, events, "provider_exited").await;
        provider = mount(&host, "com.example.provider", &workspace.to_string()).await;
        assert_eq!(
            request(&host, &provider, "fixture.port_query_stats", json!({})).await["active"],
            0
        );
        assert_eq!(
            request(&host, &provider, "fixture.port_event_stats", json!({})).await["active"],
            0
        );
    }
    // Only an explicit current-generation request opens a new stream.
    open_idle_streams(&host, &target, &provider, "agent").await;
    request(
        &host,
        &provider,
        "fixture.port_event_emit",
        json!({"order":10}),
    )
    .await;
    common::eventually(
        Duration::from_secs(5),
        "explicit Provider reconnect",
        || async {
            request(&host, &target, "fixture.port_event_events", json!({})).await["events"]
                == json!([{"order":10}])
        },
    )
    .await;
    shutdown(&host).await;
}

#[cfg(feature = "test-hooks")]
// Verifies: TEAM.SYS.PKG-R13
#[tokio::test]
async fn selection_pause_unwind_reports_original_failure_promptly() {
    const PROBE: &str = "TEAMARNESS_SELECTION_PAUSE_UNWIND_PROBE";
    if std::env::var_os(PROBE).is_some() {
        let fixture = fixture().await;
        let workspace = fixture.core.list_workspaces().unwrap()[0].id();
        let agent = fixture.core.add_agent(workspace, "Unwind Agent").unwrap();
        fixture
            .core
            .bind_agent_package(workspace, agent, "com.example.other")
            .unwrap();
        let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
            .await
            .unwrap();
        let target = mount(&host, "com.example.other", &workspace.to_string()).await;
        let pause = host.pause_next_port_selection_for_tests("com.example.other".parse().unwrap());
        let _release = pause.resume_on_drop();
        tokio::spawn(async move {
            call(
                &host,
                &target,
                "enable",
                json!({"contract":CONTRACT,"major":1}),
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(8), pause.paused())
            .await
            .expect("selection pause entered");
        panic!("intentional selection pause assertion failure");
    }
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "selection_pause_unwind_reports_original_failure_promptly",
            "--nocapture",
        ])
        .env(PROBE, "1")
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.as_std_mut().process_group(0);
    }
    let child = command.spawn().unwrap();
    let pid = child.id().unwrap();
    let output = match tokio::time::timeout(Duration::from_secs(8), child.wait_with_output()).await
    {
        Ok(output) => output.unwrap(),
        Err(error) => {
            #[cfg(unix)]
            let _ = std::process::Command::new("kill")
                .args(["-KILL", "--", &format!("-{pid}")])
                .status();
            panic!("paused storage stranded original failure: {error}");
        }
    };
    assert!(
        !output.status.success(),
        "intentional failure must remain a failure"
    );
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.contains("intentional selection pause assertion failure"),
        "{output}"
    );
    assert!(
        output.contains("test result: FAILED"),
        "original test failure must reach harness output: {output}"
    );
}
