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
            manifest["consumedPorts"] = json!([{"identity":CONTRACT,"major":1,"minMinor":0,"maxMinor":0,"required":false,"ports":[{"name":"message.post","kind":"command"},{"name":"message.list","kind":"query"},{"name":"message.actor_watch","kind":"query"},{"name":"message.created","kind":"event"}]}]);
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
        5
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
