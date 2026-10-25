mod common;

use std::fs;
use teamarness_package_host::{
    LifecycleState, OpenUiMount, PackageHost, PackageHostCommand, PackageHostOutcome,
    PackageInstanceKey, UiMountId, UiRequest, UiRequestId,
};

fn declare(fixture: &common::Fixture, directory: &str, provided: bool) {
    let root = fixture.config.application_root().join(directory);
    let path = root.join("teamarness.package.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    if provided {
        let bytes = serde_json::to_vec(&serde_json::json!({
            "schemaVersion":1,"$defs":{},"contracts":[{
                "identity":"com.example.messages","major":1,"minor":0,"description":"Fixture messages",
                "ports":[
                    {"name":"message.post","kind":"command","description":"Post a message", "request":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false},"response":{"type":"object","properties":{"accepted":{"type":"boolean"}},"required":["accepted"],"additionalProperties":false}},
                    {"name":"message.list","kind":"query","description":"List messages", "request":{"type":"object","additionalProperties":false},"response":{"type":"object","properties":{"accepted":{"type":"boolean"},"actor":{"type":"object","properties":{"kind":{"enum":["person","agent"]},"identity":{"type":"string"},"principal":{"type":"string"}},"required":["kind","identity"],"additionalProperties":false}},"required":["accepted","actor"],"additionalProperties":false}},
                    {"name":"message.watch","kind":"query","description":"Watch messages", "request":{"type":"object","additionalProperties":false},"event":{"type":"object","properties":{"order":{"type":"integer"}},"required":["order"],"additionalProperties":false}},
                    {"name":"message.actor_watch","kind":"query","description":"Watch as the Host-derived actor", "request":{"type":"object","additionalProperties":false},"event":{"type":"object","properties":{"actor":{"type":"object"}},"required":["actor"],"additionalProperties":false}}
                ]
            }]
        })).unwrap();
        fs::write(root.join("ports.json"), &bytes).unwrap();
        manifest["portContract"] =
            serde_json::json!({"path":"ports.json","sha256":common::digest(&bytes)});
        manifest["activation"] = "on_demand".into();
    } else {
        manifest["consumedPorts"] = serde_json::json!([{
            "identity":"com.example.messages","major":1,"minMinor":0,"maxMinor":0,
            "ports":[{"name":"message.post","kind":"command"},{"name":"message.list","kind":"query"},{"name":"message.watch","kind":"query"},{"name":"message.actor_watch","kind":"query"}]
        }]);
    }
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
}

fn declare_non_object_ports(fixture: &common::Fixture) {
    let provider = fixture.config.application_root().join("provider");
    let path = provider.join("ports.json");
    let mut document: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    document["contracts"][0]["major"] = 2.into();
    document["contracts"][0]["ports"].as_array_mut().unwrap().extend([
        serde_json::json!({"name":"message.title","kind":"query","description":"Read a title", "request":{"type":"object","additionalProperties":false},"response":{"type":"string"}}),
        serde_json::json!({"name":"message.title_watch","kind":"query","description":"Watch title parts", "request":{"type":"object","additionalProperties":false},"event":{"type":"array","items":{"type":"integer"}}}),
        serde_json::json!({"name":"message.marker","kind":"query","description":"Read a literal envelope-shaped object", "request":{"type":"object","additionalProperties":false},"response":{"type":"object"}}),
        serde_json::json!({"name":"message.marker_watch","kind":"query","description":"Watch literal envelope-shaped objects", "request":{"type":"object","additionalProperties":false},"event":{"type":"object"}}),
        serde_json::json!({"name":"message.large_watch","kind":"query","description":"Watch a maximum-sized value", "request":{"type":"object","additionalProperties":false},"event":{"type":"string"}}),
    ]);
    let bytes = serde_json::to_vec(&document).unwrap();
    fs::write(&path, &bytes).unwrap();
    let manifest_path = provider.join("teamarness.package.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["portContract"]["sha256"] = common::digest(&bytes).into();
    fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();

    let consumer = fixture
        .config
        .application_root()
        .join("consumer/teamarness.package.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&consumer).unwrap()).unwrap();
    manifest["consumedPorts"][0]["major"] = 2.into();
    manifest["consumedPorts"][0]["ports"]
        .as_array_mut()
        .unwrap()
        .extend([
            serde_json::json!({"name":"message.title","kind":"query"}),
            serde_json::json!({"name":"message.title_watch","kind":"query"}),
            serde_json::json!({"name":"message.marker","kind":"query"}),
            serde_json::json!({"name":"message.marker_watch","kind":"query"}),
            serde_json::json!({"name":"message.large_watch","kind":"query"}),
        ]);
    fs::write(consumer, serde_json::to_vec(&manifest).unwrap()).unwrap();
}

async fn mount(host: &PackageHost, key: PackageInstanceKey) -> UiMountId {
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

async fn request(
    host: &PackageHost,
    mount: &UiMountId,
    id: &str,
    operation: &str,
    payload: serde_json::Value,
) -> serde_json::Value {
    let result = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: mount.clone(),
            request: UiRequestId::new(id).unwrap(),
            operation: operation.into(),
            payload: serde_json::from_value(payload).unwrap(),
        }))
        .await
        .unwrap();
    let PackageHostOutcome::UiResponse { payload } = result else {
        panic!("{result:?}")
    };
    serde_json::to_value(payload).unwrap()
}

// Verifies: TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test]
async fn concurrent_first_calls_share_on_demand_provider_activation() {
    let fixture = common::Fixture::node_with_capabilities(
        &[("provider", "com.example.provider"), ("consumer", "com.example.consumer")],
        &[serde_json::json!({"identity":"teamarness.package-ports","major":2,"minMinor":0,"maxMinor":0})],
    ).await;
    declare(&fixture, "provider", true);
    declare(&fixture, "consumer", false);
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let snapshot = host.snapshot().await;
    let consumer = snapshot
        .instances
        .iter()
        .find(|slot| slot.key.package_identity.as_str() == "com.example.consumer")
        .unwrap()
        .key
        .clone();
    let mount = mount(&host, consumer).await;
    let command = request(
        &host,
        &mount,
        "first-command",
        "fixture.capability",
        serde_json::json!({
            "capability":"teamarness.package-ports", "method":"invoke",
            "arguments":{"access":"declared","major":1,"contract":"com.example.messages","port":"message.post","kind":"command","request":{"text":"first"}}
        }),
    );
    let query = request(
        &host,
        &mount,
        "first-query",
        "fixture.capability",
        serde_json::json!({
            "capability":"teamarness.package-ports", "method":"invoke",
            "arguments":{"access":"declared","major":1,"contract":"com.example.messages","port":"message.list","kind":"query","request":{}}
        }),
    );
    let (posted, listed) = tokio::join!(command, query);
    assert_eq!(
        posted["result"]["accepted"], true,
        "first command: {posted:?}"
    );
    assert_eq!(
        listed["result"]["accepted"], true,
        "concurrent query: {listed:?}"
    );
    let provider = host
        .snapshot()
        .await
        .instances
        .into_iter()
        .find(|slot| slot.key.package_identity.as_str() == "com.example.provider")
        .unwrap();
    assert_eq!(provider.process_generation, Some(1));
}

/// Opens the `message.actor_watch` live query and returns the actor the
/// Provider saw in its first event.
async fn live_actor(host: &PackageHost, mount: &UiMountId, id: &str) -> serde_json::Value {
    let opened = request(
        host,
        mount,
        id,
        "fixture.port_open",
        serde_json::json!({"port":"message.actor_watch"}),
    )
    .await;
    assert_eq!(opened["opened"], true, "{opened}");
    let polls = std::sync::atomic::AtomicUsize::new(0);
    let events = || async {
        let poll = polls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        request(
            host,
            mount,
            &format!("{id}-events-{poll}"),
            "fixture.port_events",
            serde_json::json!({}),
        )
        .await
    };
    common::eventually(
        std::time::Duration::from_secs(5),
        "the live query completes",
        || async { events().await["closeCode"] == "completed" },
    )
    .await;
    events().await["events"][0]["actor"].clone()
}

/// Verifies: TEAM.SYS.PKG-R13 (Host-derived actor, ADR 0030)
#[tokio::test]
async fn a_bound_consumer_presents_an_agent_actor_and_an_unbound_one_its_person() {
    let fixture = common::Fixture::node_with_capabilities(
        &[("provider", "com.example.provider"), ("consumer", "com.example.consumer")],
        &[serde_json::json!({"identity":"teamarness.package-ports","major":2,"minMinor":0,"maxMinor":0})],
    ).await;
    declare(&fixture, "provider", true);
    declare(&fixture, "consumer", false);
    let workspace = fixture.core.list_workspaces().unwrap()[0].id();
    let agent = fixture.core.add_agent(workspace, "Bound Agent").unwrap();
    let agent_key = fixture
        .core
        .principal(workspace, agent)
        .unwrap()
        .unwrap()
        .keys()[0];
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let consumer = host
        .snapshot()
        .await
        .instances
        .iter()
        .find(|slot| slot.key.package_identity.as_str() == "com.example.consumer")
        .unwrap()
        .key
        .clone();
    let mount = mount(&host, consumer).await;
    let person =
        serde_json::json!({"kind":"person","identity":fixture.core.public_identity().to_string()});
    let as_agent = serde_json::json!({"kind":"agent","principal":agent.to_string(),"identity":agent_key.to_string()});
    let query = |id: &'static str| {
        request(
            &host,
            &mount,
            id,
            "fixture.capability",
            serde_json::json!({
                "capability":"teamarness.package-ports","method":"invoke",
                "arguments":{"access":"declared","major":1,"contract":"com.example.messages","port":"message.list","kind":"query","request":{}}
            }),
        )
    };
    assert_eq!(query("unbound").await["result"]["actor"], person);
    assert_eq!(live_actor(&host, &mount, "unbound-live").await, person);

    // Binding is read per dispatch: no restart, no cache.
    fixture
        .core
        .bind_agent_package(workspace, agent, "com.example.consumer")
        .unwrap();
    assert_eq!(query("bound").await["result"]["actor"], as_agent);
    assert_eq!(live_actor(&host, &mount, "bound-live").await, as_agent);
    // A different bound Package does not change this caller's actor, and a
    // caller-supplied actor is still refused.
    fixture
        .core
        .bind_agent_package(workspace, agent, "com.example.other")
        .unwrap();
    let forged = request(&host, &mount, "forged-agent", "fixture.capability", serde_json::json!({
        "capability":"teamarness.package-ports","method":"invoke",
        "arguments":{"access":"declared","major":1,"contract":"com.example.messages","port":"message.list","kind":"query","request":{},
            "actor":person}
    })).await;
    assert_eq!(forged["error"], "capability_request_invalid");

    fixture
        .core
        .unbind_agent_package(workspace, "com.example.consumer")
        .unwrap();
    assert_eq!(query("unbound-again").await["result"]["actor"], person);
    assert_eq!(
        live_actor(&host, &mount, "unbound-again-live").await,
        person
    );
}

// Verifies: TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test]
async fn exact_scoped_calls_activate_provider_and_validate_request() {
    let fixture = common::Fixture::node_with_capabilities(
        &[("provider", "com.example.provider"), ("consumer", "com.example.consumer")],
        &[serde_json::json!({"identity":"teamarness.package-ports","major":2,"minMinor":0,"maxMinor":0})],
    ).await;
    declare(&fixture, "provider", true);
    declare(&fixture, "consumer", false);
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let snapshot = host.snapshot().await;
    let provider = snapshot
        .instances
        .iter()
        .find(|slot| slot.key.package_identity.as_str() == "com.example.provider")
        .unwrap();
    assert_eq!(provider.lifecycle, LifecycleState::Admitted);
    let consumer = snapshot
        .instances
        .iter()
        .find(|slot| slot.key.package_identity.as_str() == "com.example.consumer")
        .unwrap()
        .key
        .clone();
    let mount = mount(&host, consumer).await;
    let available = request(
        &host,
        &mount,
        "available",
        "fixture.capability",
        serde_json::json!({
            "capability":"teamarness.package-ports", "method":"available","arguments":{"access":"declared"}
        }),
    )
    .await;
    assert_eq!(
        available["result"]["contracts"][0]["available"], true,
        "an admitted on-demand Provider is available without having started"
    );
    assert_eq!(
        host.snapshot()
            .await
            .instances
            .iter()
            .find(|slot| slot.key.package_identity.as_str() == "com.example.provider")
            .unwrap()
            .lifecycle,
        LifecycleState::Admitted
    );
    let mut observations = host.observe_ports();
    for (id, kind, port, input, expected) in [
        (
            "one",
            "command",
            "message.post",
            serde_json::json!({"text":"hi"}),
            "accepted",
        ),
        (
            "two",
            "query",
            "message.list",
            serde_json::json!({}),
            "accepted",
        ),
        (
            "three",
            "command",
            "message.list",
            serde_json::json!({}),
            "ungranted_port",
        ),
        (
            "four",
            "command",
            "message.post",
            serde_json::json!({"text":1}),
            "port_request_invalid",
        ),
    ] {
        let value = request(&host, &mount, id, "fixture.capability", serde_json::json!({
            "capability":"teamarness.package-ports","method":"invoke",
            "arguments":{"access":"declared","major":1,"contract":"com.example.messages","port":port,"kind":kind,"request":input}
        })).await;
        if expected == "accepted" {
            assert_eq!(value["result"]["accepted"], true, "{value}");
            if port == "message.list" {
                assert_eq!(
                    value["result"]["actor"],
                    serde_json::json!({
                        "kind":"person", "identity":fixture.core.public_identity().to_string()
                    })
                );
            }
        } else {
            assert_eq!(value["error"], expected, "{value}");
        }
    }
    let trace: Vec<_> = std::iter::from_fn(|| observations.try_recv().ok()).collect();
    let dispatch: Vec<_> = trace
        .iter()
        .filter(|event| event.stage == teamarness_package_host::PortStage::Dispatched)
        .collect();
    assert_eq!(dispatch.len(), 2);
    for assigned in dispatch {
        assert!(assigned.consumer_id.starts_with("p:"));
        assert!(
            assigned
                .provider_id
                .as_deref()
                .unwrap()
                .starts_with("h:port-")
        );
        assert!(
            trace
                .iter()
                .any(|terminal| terminal.consumer_id == assigned.consumer_id
                    && terminal.provider_id == assigned.provider_id
                    && terminal.stage == teamarness_package_host::PortStage::Completed)
        );
    }
    assert_eq!(
        trace
            .iter()
            .filter(
                |event| event.stage == teamarness_package_host::PortStage::Rejected
                    && event.provider_id.is_none()
            )
            .count(),
        2
    );
    assert!(
        !format!("{trace:?}").contains("hi"),
        "observations must contain no Message payload"
    );
    let cancelled = request(
        &host,
        &mount,
        "cancel-race",
        "fixture.port_cancel",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(cancelled["cancelled"], true);
    let after_cancel = request(&host, &mount, "after-cancel", "fixture.capability", serde_json::json!({
        "capability":"teamarness.package-ports","method":"invoke",
        "arguments":{"access":"declared","major":1,"contract":"com.example.messages","port":"message.list","kind":"query","request":{}}
    })).await;
    assert_eq!(
        after_cancel["result"]["accepted"], true,
        "a cancelled call leaves the session usable"
    );
    let bad_response = request(&host, &mount, "bad-response", "fixture.capability", serde_json::json!({
        "capability":"teamarness.package-ports","method":"invoke",
        "arguments":{"access":"declared","major":1,"contract":"com.example.messages","port":"message.post","kind":"command","request":{"text":"bad_response"}}
    })).await;
    assert_eq!(bad_response["error"], "port_response_invalid");
    assert_eq!(bad_response["outcomeUncertain"], true);
    assert_eq!(bad_response["retryable"], false);
    let failure_trace: Vec<_> = std::iter::from_fn(|| observations.try_recv().ok()).collect();
    assert!(failure_trace.iter().any(|event| {
        event.stage == teamarness_package_host::PortStage::Failed
            && event.code == Some("port_response_invalid")
            && event.outcome_uncertain
            && event
                .provider_id
                .as_deref()
                .is_some_and(|id| id.starts_with("h:port-"))
    }));
    let owned = request(&host, &mount, "owned-error", "fixture.capability", serde_json::json!({
        "capability":"teamarness.package-ports","method":"invoke",
        "arguments":{"access":"declared","major":1,"contract":"com.example.messages","port":"message.post","kind":"command","request":{"text":"provider_error"}}
    })).await;
    assert_eq!(owned["error"], "provider_error");
    assert_eq!(owned["details"]["provider"]["code"], "fixture.rejected");
    let committed = request(&host, &mount, "committed-error", "fixture.capability", serde_json::json!({
        "capability":"teamarness.package-ports","method":"invoke",
        "arguments":{"access":"declared","major":1,"contract":"com.example.messages","port":"message.post","kind":"command","request":{"text":"committed_error"}}
    })).await;
    assert_eq!(committed["error"], "provider_error");
    assert_eq!(
        committed["details"]["provider"]["code"],
        "fixture.committed"
    );
    assert_eq!(committed["outcomeUncertain"], true);
    assert_eq!(committed["retryable"], false);
    let forged = request(&host, &mount, "forged", "fixture.capability", serde_json::json!({
        "capability":"teamarness.package-ports","method":"invoke",
        "arguments":{"access":"declared","major":1,"contract":"com.example.messages","port":"message.post","kind":"command",
            "request":{"text":"hi"},"workspaceId":"other","actor":{"kind":"agent","identity":"forged"}}
    })).await;
    assert_eq!(forged["error"], "capability_request_invalid");

    let stalled = request(&host, &mount, "stalled", "fixture.capability", serde_json::json!({
        "capability":"teamarness.package-ports","method":"invoke",
        "arguments":{"access":"declared","major":1,"contract":"com.example.messages","port":"message.post","kind":"command","request":{"text":"stall"}}
    })).await;
    assert_eq!(stalled["error"], "outcome_unknown");
    assert_eq!(stalled["outcomeUncertain"], true);
    assert_eq!(stalled["retryable"], false);
    let timeout_trace: Vec<_> = std::iter::from_fn(|| observations.try_recv().ok()).collect();
    assert!(!timeout_trace.iter().any(|event| {
        event.stage == teamarness_package_host::PortStage::Completed
            && event.port.as_deref() == Some("message.post")
            && event.provider_id.as_deref().is_some_and(|id| {
                timeout_trace.iter().any(|other| {
                    other.stage == teamarness_package_host::PortStage::Dispatched
                        && other.provider_id.as_deref() == Some(id)
                        && other.consumer_id == event.consumer_id
                })
            })
    }));
    let result = request(
        &host,
        &mount,
        "live",
        "fixture.port_open",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(result["opened"], true);
    let mut next = 0;
    common::eventually(
        std::time::Duration::from_secs(3),
        "two live query events and completion",
        || {
            next += 1;
            let host = &host;
            let mount = mount.clone();
            async move {
                let value = request(
                    host,
                    &mount,
                    &format!("events-{next}"),
                    "fixture.port_events",
                    serde_json::json!({}),
                )
                .await;
                value["events"] == serde_json::json!([{"order":1},{"order":2}])
                    && value["closeCode"] == "completed"
            }
        },
    )
    .await;
    let provider = host
        .snapshot()
        .await
        .instances
        .into_iter()
        .find(|slot| slot.key.package_identity.as_str() == "com.example.provider")
        .unwrap();
    assert_eq!(provider.lifecycle, LifecycleState::Active);
    let exited = request(&host, &mount, "exited", "fixture.capability", serde_json::json!({
        "capability":"teamarness.package-ports","method":"invoke",
        "arguments":{"access":"declared","major":1,"contract":"com.example.messages","port":"message.post","kind":"command","request":{"text":"exit"}}
    })).await;
    assert_eq!(exited["error"], "outcome_unknown");
    assert_eq!(exited["outcomeUncertain"], true);
    assert_eq!(exited["retryable"], false);
    common::eventually(
        std::time::Duration::from_secs(8),
        "provider replacement after exit",
        || {
            let host = &host;
            async move {
                host.snapshot().await.instances.iter().any(|slot| {
                    slot.key.package_identity.as_str() == "com.example.provider"
                        && slot.lifecycle == LifecycleState::Active
                        && slot
                            .process_generation
                            .is_some_and(|generation| generation >= 2)
                })
            }
        },
    )
    .await;
    assert_eq!(
        port_attempts(&fixture._private),
        1,
        "a command was not automatically replayed"
    );
    assert!(
        host.snapshot()
            .await
            .instances
            .iter()
            .any(
                |slot| slot.key.package_identity.as_str() == "com.example.consumer"
                    && slot.lifecycle == LifecycleState::Active
            )
    );
}

fn port_attempts(root: &std::path::Path) -> usize {
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                total += port_attempts(&path);
            } else if path
                .file_name()
                .is_some_and(|name| name == "port-command-attempts")
            {
                total += fs::read_to_string(path).unwrap().lines().count();
            }
        }
    }
    total
}

// Verifies: TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test]
async fn major_two_port_values_round_trip_through_object_only_frames() {
    let fixture = common::Fixture::node_with_capabilities(
        &[("provider", "com.example.provider"), ("consumer", "com.example.consumer")],
        &[serde_json::json!({"identity":"teamarness.package-ports","major":2,"minMinor":0,"maxMinor":0})],
    ).await;
    declare(&fixture, "provider", true);
    declare(&fixture, "consumer", false);
    declare_non_object_ports(&fixture);
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let consumer = host
        .snapshot()
        .await
        .instances
        .into_iter()
        .find(|slot| slot.key.package_identity.as_str() == "com.example.consumer")
        .unwrap()
        .key;
    let mount = mount(&host, consumer).await;
    let title = request(&host, &mount, "title", "fixture.capability", serde_json::json!({
        "capability":"teamarness.package-ports","method":"invoke",
        "arguments":{"access":"declared","major":2,"contract":"com.example.messages","port":"message.title","kind":"query","request":{}}
    })).await;
    assert_eq!(title["result"]["$teamarnessPortEnvelope"], 1);
    assert_eq!(title["result"]["$teamarnessPortValue"], "hello");
    let object = request(&host, &mount, "object", "fixture.capability", serde_json::json!({
        "capability":"teamarness.package-ports","method":"invoke",
        "arguments":{"access":"declared","major":2,"contract":"com.example.messages","port":"message.list","kind":"query","request":{}}
    })).await;
    assert_eq!(object["result"]["accepted"], true);
    let marker = request(&host, &mount, "marker", "fixture.capability", serde_json::json!({
        "capability":"teamarness.package-ports","method":"invoke",
        "arguments":{"access":"declared","major":2,"contract":"com.example.messages","port":"message.marker","kind":"query","request":{}}
    })).await;
    assert_eq!(
        marker["result"]["$teamarnessPortValue"],
        serde_json::json!({"$teamarnessPortEnvelope":1,"$teamarnessPortValue":"literal"})
    );
    let opened = request(
        &host,
        &mount,
        "open-title",
        "fixture.port_open",
        serde_json::json!({"major":2,"port":"message.title_watch"}),
    )
    .await;
    assert_eq!(opened["opened"], true);
    let mut next = 0;
    common::eventually(
        std::time::Duration::from_secs(3),
        "non-object Port event",
        || {
            next += 1;
            let host = &host;
            let mount = mount.clone();
            async move {
                let events = request(
                    host,
                    &mount,
                    &format!("title-events-{next}"),
                    "fixture.port_events",
                    serde_json::json!({}),
                )
                .await;
                events["events"][0]["$teamarnessPortValue"] == serde_json::json!([1, 2])
                    && events["closeCode"] == "completed"
            }
        },
    )
    .await;
    let opened = request(
        &host,
        &mount,
        "open-marker",
        "fixture.port_open",
        serde_json::json!({"major":2,"port":"message.marker_watch"}),
    )
    .await;
    assert_eq!(opened["opened"], true);
    let mut next = 0;
    common::eventually(std::time::Duration::from_secs(3), "literal marker Port event", || {
        next += 1;
        let host = &host;
        let mount = mount.clone();
        async move {
            let events = request(host, &mount, &format!("marker-events-{next}"),
                "fixture.port_events", serde_json::json!({})).await;
            events["events"][0]["$teamarnessPortValue"] ==
                serde_json::json!({"$teamarnessPortEnvelope":1,"$teamarnessPortValue":"literal"})
                && events["closeCode"] == "completed"
        }
    }).await;
    let opened = request(
        &host,
        &mount,
        "open-large",
        "fixture.port_open",
        serde_json::json!({"major":2,"port":"message.large_watch"}),
    )
    .await;
    assert_eq!(opened["opened"], true);
    let mut next = 0;
    common::eventually(
        std::time::Duration::from_secs(3),
        "maximum logical Port event",
        || {
            next += 1;
            let host = &host;
            let mount = mount.clone();
            async move {
                let events = request(
                    host,
                    &mount,
                    &format!("large-events-{next}"),
                    "fixture.port_events",
                    serde_json::json!({"summary":true}),
                )
                .await;
                events["count"] == 1
                    && events["firstLength"] == 262142
                    && events["closeCode"] == "completed"
            }
        },
    )
    .await;
}

// Verifies: TEAM.SYS.PKG-R14
#[tokio::test]
async fn invalid_live_query_event_ends_only_that_stream() {
    let fixture = common::Fixture::node_with_capabilities(
        &[("provider", "com.example.invalid"), ("consumer", "com.example.consumer")],
        &[serde_json::json!({"identity":"teamarness.package-ports","major":2,"minMinor":0,"maxMinor":0})],
    ).await;
    declare(&fixture, "provider", true);
    declare(&fixture, "consumer", false);
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let consumer = host
        .snapshot()
        .await
        .instances
        .into_iter()
        .find(|slot| slot.key.package_identity.as_str() == "com.example.consumer")
        .unwrap()
        .key;
    let mount = mount(&host, consumer).await;
    let opened = request(
        &host,
        &mount,
        "invalid-live",
        "fixture.port_open",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(opened["opened"], true);
    let mut next = 0;
    common::eventually(
        std::time::Duration::from_secs(3),
        "invalid event closes the live query",
        || {
            next += 1;
            let host = &host;
            let mount = mount.clone();
            async move {
                let value = request(
                    host,
                    &mount,
                    &format!("invalid-events-{next}"),
                    "fixture.port_events",
                    serde_json::json!({}),
                )
                .await;
                value["events"] == serde_json::json!([])
                    && value["closeCode"] == "port_event_invalid"
            }
        },
    )
    .await;
    let query = request(&host, &mount, "still-usable", "fixture.capability", serde_json::json!({
        "capability":"teamarness.package-ports","method":"invoke",
        "arguments":{"access":"declared","major":1,"contract":"com.example.messages","port":"message.list","kind":"query","request":{}}
    })).await;
    assert_eq!(query["result"]["accepted"], true);
    assert_eq!(
        host.snapshot()
            .await
            .instances
            .iter()
            .find(|slot| slot.key.package_identity.as_str() == "com.example.invalid")
            .unwrap()
            .lifecycle,
        LifecycleState::Active
    );
}

// Verifies: TEAM.SYS.PKG-R13
#[tokio::test]
async fn a_consumer_wakes_only_its_own_workspace_provider() {
    let fixture = common::Fixture::node_with_capabilities(
        &[("provider", "com.example.provider"), ("consumer", "com.example.consumer")],
        &[serde_json::json!({"identity":"teamarness.package-ports","major":2,"minMinor":0,"maxMinor":0})],
    ).await;
    declare(&fixture, "provider", true);
    declare(&fixture, "consumer", false);
    let second = fixture.core.create_workspace("Second Workspace").unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let consumer = host
        .snapshot()
        .await
        .instances
        .into_iter()
        .find(|slot| {
            slot.key.package_identity.as_str() == "com.example.consumer"
                && slot.key.workspace_id.as_str() == second.id().to_string()
        })
        .unwrap()
        .key;
    let mount = mount(&host, consumer).await;
    let value = request(&host, &mount, "second-workspace", "fixture.capability", serde_json::json!({
        "capability":"teamarness.package-ports","method":"invoke",
        "arguments":{"access":"declared","major":1,"contract":"com.example.messages","port":"message.list","kind":"query","request":{}}
    })).await;
    assert_eq!(value["result"]["accepted"], true);
    let providers = host
        .snapshot()
        .await
        .instances
        .into_iter()
        .filter(|slot| slot.key.package_identity.as_str() == "com.example.provider")
        .collect::<Vec<_>>();
    assert_eq!(providers.len(), 2);
    assert_eq!(
        providers
            .iter()
            .filter(|slot| slot.lifecycle == LifecycleState::Active)
            .count(),
        1
    );
    assert_eq!(
        providers
            .iter()
            .find(|slot| slot.lifecycle == LifecycleState::Active)
            .unwrap()
            .key
            .workspace_id
            .as_str(),
        second.id().to_string()
    );
}

// Verifies: TEAM.SYS.PKG-R13
#[tokio::test]
async fn a_replacement_generation_rejects_its_predecessors_grant() {
    let fixture = common::Fixture::node_with_capabilities(
        &[("consumer", "com.example.consumer")],
        &[serde_json::json!({"identity":"teamarness.package-ports","major":2,"minMinor":0,"maxMinor":0})],
    ).await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let key = host.snapshot().await.instances[0].key.clone();
    let first = mount(&host, key.clone()).await;
    let result = request(
        &host,
        &first,
        "initial-grant",
        "fixture.grant",
        serde_json::json!({"capability":"teamarness.package-ports"}),
    )
    .await;
    let old_grant = result["grant"].as_str().unwrap().to_owned();
    assert_ne!(old_grant, "missing");
    let _ = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: first,
            request: UiRequestId::new("crash").unwrap(),
            operation: "fixture.abort".into(),
            payload: serde_json::from_value(serde_json::json!({})).unwrap(),
        }))
        .await;
    common::eventually(
        std::time::Duration::from_secs(8),
        "consumer generation replaced",
        || {
            let host = &host;
            async move {
                host.snapshot().await.instances.iter().any(|slot| {
                    slot.lifecycle == LifecycleState::Active
                        && slot
                            .process_generation
                            .is_some_and(|generation| generation >= 2)
                })
            }
        },
    )
    .await;
    let second = mount(&host, key).await;
    let result = request(
        &host,
        &second,
        "old-grant",
        "fixture.capability",
        serde_json::json!({
            "grant":old_grant,"method":"available","arguments":{"access":"declared"}
        }),
    )
    .await;
    assert_eq!(result["error"], "capability_request_invalid");
    let current = request(
        &host,
        &second,
        "current-grant",
        "fixture.capability",
        serde_json::json!({
            "capability":"teamarness.package-ports","method":"available","arguments":{"access":"declared"}
        }),
    )
    .await;
    assert!(current["result"]["contracts"].is_array());
}

// Verifies: TEAM.SYS.PKG-R13
#[tokio::test]
async fn optional_absence_is_filtered_and_an_attempt_cannot_dispatch() {
    let fixture = common::Fixture::node_with_capabilities(
        &[("consumer", "com.example.consumer")],
        &[serde_json::json!({"identity":"teamarness.package-ports","major":2,"minMinor":0,"maxMinor":0})],
    ).await;
    declare(&fixture, "consumer", false);
    let path = fixture
        .config
        .application_root()
        .join("consumer/teamarness.package.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["consumedPorts"][0]["required"] = false.into();
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let consumer = host.snapshot().await.instances[0].key.clone();
    let mount = mount(&host, consumer).await;
    let available = request(
        &host,
        &mount,
        "available",
        "fixture.capability",
        serde_json::json!({
            "capability":"teamarness.package-ports", "method":"available","arguments":{"access":"declared"}
        }),
    )
    .await;
    assert_eq!(available["result"]["contracts"][0]["available"], false);
    assert!(available["result"]["contracts"][0].get("ports").is_none());
    let result = request(&host, &mount, "missing", "fixture.capability", serde_json::json!({
        "capability":"teamarness.package-ports", "method":"invoke",
        "arguments":{"access":"declared","major":1,"contract":"com.example.messages","port":"message.list","kind":"query","request":{}}
    })).await;
    assert_eq!(result["error"], "port_unavailable");
}
