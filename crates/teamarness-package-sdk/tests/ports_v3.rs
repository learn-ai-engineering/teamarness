use serde_json::{Value, json};
use teamarness_package_sdk::{
    Empty, Router,
    ports_v3::{PortAccessV3, PortClientV3, PortContractRef},
    testing::{HarnessBuilder, ScriptedCapability},
};

const CONTRACT: &str = "dev.example.metadata";
const DIGEST: &str = "0de9112f997e9a8350b7bef09d9331815451382033e418f3954c614a42fdbbc7";
const REVISION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
fn golden() -> Value {
    serde_json::from_str(include_str!(
        "../../../contracts/package-capabilities/v1/package-ports-3.0.golden.json"
    ))
    .unwrap()
}

async fn exercise(
    major: u16,
    reply: Option<Value>,
) -> Result<Value, teamarness_package_sdk::BackendError> {
    exercise_reply(major, "available", reply).await
}

async fn exercise_reply(
    major: u16,
    reply_method: &'static str,
    reply: Option<Value>,
) -> Result<Value, teamarness_package_sdk::BackendError> {
    let scripted = ScriptedCapability::new().calls(move |method, payload| {
        if let Some(reply) = &reply && method == reply_method { return Ok(reply.clone()); }
        assert_eq!(payload["access"], "person");
        match method {
            "available" => { assert_eq!(payload, json!({"access":"person"})); Ok(golden()["availableResult"].clone()) },
            "describe" => { assert_eq!(payload, json!({"access":"person","contract":CONTRACT,"major":1,"metadataSha256":DIGEST,"offset":0,"routeRevision":REVISION})); Ok(golden()["describeResult"].clone()) },
            "enable" => { assert_eq!(payload, json!({"access":"person","contract":CONTRACT,"major":1,"routeRevision":REVISION})); Ok(json!({"access":"person","contract":CONTRACT,"major":1,"enabled":true})) },
            "disable" => { assert_eq!(payload, json!({"access":"person","contract":CONTRACT})); Ok(json!({"access":"person","contract":CONTRACT,"enabled":false})) },
            "invoke" => { assert_eq!(payload, json!({"access":"person","contract":CONTRACT,"major":1,"routeRevision":REVISION,"port":"value.read","kind":"query","request":{}})); Ok(json!({"$teamarnessPortEnvelope":1,"$teamarnessPortValue":[null,42]})) },
            _ => panic!("unexpected {method}"),
        }
    });
    let router = Router::stateless().request("fixture.read", |context, _: Empty| async move {
        let client = PortClientV3::from_initialization(
            context.initialization(),
            context.capabilities().clone(),
        )?;
        let page = client.available(PortAccessV3::Person, None).await?;
        assert_eq!(
            page.contracts[0].provider.as_ref().unwrap().identity,
            "dev.example.provider"
        );
        let route = PortContractRef {
            access: PortAccessV3::Person,
            contract: CONTRACT,
            major: 1,
            route_revision: Some(REVISION),
        };
        let metadata = client.describe(route, DIGEST).await?;
        assert_eq!(metadata.definitions["text"], json!({"type":"string"}));
        client.enable(route).await?;
        let value = client.query(route, "value.read", json!({})).await?;
        client.disable(PortAccessV3::Person, CONTRACT).await?;
        Ok(json!({"value":value}))
    });
    let directory = tempfile::tempdir().unwrap();
    let mut harness = HarnessBuilder::new(router.into_backend())
        .data_directory(directory.path())
        .capability_version("teamarness.package-ports", scripted, major, 0)
        .start()
        .await;
    harness.activate().await.unwrap();
    let result = harness.request("fixture.read", json!({})).await;
    harness.shutdown().await.unwrap();
    result
}

// Verifies: TEAM.SYS.PKG-R03, TEAM.SYS.PKG-R12
#[tokio::test]
async fn person_client_rejects_corrupt_chunks_and_cross_scope_selection_acknowledgements() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let chunk = golden()["describeResult"].clone();
    for (key, value) in [
        ("data", json!("!!==")),
        ("offset", json!(1)),
        ("totalBytes", json!(1024 * 1024 + 1)),
        ("metadataSha256", json!("0".repeat(64))),
        ("nextOffset", Value::Null),
        ("nextOffset", json!(0)),
        ("extra", json!(true)),
        ("data", json!(STANDARD.encode(vec![b' '; 64 * 1024 + 1]))),
    ] {
        let mut bad = chunk.clone();
        bad[key] = value;
        assert_eq!(
            exercise_reply(3, "describe", Some(bad))
                .await
                .unwrap_err()
                .code(),
            "package.invalid_capability_response"
        );
    }
    for response in [
        json!({"access":"person","contract":CONTRACT,"major":1,"enabled":false}),
        json!({"access":"agent","contract":CONTRACT,"major":1,"enabled":true}),
        json!({"access":"person","contract":CONTRACT,"major":2,"enabled":true}),
        json!({"access":"person","contract":"dev.example.other","major":1,"enabled":true}),
    ] {
        assert_eq!(
            exercise_reply(3, "enable", Some(response))
                .await
                .unwrap_err()
                .code(),
            "package.invalid_capability_response"
        );
    }
}

// Verifies: TEAM.SYS.PKG-R03
#[tokio::test]
async fn person_client_without_a_grant_fails_before_any_host_call() {
    let router = Router::stateless().request("fixture.missing", |context, _: Empty| async move {
        let error = PortClientV3::from_initialization(
            context.initialization(),
            context.capabilities().clone(),
        )
        .err()
        .expect("no grant");
        Ok(json!({"code":error.code()}))
    });
    let directory = tempfile::tempdir().unwrap();
    let mut harness = HarnessBuilder::new(router.into_backend())
        .data_directory(directory.path())
        .start()
        .await;
    harness.activate().await.unwrap();
    assert_eq!(
        harness.request("fixture.missing", json!({})).await.unwrap(),
        json!({"code":"package.capability_not_granted"})
    );
    harness.shutdown().await.unwrap();
}

// Verifies: TEAM.SYS.PKG-R03, TEAM.SYS.PKG-R12, TEAM.SYS.PKG-R13
#[tokio::test]
async fn person_client_requires_major_three_and_preserves_complete_metadata_and_values() {
    assert_eq!(
        exercise(3, None).await.unwrap(),
        json!({"value":[null, 42]})
    );
    assert_eq!(
        exercise(2, None).await.unwrap_err().code(),
        "package.capability_version_unsupported"
    );
    assert_eq!(
        exercise(1, None).await.unwrap_err().code(),
        "package.capability_version_unsupported"
    );
}

// Verifies: TEAM.SYS.PKG-R03, TEAM.SYS.PKG-R12
#[tokio::test]
async fn person_client_rejects_missing_extra_and_malformed_catalog_fields() {
    let row = golden()["availableResult"]["contracts"][0].clone();
    for field in [
        "provider",
        "routeRevision",
        "guideEligibility",
        "description",
        "enabledMajor",
    ] {
        let mut bad = row.clone();
        bad.as_object_mut().unwrap().remove(field);
        assert_eq!(
            exercise(3, Some(json!({"contracts":[bad]})))
                .await
                .unwrap_err()
                .code(),
            "package.invalid_capability_response"
        );
    }
    for (key, value) in [
        ("routeRevision", json!("A".repeat(64))),
        ("guideEligibility", json!("future")),
        ("provider", json!({})),
        (
            "provider",
            json!({"identity":"dev.example.provider","version":"xxxxx","displayName":"Example Provider","description":null}),
        ),
        ("extra", json!(true)),
    ] {
        let mut bad = row.clone();
        bad[key] = value;
        assert_eq!(
            exercise(3, Some(json!({"contracts":[bad]})))
                .await
                .unwrap_err()
                .code(),
            "package.invalid_capability_response"
        );
    }
    for reply in [
        json!({"contracts":vec![row;33]}),
        json!({"contracts":[],"nextCursor":"cursor"}),
    ] {
        assert_eq!(
            exercise(3, Some(reply)).await.unwrap_err().code(),
            "package.invalid_capability_response"
        );
    }
}

// Verifies: TEAM.SYS.PKG-R04, TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test]
async fn confirmed_port_methods_preserve_shape_values_and_selected_version_refusal() {
    for minor in [0, 1] {
        for query in [false, true] {
            let scripted = ScriptedCapability::new();
            let router =
                Router::stateless().request("fixture.open", move |context, _: Empty| async move {
                    assert_eq!(context.initialization().protocol().major, 1);
                    let client = PortClientV3::from_initialization(
                        context.initialization(),
                        context.capabilities().clone(),
                    )?;
                    let route = PortContractRef {
                        access: PortAccessV3::Person,
                        contract: CONTRACT,
                        major: 1,
                        route_revision: Some(REVISION),
                    };
                    let mut subscription = if query {
                        client
                            .live_query_confirmed(route, "value.watch", json!([null, 42]))
                            .await?
                    } else {
                        client
                            .subscribe_event_confirmed(route, "value.changed")
                            .await?
                    };
                    let value = subscription.recv().await.unwrap()?;
                    subscription.close().await;
                    Ok(json!({"value":value}))
                });
            let directory = tempfile::tempdir().unwrap();
            let mut harness = HarnessBuilder::new(router.into_backend())
                .process_minor(minor)
                .data_directory(directory.path())
                .capability_version("teamarness.package-ports", scripted.clone(), 3, 0)
                .start()
                .await;
            harness.activate().await.unwrap();
            let pending = harness.start_request("fixture.open", json!({})).await;
            if minor == 0 {
                assert_eq!(
                    pending.result().await.unwrap_err().code(),
                    "capability_version_unsupported"
                );
                assert!(scripted.recorded_opens().is_empty());
            } else {
                tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    scripted.stream().opened(),
                )
                .await
                .unwrap();
                let mut expected = json!({"access":"person","contract":CONTRACT,"major":1,"routeRevision":REVISION,"port":if query {"value.watch"} else {"value.changed"},"kind":if query {"query"} else {"event"}});
                if query {
                    expected["request"] = json!([null, 42]);
                }
                let mut actual = scripted.recorded_opens()[0].clone();
                assert!(actual.as_object_mut().unwrap().remove("grant").is_some());
                assert_eq!(actual, expected);
                let value = json!({"$teamarnessPortEnvelope":1,"$teamarnessPortValue":null});
                assert!(
                    scripted
                        .stream()
                        .send(json!({"$teamarnessPortEnvelope":1,"$teamarnessPortValue":value}))
                        .await
                );
                assert_eq!(pending.result().await.unwrap(), json!({"value":value}));
            }
            harness.shutdown().await.unwrap();
        }
    }
}
