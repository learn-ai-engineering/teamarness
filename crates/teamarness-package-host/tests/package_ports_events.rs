mod common;

use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

use serde_json::{Value, json};
use teamarness_package_host::{
    LifecycleState, OpenUiMount, PackageHost, PackageHostCommand, PackageHostOutcome,
    PackageInstanceKey, UiMountId, UiRequest, UiRequestId,
};

fn declare(fixture: &common::Fixture, directory: &str, provider: bool) {
    let root = fixture.config.application_root().join(directory);
    let path = root.join("teamarness.package.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    if provider {
        let bytes = serde_json::to_vec(&json!({"schemaVersion":1,"$defs":{},"contracts":[{
            "identity":"com.example.messages","major":1,"minor":0,"description":"Message fixture",
            "ports":[{"name":"message.created","kind":"event","description":"Message was created",
                "event":{"type":"object","properties":{"order":{"type":"integer"},"padding":{"type":"string"}},
                    "required":["order"],"additionalProperties":false}}
            ]}]})).unwrap();
        fs::write(root.join("ports.json"), &bytes).unwrap();
        manifest["portContract"] = json!({"path":"ports.json","sha256":common::digest(&bytes)});
        manifest["activation"] = "on_demand".into();
    } else {
        manifest["consumedPorts"] = json!([{"identity":"com.example.messages","major":1,
            "minMinor":0,"maxMinor":0,"ports":[{"name":"message.created","kind":"event"}]}]);
    }
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
}

async fn mount(host: &PackageHost, key: PackageInstanceKey) -> UiMountId {
    let result = host
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount {
            instance: key,
        }))
        .await
        .unwrap();
    let PackageHostOutcome::UiMountOpened { mount, .. } = result else {
        panic!("{result:?}")
    };
    mount
}

async fn request(
    host: &PackageHost,
    mount: &UiMountId,
    id: &str,
    operation: &str,
    payload: Value,
) -> Value {
    let result = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: mount.clone(),
            request: UiRequestId::new(format!(
                "{id}-{}",
                NEXT_REQUEST.fetch_add(1, Ordering::Relaxed)
            ))
            .unwrap(),
            operation: operation.into(),
            payload: serde_json::from_value(payload).unwrap(),
        }))
        .await
        .unwrap_or_else(|error| panic!("{operation} {id}: {error:?}"));
    let PackageHostOutcome::UiResponse { payload } = result else {
        panic!("{result:?}")
    };
    serde_json::to_value(payload).unwrap()
}

async fn events(host: &PackageHost, mount: &UiMountId, id: usize) -> Value {
    request(
        host,
        mount,
        &format!("events-{id}"),
        "fixture.port_event_events",
        json!({}),
    )
    .await
}

async fn stats(host: &PackageHost, mount: &UiMountId, id: usize) -> Value {
    request(
        host,
        mount,
        &format!("stats-{id}"),
        "fixture.port_event_stats",
        json!({}),
    )
    .await
}

/// Stream opens run independently of the Consumer reader. Wait for each
/// observable Host registration before emitting a live-only Provider fact.
async fn wait_subscribed(
    observations: &mut teamarness_package_host::PortObservations,
    consumer: &str,
) {
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let event = observations.recv().await.expect("Port observation");
            if event.stage == teamarness_package_host::PortStage::Subscribed
                && event.consumer_package == consumer
            {
                break;
            }
        }
    })
    .await
    .expect("Consumer stream registered before Provider emission");
}

// Verifies: TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test]
async fn two_consumers_share_one_upstream_and_keep_order_without_replay() {
    let fixture = common::Fixture::node_with_capabilities(
        &[
            ("provider", "com.example.provider"),
            ("consumer-a", "com.example.consumer-a"),
            ("consumer-b", "com.example.consumer-b"),
        ],
        &[json!({"identity":"teamarness.package-ports","major":2,"minMinor":0,"maxMinor":0})],
    )
    .await;
    for (directory, provider) in [
        ("provider", true),
        ("consumer-a", false),
        ("consumer-b", false),
    ] {
        declare(&fixture, directory, provider);
    }
    let dormant_manifest = fixture
        .config
        .application_root()
        .join("consumer-b/teamarness.package.json");
    let mut dormant: Value = serde_json::from_slice(&fs::read(&dormant_manifest).unwrap()).unwrap();
    dormant["activation"] = "on_demand".into();
    fs::write(dormant_manifest, serde_json::to_vec(&dormant).unwrap()).unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let snapshot = host.snapshot().await;
    assert_eq!(
        snapshot
            .instances
            .iter()
            .find(|instance| instance.key.package_identity.as_str() == "com.example.provider")
            .unwrap()
            .lifecycle,
        LifecycleState::Admitted
    );
    let key = |name: &str| {
        snapshot
            .instances
            .iter()
            .find(|instance| instance.key.package_identity.as_str() == name)
            .unwrap()
            .key
            .clone()
    };
    let a = mount(&host, key("com.example.consumer-a")).await;
    let mut observations = host.observe_ports();
    assert_eq!(
        request(&host, &a, "open-a", "fixture.port_event_open", json!({})).await["opened"],
        true
    );
    common::eventually(
        Duration::from_secs(4),
        "subscription activates on-demand Provider",
        || {
            let host = &host;
            async move {
                host.snapshot().await.instances.iter().any(|instance| {
                    instance.key.package_identity.as_str() == "com.example.provider"
                        && instance.lifecycle == LifecycleState::Active
                })
            }
        },
    )
    .await;
    let p = mount(&host, key("com.example.provider")).await;
    common::eventually(Duration::from_secs(4), "one Provider event stream", || {
        let host = &host;
        let p = p.clone();
        async move { stats(host, &p, 1).await["active"] == 1 }
    })
    .await;
    request(
        &host,
        &p,
        "emit-before-b",
        "fixture.port_event_emit",
        json!({"order":1}),
    )
    .await;
    common::eventually(
        Duration::from_secs(4),
        "first Consumer receives live event",
        || {
            let host = &host;
            let a = a.clone();
            async move {
                events(host, &a, 1).await["events"]
                    .as_array()
                    .unwrap()
                    .len()
                    == 1
            }
        },
    )
    .await;
    assert_eq!(
        host.snapshot()
            .await
            .instances
            .iter()
            .find(|instance| instance.key.package_identity.as_str() == "com.example.consumer-b")
            .unwrap()
            .lifecycle,
        LifecycleState::Admitted,
        "an event never wakes a dormant Consumer"
    );
    let b = mount(&host, key("com.example.consumer-b")).await;
    let mut readiness = host.observe_ports();
    assert_eq!(
        request(&host, &b, "open-b", "fixture.port_event_open", json!({})).await["opened"],
        true
    );
    wait_subscribed(&mut readiness, "com.example.consumer-b").await;
    assert_eq!(stats(&host, &p, 2).await["opens"], 1);
    for order in 2..=3 {
        assert_eq!(
            request(
                &host,
                &p,
                &format!("emit-{order}"),
                "fixture.port_event_emit",
                json!({"order":order})
            )
            .await["emitted"],
            1
        );
        common::eventually(
            Duration::from_secs(4),
            "both subscribers receive each event",
            || {
                let host = &host;
                let a = a.clone();
                let b = b.clone();
                async move {
                    events(host, &a, order * 2).await["events"]
                        .as_array()
                        .unwrap()
                        .len()
                        == order
                        && events(host, &b, order * 2 + 1).await["events"]
                            .as_array()
                            .unwrap()
                            .len()
                            == order - 1
                }
            },
        )
        .await;
    }
    assert_eq!(
        events(&host, &a, 100).await["events"],
        json!([{"order":1},{"order":2},{"order":3}])
    );
    assert_eq!(
        events(&host, &b, 101).await["events"],
        json!([{"order":2},{"order":3}])
    );
    let trace: Vec<_> = std::iter::from_fn(|| observations.try_recv().ok()).collect();
    let subscribers: Vec<_> = trace
        .iter()
        .filter(|event| event.stage == teamarness_package_host::PortStage::Subscribed)
        .collect();
    assert_eq!(subscribers.len(), 2);
    assert_eq!(
        subscribers[0].provider_id, subscribers[1].provider_id,
        "one upstream must map to distinct Consumer streams"
    );
    assert_ne!(
        subscribers[0].consumer_package,
        subscribers[1].consumer_package
    );
    // p: IDs are process-scoped, so these two streams may have the same local counter.
    assert!(subscribers.iter().all(|event| {
        event.consumer_id.starts_with("p:")
            && event
                .provider_id
                .as_deref()
                .unwrap()
                .starts_with("h:port-event-")
    }));
    assert!(trace.iter().any(
        |event| event.stage == teamarness_package_host::PortStage::Event
            && event.provider_id == subscribers[0].provider_id
    ));
    request(&host, &a, "close-a", "fixture.port_event_close", json!({})).await;
    assert_eq!(stats(&host, &p, 3).await["active"], 1);
    request(&host, &b, "close-b", "fixture.port_event_close", json!({})).await;
    common::eventually(
        Duration::from_secs(4),
        "last subscriber closes upstream",
        || {
            let host = &host;
            let p = p.clone();
            async move { stats(host, &p, 4).await["active"] == 0 }
        },
    )
    .await;
    let closes: Vec<_> = std::iter::from_fn(|| observations.try_recv().ok()).collect();
    assert!(closes.iter().any(
        |event| event.stage == teamarness_package_host::PortStage::Closed
            && event.provider_id == subscribers[0].provider_id
    ));
    request(&host, &a, "open-late", "fixture.port_event_open", json!({})).await;
    common::eventually(
        Duration::from_secs(4),
        "new upstream after reconnect",
        || {
            let host = &host;
            let p = p.clone();
            async move { stats(host, &p, 5).await["opens"] == 2 }
        },
    )
    .await;
    assert_eq!(events(&host, &a, 102).await["events"], json!([]));
    assert_eq!(
        request(
            &host,
            &a,
            "health",
            "fixture.capability",
            json!({
                "capability":"teamarness.package-ports","method":"available","arguments":{"access":"declared"}
            })
        )
        .await["result"]["contracts"][0]["available"],
        true
    );
}

// Verifies: TEAM.SYS.PKG-R14
#[tokio::test]
async fn invalid_event_ends_only_subscriptions_and_sessions_stay_usable() {
    let fixture = common::Fixture::node_with_capabilities(
        &[
            ("provider", "com.example.provider"),
            ("consumer-a", "com.example.consumer-a"),
            ("consumer-b", "com.example.consumer-b"),
        ],
        &[json!({"identity":"teamarness.package-ports","major":2,"minMinor":0,"maxMinor":0})],
    )
    .await;
    for (directory, provider) in [
        ("provider", true),
        ("consumer-a", false),
        ("consumer-b", false),
    ] {
        declare(&fixture, directory, provider);
    }
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let snapshot = host.snapshot().await;
    let key = |name: &str| {
        snapshot
            .instances
            .iter()
            .find(|instance| instance.key.package_identity.as_str() == name)
            .unwrap()
            .key
            .clone()
    };
    let a = mount(&host, key("com.example.consumer-a")).await;
    let b = mount(&host, key("com.example.consumer-b")).await;
    request(&host, &a, "open-a", "fixture.port_event_open", json!({})).await;
    let mut readiness = host.observe_ports();
    request(&host, &b, "open-b", "fixture.port_event_open", json!({})).await;
    wait_subscribed(&mut readiness, "com.example.consumer-b").await;
    common::eventually(Duration::from_secs(4), "Provider active", || {
        let host = &host;
        async move {
            host.snapshot().await.instances.iter().any(|i| {
                i.key.package_identity.as_str() == "com.example.provider"
                    && i.lifecycle == LifecycleState::Active
            })
        }
    })
    .await;
    let p = mount(&host, key("com.example.provider")).await;
    common::eventually(Duration::from_secs(4), "upstream opened", || {
        let host = &host;
        let p = p.clone();
        async move { stats(host, &p, 10).await["active"] == 1 }
    })
    .await;
    request(
        &host,
        &p,
        "emit-good",
        "fixture.port_event_emit",
        json!({"order":1}),
    )
    .await;
    common::eventually(Duration::from_secs(4), "valid events queued", || {
        let host = &host;
        let a = a.clone();
        let b = b.clone();
        async move {
            events(host, &a, 11).await["events"]
                .as_array()
                .unwrap()
                .len()
                == 1
                && events(host, &b, 12).await["events"]
                    .as_array()
                    .unwrap()
                    .len()
                    == 1
        }
    })
    .await;
    request(
        &host,
        &p,
        "emit-invalid",
        "fixture.port_event_emit",
        json!({"invalid":true}),
    )
    .await;
    common::eventually(
        Duration::from_secs(4),
        "both affected streams end after accepted event",
        || {
            let host = &host;
            let a = a.clone();
            let b = b.clone();
            async move {
                events(host, &a, 13).await["closeCode"] == "port_event_invalid"
                    && events(host, &b, 14).await["closeCode"] == "port_event_invalid"
            }
        },
    )
    .await;
    assert_eq!(events(&host, &a, 15).await["events"], json!([{"order":1}]));
    assert_eq!(
        request(
            &host,
            &b,
            "health",
            "fixture.capability",
            json!({"capability":"teamarness.package-ports","method":"available","arguments":{"access":"declared"}})
        )
        .await["result"]["contracts"][0]["available"],
        true
    );
}

// Verifies: TEAM.SYS.PKG-R14
#[tokio::test]
async fn slow_subscriber_overflows_after_accepted_events_while_peer_keeps_progressing() {
    let fixture = common::Fixture::node_with_capabilities(
        &[
            ("provider", "com.example.provider"),
            ("consumer-a", "com.example.consumer-a"),
            ("consumer-b", "com.example.consumer-b"),
        ],
        &[json!({"identity":"teamarness.package-ports","major":2,"minMinor":0,"maxMinor":0})],
    )
    .await;
    for (directory, provider) in [
        ("provider", true),
        ("consumer-a", false),
        ("consumer-b", false),
    ] {
        declare(&fixture, directory, provider);
    }
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let snapshot = host.snapshot().await;
    let key = |name: &str| {
        snapshot
            .instances
            .iter()
            .find(|i| i.key.package_identity.as_str() == name)
            .unwrap()
            .key
            .clone()
    };
    let a = mount(&host, key("com.example.consumer-a")).await;
    let b = mount(&host, key("com.example.consumer-b")).await;
    request(&host, &a, "open-a", "fixture.port_event_open", json!({})).await;
    let mut readiness = host.observe_ports();
    request(&host, &b, "open-b", "fixture.port_event_open", json!({})).await;
    wait_subscribed(&mut readiness, "com.example.consumer-b").await;
    common::eventually(Duration::from_secs(4), "Provider active", || {
        let host = &host;
        async move {
            host.snapshot().await.instances.iter().any(|i| {
                i.key.package_identity.as_str() == "com.example.provider"
                    && i.lifecycle == LifecycleState::Active
            })
        }
    })
    .await;
    let p = mount(&host, key("com.example.provider")).await;
    common::eventually(Duration::from_secs(4), "one upstream", || {
        let host = &host;
        let p = p.clone();
        async move { stats(host, &p, 0).await["active"] == 1 }
    })
    .await;
    for order in 1..=40_usize {
        assert_eq!(
            request(
                &host,
                &p,
                &format!("emit-{order}"),
                "fixture.port_event_emit",
                json!({"order":order})
            )
            .await["emitted"],
            1
        );
        common::eventually(
            Duration::from_secs(4),
            "peer receives ordered live event",
            || {
                let host = &host;
                let b = b.clone();
                async move {
                    events(host, &b, order).await["events"]
                        .as_array()
                        .unwrap()
                        .len()
                        == order
                }
            },
        )
        .await;
        if order >= 32 {
            request(
                &host,
                &b,
                &format!("credit-{order}"),
                "fixture.port_event_credit",
                json!({"amount":1}),
            )
            .await;
        }
    }
    request(
        &host,
        &a,
        "release-a",
        "fixture.port_event_credit",
        json!({"amount":4}),
    )
    .await;
    common::eventually(
        Duration::from_secs(4),
        "slow stream terminates locally after its queued events",
        || {
            let host = &host;
            let a = a.clone();
            async move { events(host, &a, 100).await["closeCode"] == "port_subscriber_overflow" }
        },
    )
    .await;
    let slow = events(&host, &a, 101).await;
    let fast = events(&host, &b, 102).await;
    assert!(slow["events"].as_array().unwrap().len() >= 32, "{slow}");
    assert!(slow["events"].as_array().unwrap().len() < 40, "{slow}");
    let expected = (1..=40)
        .map(|order| json!({"order":order}))
        .collect::<Vec<_>>();
    assert_eq!(fast["events"], json!(expected));
    assert_eq!(fast["closeCode"], Value::Null);
    assert_eq!(stats(&host, &p, 103).await["active"], 1);
}

// Verifies: TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test]
async fn invalid_event_grant_and_churn_do_not_start_or_strand_upstreams() {
    let fixture = common::Fixture::node_with_capabilities(
        &[
            ("provider", "com.example.provider"),
            ("consumer-a", "com.example.consumer-a"),
            ("consumer-b", "com.example.consumer-b"),
        ],
        &[json!({"identity":"teamarness.package-ports","major":2,"minMinor":0,"maxMinor":0})],
    )
    .await;
    for (directory, provider) in [
        ("provider", true),
        ("consumer-a", false),
        ("consumer-b", false),
    ] {
        declare(&fixture, directory, provider);
    }
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let snapshot = host.snapshot().await;
    let key = |name: &str| {
        snapshot
            .instances
            .iter()
            .find(|i| i.key.package_identity.as_str() == name)
            .unwrap()
            .key
            .clone()
    };
    let a = mount(&host, key("com.example.consumer-a")).await;
    let b = mount(&host, key("com.example.consumer-b")).await;
    request(
        &host,
        &a,
        "wrong-port",
        "fixture.port_event_open",
        json!({"port":"message.secret"}),
    )
    .await;
    common::eventually(Duration::from_secs(3), "wrong port refused", || {
        let host = &host;
        let a = a.clone();
        async move { events(host, &a, 1).await["closeCode"] == "ungranted_port" }
    })
    .await;
    assert_eq!(
        host.snapshot()
            .await
            .instances
            .iter()
            .find(|i| i.key.package_identity.as_str() == "com.example.provider")
            .unwrap()
            .lifecycle,
        LifecycleState::Admitted
    );
    request(
        &host,
        &a,
        "forged",
        "fixture.port_event_open",
        json!({"extra":{"workspaceId":"forged"}}),
    )
    .await;
    common::eventually(Duration::from_secs(3), "forged envelope refused", || {
        let host = &host;
        let a = a.clone();
        async move { events(host, &a, 2).await["closeCode"] == "capability_request_invalid" }
    })
    .await;
    for iteration in 0..8 {
        let open_a = format!("open-a-{iteration}");
        let open_b = format!("open-b-{iteration}");
        let (one, two) = tokio::join!(
            request(&host, &a, &open_a, "fixture.port_event_open", json!({})),
            request(&host, &b, &open_b, "fixture.port_event_open", json!({})),
        );
        assert_eq!(one["opened"], true);
        assert_eq!(two["opened"], true);
        common::eventually(Duration::from_secs(4), "Provider active", || {
            let host = &host;
            async move {
                host.snapshot().await.instances.iter().any(|i| {
                    i.key.package_identity.as_str() == "com.example.provider"
                        && i.lifecycle == LifecycleState::Active
                })
            }
        })
        .await;
        let p = mount(&host, key("com.example.provider")).await;
        common::eventually(
            Duration::from_secs(4),
            "one upstream for both subscribers",
            || {
                let host = &host;
                let p = p.clone();
                async move { stats(host, &p, iteration * 3).await["active"] == 1 }
            },
        )
        .await;
        assert_eq!(
            stats(&host, &p, iteration * 3 + 1).await["opens"],
            iteration + 1
        );
        let close_a = format!("close-a-{iteration}");
        let close_b = format!("close-b-{iteration}");
        let _ = tokio::join!(
            request(&host, &a, &close_a, "fixture.port_event_close", json!({})),
            request(&host, &b, &close_b, "fixture.port_event_close", json!({})),
        );
        if iteration == 7 {
            common::eventually(
                Duration::from_secs(4),
                "upstream released after last close",
                || {
                    let host = &host;
                    let p = p.clone();
                    async move { stats(host, &p, iteration * 3 + 2).await["active"] == 0 }
                },
            )
            .await;
        }
        // Earlier iterations reconnect immediately while the previous close
        // may still be in flight; no second upstream may overlap it.
    }
}

// Verifies: TEAM.SYS.PKG-R14
#[tokio::test]
async fn replacing_provider_generation_revokes_streams_without_replay() {
    let fixture = common::Fixture::node_with_capabilities(
        &[
            ("provider", "com.example.provider"),
            ("consumer-a", "com.example.consumer-a"),
            ("consumer-b", "com.example.consumer-b"),
        ],
        &[json!({"identity":"teamarness.package-ports","major":2,"minMinor":0,"maxMinor":0})],
    )
    .await;
    for (directory, provider) in [
        ("provider", true),
        ("consumer-a", false),
        ("consumer-b", false),
    ] {
        declare(&fixture, directory, provider);
    }
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let snapshot = host.snapshot().await;
    let key = |name: &str| {
        snapshot
            .instances
            .iter()
            .find(|i| i.key.package_identity.as_str() == name)
            .unwrap()
            .key
            .clone()
    };
    let a = mount(&host, key("com.example.consumer-a")).await;
    let b = mount(&host, key("com.example.consumer-b")).await;
    request(&host, &a, "open-a", "fixture.port_event_open", json!({})).await;
    let mut readiness = host.observe_ports();
    request(&host, &b, "open-b", "fixture.port_event_open", json!({})).await;
    wait_subscribed(&mut readiness, "com.example.consumer-b").await;
    common::eventually(Duration::from_secs(4), "Provider active", || {
        let host = &host;
        async move {
            host.snapshot().await.instances.iter().any(|i| {
                i.key.package_identity.as_str() == "com.example.provider"
                    && i.lifecycle == LifecycleState::Active
            })
        }
    })
    .await;
    let p = mount(&host, key("com.example.provider")).await;
    common::eventually(Duration::from_secs(4), "upstream open", || {
        let host = &host;
        let p = p.clone();
        async move { stats(host, &p, 1).await["active"] == 1 }
    })
    .await;
    request(
        &host,
        &p,
        "emit-first",
        "fixture.port_event_emit",
        json!({"order":1}),
    )
    .await;
    common::eventually(
        Duration::from_secs(4),
        "live event before replacement",
        || {
            let host = &host;
            let a = a.clone();
            let b = b.clone();
            async move {
                events(host, &a, 2).await["events"]
                    .as_array()
                    .unwrap()
                    .len()
                    == 1
                    && events(host, &b, 3).await["events"]
                        .as_array()
                        .unwrap()
                        .len()
                        == 1
            }
        },
    )
    .await;
    let _ = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: p,
            request: UiRequestId::new("crash-provider").unwrap(),
            operation: "fixture.abort".into(),
            payload: serde_json::from_value(json!({})).unwrap(),
        }))
        .await;
    common::eventually(
        Duration::from_secs(8),
        "Provider replacement generation",
        || {
            let host = &host;
            async move {
                host.snapshot().await.instances.iter().any(|i| {
                    i.key.package_identity.as_str() == "com.example.provider"
                        && i.lifecycle == LifecycleState::Active
                        && i.process_generation.unwrap_or(0) >= 2
                })
            }
        },
    )
    .await;
    common::eventually(Duration::from_secs(4), "both old streams revoked", || {
        let host = &host;
        let a = a.clone();
        let b = b.clone();
        async move {
            events(host, &a, 4).await["closeCode"] == "provider_exited"
                && events(host, &b, 5).await["closeCode"] == "provider_exited"
        }
    })
    .await;
    request(&host, &a, "reconnect", "fixture.port_event_open", json!({})).await;
    let p = mount(&host, key("com.example.provider")).await;
    common::eventually(Duration::from_secs(4), "new Provider upstream", || {
        let host = &host;
        let p = p.clone();
        async move { stats(host, &p, 6).await["active"] == 1 }
    })
    .await;
    assert_eq!(events(&host, &a, 7).await["events"], json!([]));
    request(
        &host,
        &p,
        "emit-second",
        "fixture.port_event_emit",
        json!({"order":2}),
    )
    .await;
    common::eventually(Duration::from_secs(4), "new event without replay", || {
        let host = &host;
        let a = a.clone();
        async move { events(host, &a, 8).await["events"] == json!([{"order":2}]) }
    })
    .await;
    assert_eq!(events(&host, &b, 9).await["events"], json!([{"order":1}]));
}

// Verifies: TEAM.SYS.PKG-R14
#[tokio::test]
async fn largest_schema_accepted_event_fits_bounded_process_and_fanout_queues() {
    let fixture = common::Fixture::node_with_capabilities(
        &[
            ("provider", "com.example.provider"),
            ("consumer", "com.example.consumer"),
        ],
        &[json!({"identity":"teamarness.package-ports","major":2,"minMinor":0,"maxMinor":0})],
    )
    .await;
    declare(&fixture, "provider", true);
    declare(&fixture, "consumer", false);
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let snapshot = host.snapshot().await;
    let key = |name: &str| {
        snapshot
            .instances
            .iter()
            .find(|i| i.key.package_identity.as_str() == name)
            .unwrap()
            .key
            .clone()
    };
    let consumer = mount(&host, key("com.example.consumer")).await;
    request(
        &host,
        &consumer,
        "open",
        "fixture.port_event_open",
        json!({}),
    )
    .await;
    common::eventually(Duration::from_secs(4), "Provider active", || {
        let host = &host;
        async move {
            host.snapshot().await.instances.iter().any(|i| {
                i.key.package_identity.as_str() == "com.example.provider"
                    && i.lifecycle == LifecycleState::Active
            })
        }
    })
    .await;
    let provider = mount(&host, key("com.example.provider")).await;
    common::eventually(Duration::from_secs(4), "upstream opened", || {
        let host = &host;
        let provider = provider.clone();
        async move { stats(host, &provider, 1).await["active"] == 1 }
    })
    .await;
    // The Port schema's UI-profile maximum is 256 KiB; this stays just under
    // that limit while exercising the process frame and three queue ledgers.
    let padding = "x".repeat(256 * 1024 - 200);
    for order in 1..=2 {
        assert_eq!(
            request(
                &host,
                &provider,
                &format!("large-{order}"),
                "fixture.port_event_emit",
                json!({"order":order,"padding":padding})
            )
            .await["emitted"],
            1
        );
        common::eventually(Duration::from_secs(4), "maximum payload accepted", || {
            let host = &host;
            let consumer = consumer.clone();
            async move {
                request(
                    host,
                    &consumer,
                    &format!("count-{order}"),
                    "fixture.port_event_events",
                    json!({"summary":true}),
                )
                .await["count"]
                    == order
            }
        })
        .await;
    }
    let received = request(
        &host,
        &consumer,
        "large-count",
        "fixture.port_event_events",
        json!({"summary":true}),
    )
    .await;
    assert_eq!(received["closeCode"], Value::Null);
    assert_eq!(received["paddingLength"], padding.len());
    assert_eq!(received["lastOrder"], 2);
}
