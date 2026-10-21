mod common;

use std::collections::BTreeMap;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use teamarness_package_host::{
    OpenUiMount, PackageErrorCode, PackageHost, PackageHostCommand, PackageHostOutcome,
    PackageValue, ReconcileWorkspaces, UiRequest, UiRequestId,
};

fn object(entries: impl IntoIterator<Item = (&'static str, PackageValue)>) -> PackageValue {
    PackageValue::Object(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

async fn call(
    host: &PackageHost,
    mount: teamarness_package_host::UiMountId,
    request: &str,
    payload: PackageValue,
) -> BTreeMap<String, PackageValue> {
    let outcome = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount,
            request: UiRequestId::new(request).unwrap(),
            operation: "fixture.capability".into(),
            payload,
        }))
        .await
        .unwrap();
    let PackageHostOutcome::UiResponse {
        payload: PackageValue::Object(mut response),
    } = outcome
    else {
        panic!("unexpected capability response")
    };
    match response.remove("result") {
        Some(PackageValue::Object(result)) => result,
        Some(_) => panic!("capability result was not an object"),
        None => response,
    }
}

async fn grant(
    host: &PackageHost,
    mount: teamarness_package_host::UiMountId,
    request: &str,
    capability: &str,
) -> PackageValue {
    match host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount,
            request: UiRequestId::new(request).unwrap(),
            operation: "fixture.grant".into(),
            payload: object([("capability", PackageValue::String(capability.to_owned()))]),
        }))
        .await
        .unwrap()
    {
        PackageHostOutcome::UiResponse {
            payload: PackageValue::Object(value),
        } => value["grant"].clone(),
        other => panic!("unexpected grant response: {other:?}"),
    }
}

async fn mount(host: &PackageHost, index: usize) -> teamarness_package_host::UiMountId {
    let instance = host.snapshot().await.instances[index].key.clone();
    match host
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount { instance }))
        .await
        .unwrap()
    {
        PackageHostOutcome::UiMountOpened { mount, .. } => mount,
        other => panic!("unexpected mount outcome: {other:?}"),
    }
}

fn capability(name: &str, method: &str, arguments: PackageValue) -> PackageValue {
    object([
        ("capability", PackageValue::String(name.into())),
        ("method", PackageValue::String(method.into())),
        ("arguments", arguments),
    ])
}

#[tokio::test]
async fn reserved_person_name_package_cannot_be_admitted_or_granted() {
    let fixture = common::Fixture::node_with_capabilities(
        &[
            ("reserved", "internal:teamarness.person-names"),
            ("previously-valid", "internal.teamarness.person-names"),
        ],
        &[serde_json::json!({"identity":"teamarness.immutable-record-log","major":1,"minMinor":0,"maxMinor":0})],
    )
    .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let snapshot = host.snapshot().await;
    assert_eq!(snapshot.packages.len(), 1);
    assert_eq!(
        snapshot.packages[0].identity.as_str(),
        "internal.teamarness.person-names"
    );
    assert_eq!(snapshot.instances.len(), 1);
    assert_eq!(
        snapshot.instances[0].key.package_identity.as_str(),
        "internal.teamarness.person-names"
    );
    assert!(
        snapshot
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == PackageErrorCode::InvalidPackageIdentity)
    );
}

#[tokio::test]
async fn person_name_capability_negotiates_only_supported_major_and_minor() {
    let requirements = [
        serde_json::json!({"identity":"teamarness.person-names","major":2,"minMinor":0,"maxMinor":0}),
    ];
    let fixture = common::Fixture::node_with_capabilities(
        &[("unsupported", "dev.teamarness.unsupported")],
        &requirements,
    )
    .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let snapshot = host.snapshot().await;
    assert!(
        snapshot
            .instances
            .iter()
            .all(|instance| instance.lifecycle == teamarness_package_host::LifecycleState::Failed)
    );
    assert!(
        snapshot
            .diagnostics
            .iter()
            .any(|entry| entry.code == PackageErrorCode::CapabilityVersionUnsupported)
    );
}

#[tokio::test]
async fn independent_fixture_reads_person_names_without_membership_authority() {
    let fixture = common::Fixture::node_with_capabilities(
        &[("fixture", "dev.teamarness.fixture")],
        &[serde_json::json!({"identity":"teamarness.person-names","major":1,"minMinor":0,"maxMinor":0})],
    ).await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let mount = mount(&host, 0).await;
    let result = call(
        &host,
        mount.clone(),
        "names",
        capability("teamarness.person-names", "current", object([])),
    )
    .await;
    assert_eq!(
        result.get("workspaceId"),
        Some(&PackageValue::String(
            fixture.core.list_workspaces().unwrap()[0].id().to_string()
        ))
    );
    assert!(
        matches!(result.get("persons"), Some(PackageValue::Array(values)) if values.len() == 1)
    );
    assert_eq!(
        result.get("cursor"),
        Some(&PackageValue::String("0".into()))
    );
    let identity = fixture.core.public_identity().to_string();
    let lookup = call(
        &host,
        mount.clone(),
        "lookup-name",
        capability(
            "teamarness.person-names",
            "lookup",
            object([("identity", PackageValue::String(identity.clone()))]),
        ),
    )
    .await;
    assert!(
        matches!(lookup.get("person"), Some(PackageValue::Object(person)) if person.get("identity") == Some(&PackageValue::String(identity)))
    );
    let invalid = call(
        &host,
        mount,
        "invalid-name-lookup",
        capability(
            "teamarness.person-names",
            "lookup",
            object([("identity", PackageValue::String("short".into()))]),
        ),
    )
    .await;
    assert_eq!(
        invalid.get("error"),
        Some(&PackageValue::String("capability_request_invalid".into()))
    );
}

#[tokio::test]
async fn independent_fixture_resumes_person_name_stream_after_rename() {
    let fixture = common::Fixture::node_with_capabilities(
        &[("fixture", "dev.teamarness.fixture")],
        &[serde_json::json!({"identity":"teamarness.person-names","major":1,"minMinor":0,"maxMinor":0})],
    ).await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let mount = mount(&host, 0).await;
    let workspace = fixture.core.list_workspaces().unwrap()[0].id();
    let first = fixture_request(
        &host,
        &mount,
        "names-open",
        "fixture.person_names_open",
        object([("afterCursor", PackageValue::String("0".into()))]),
    )
    .await;
    assert!(first.contains_key("streamId"));
    fixture.core.set_own_person_name(workspace, "👩‍💻").unwrap();
    let expiry = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let mut attempt = 0;
    let cursor = loop {
        attempt += 1;
        let result = fixture_request(
            &host,
            &mount,
            &format!("names-events-{attempt}"),
            "fixture.person_names_events",
            object([]),
        )
        .await;
        if let Some(PackageValue::Array(events)) = result.get("events")
            && let Some(PackageValue::Object(snapshot)) = events.last()
        {
            assert!(
                matches!(snapshot.get("persons"), Some(PackageValue::Array(persons)) if persons.iter().any(|person| matches!(person, PackageValue::Object(value) if value.get("name") == Some(&PackageValue::String("👩‍💻".into())))))
            );
            break snapshot.get("cursor").unwrap().clone();
        }
        assert!(
            std::time::Instant::now() < expiry,
            "renamed snapshot reaches unrelated fixture"
        );
        // poll-interval: bounded wait for an asynchronous stream delivery.
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    };
    fixture_request(
        &host,
        &mount,
        "names-close",
        "fixture.person_names_close",
        object([]),
    )
    .await;
    fixture_request(
        &host,
        &mount,
        "names-resume",
        "fixture.person_names_open",
        object([("afterCursor", cursor.clone())]),
    )
    .await;
    fixture
        .core
        .set_own_person_name(workspace, "Second")
        .unwrap();
    let expiry = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        attempt += 1;
        let result = fixture_request(
            &host,
            &mount,
            &format!("names-resumed-{attempt}"),
            "fixture.person_names_events",
            object([]),
        )
        .await;
        if matches!(result.get("events"), Some(PackageValue::Array(events)) if events.iter().any(|event| matches!(event, PackageValue::Object(value) if value.get("cursor") != Some(&cursor))))
        {
            break;
        }
        assert!(
            std::time::Instant::now() < expiry,
            "resumed stream receives next cursor"
        );
        // poll-interval: bounded wait for an asynchronous stream delivery.
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn routes_scoped_workspace_membership_and_signing_calls() {
    let requirements = [
        serde_json::json!({"identity":"teamarness.workspace-context","major":1,"minMinor":0,"maxMinor":0}),
        serde_json::json!({"identity":"teamarness.membership","major":1,"minMinor":0,"maxMinor":0}),
        serde_json::json!({"identity":"teamarness.signing","major":1,"minMinor":0,"maxMinor":0}),
    ];
    let fixture = common::Fixture::node_with_capabilities(
        &[("fixture", "dev.teamarness.fixture")],
        &requirements,
    )
    .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let mount = mount(&host, 0).await;

    let context = call(
        &host,
        mount.clone(),
        "context",
        capability("teamarness.workspace-context", "get", object([])),
    )
    .await;
    assert_eq!(
        context.get("workspaceName"),
        Some(&PackageValue::String("Fixture Workspace".into()))
    );
    assert_eq!(
        context.get("installationDisplayName"),
        Some(&PackageValue::String(fixture.core.display_name()))
    );

    let current = call(
        &host,
        mount.clone(),
        "current",
        capability("teamarness.membership", "current", object([])),
    )
    .await;
    assert_eq!(
        current.get("epoch"),
        Some(&PackageValue::String("0".into()))
    );
    assert!(
        matches!(current.get("members"), Some(PackageValue::Array(values)) if values.len() == 1)
    );

    let history = call(
        &host,
        mount.clone(),
        "history",
        capability(
            "teamarness.membership",
            "was_member",
            object([
                (
                    "identity",
                    PackageValue::String(fixture.core.public_identity().to_string()),
                ),
                ("atUnixMs", PackageValue::String("0".into())),
            ]),
        ),
    )
    .await;
    assert_eq!(
        history.get("warning"),
        Some(&PackageValue::String(
            "informational_not_authorization".into()
        ))
    );

    let signed = call(
        &host,
        mount.clone(),
        "sign",
        capability(
            "teamarness.signing",
            "sign",
            object([
                ("purpose", PackageValue::String("fixture.read".into())),
                ("payload", PackageValue::String("aGVsbG8".into())),
            ]),
        ),
    )
    .await;
    assert_eq!(
        signed.get("envelopeVersion"),
        Some(&PackageValue::Integer(1))
    );
    let signer = signed.get("signer").cloned().unwrap();
    let signature = signed.get("signature").cloned().unwrap();
    let verified = call(
        &host,
        mount.clone(),
        "verify",
        capability(
            "teamarness.signing",
            "verify",
            object([
                ("signer", signer),
                ("purpose", PackageValue::String("fixture.read".into())),
                ("payload", PackageValue::String("aGVsbG8".into())),
                ("signature", signature),
            ]),
        ),
    )
    .await;
    assert_eq!(verified.get("valid"), Some(&PackageValue::Boolean(true)));

    let selector = call(
        &host,
        mount,
        "forbidden-selector",
        capability(
            "teamarness.workspace-context",
            "get",
            object([("workspaceId", PackageValue::String("forged".into()))]),
        ),
    )
    .await;
    assert_eq!(
        selector.get("error"),
        Some(&PackageValue::String("capability_request_invalid".into()))
    );
}

#[tokio::test]
async fn immutable_record_stream_waits_for_a_new_scope() {
    let requirements = [serde_json::json!({
        "identity":"teamarness.synchronization.immutable-record-log",
        "major":1,"minMinor":0,"maxMinor":0
    })];
    let fixture = common::Fixture::node_with_capabilities(
        &[("fixture", "dev.teamarness.fixture")],
        &requirements,
    )
    .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let mount = mount(&host, 0).await;
    let opened = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: mount.clone(),
            request: UiRequestId::new("immutable-subscribe").unwrap(),
            operation: "fixture.immutable_subscribe".into(),
            payload: object([]),
        }))
        .await
        .unwrap();
    assert!(matches!(opened, PackageHostOutcome::UiResponse { .. }));

    // The empty scope's one state event, waited for rather than slept past:
    // the acknowledgement below credits exactly what arrived, so a late event
    // used to mean the credit was for work that never happened.
    poll_immutable(
        &host,
        &mount,
        "the empty scope reports its state",
        |response| !events_of(response).is_empty(),
    )
    .await;
    assert_eq!(
        host.snapshot().await.instances[0].lifecycle,
        teamarness_package_host::LifecycleState::Active
    );

    let credited = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: mount.clone(),
            request: UiRequestId::new("immutable-credit").unwrap(),
            operation: "fixture.immutable_credit".into(),
            payload: object([("amount", PackageValue::Integer(1))]),
        }))
        .await
        .unwrap();
    assert!(matches!(credited, PackageHostOutcome::UiResponse { .. }));

    let workspace = fixture.core.list_workspaces().unwrap()[0].clone();
    fixture
        .core
        .append_record(
            workspace.id(),
            "dev.teamarness.fixture",
            "fixture.record",
            b"hello",
        )
        .unwrap();
    let value = poll_immutable(
        &host,
        &mount,
        "the record reaches the package",
        |response| record_events(response) == 1,
    )
    .await;
    assert_eq!(
        record_events(&value),
        1,
        "one record event, beside its state events"
    );
}

/// The language-neutral reference implementation cannot emit credit for work
/// it never accepted.
///
/// `fixture.immutable_credit` put every frame the request asked for on the
/// wire with nothing bounding it by what the stream had delivered. Since the
/// window became a ceiling, two frames of one credit on an empty scope — which
/// emits a single state event — carry the stream to 33 against 32 and are
/// `credit_violation`: fatal, and the instance goes `Failed` with its mount
/// cleared. The contract's own reference implementation is the last place that
/// should be able to do it.
#[tokio::test]
async fn crediting_more_than_the_stream_delivered_keeps_the_generation() {
    let requirements = [serde_json::json!({
        "identity":"teamarness.synchronization.immutable-record-log",
        "major":1,"minMinor":0,"maxMinor":0
    })];
    let fixture = common::Fixture::node_with_capabilities(
        &[("fixture", "dev.teamarness.fixture")],
        &requirements,
    )
    .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let mount = mount(&host, 0).await;
    fixture_request(
        &host,
        &mount,
        "over-credit-subscribe",
        "fixture.immutable_subscribe",
        object([]),
    )
    .await;
    poll_immutable(
        &host,
        &mount,
        "the empty scope reports its state",
        |response| !events_of(response).is_empty(),
    )
    .await;

    let credited = fixture_request(
        &host,
        &mount,
        "over-credit",
        "fixture.immutable_credit",
        object([
            ("amount", PackageValue::Integer(1)),
            ("frames", PackageValue::Integer(2)),
        ]),
    )
    .await;
    assert_eq!(
        credited.get("credited"),
        Some(&PackageValue::Integer(1)),
        "only the one event the stream accepted is credited"
    );
    assert_eq!(
        host.snapshot().await.instances[0].lifecycle,
        teamarness_package_host::LifecycleState::Active,
        "an over-window grant would be `credit_violation` and fail the generation"
    );
}

#[tokio::test]
async fn immutable_record_capability_appends_and_opens_through_core() {
    let requirements = [serde_json::json!({
        "identity":"teamarness.synchronization.immutable-record-log",
        "major":1,"minMinor":0,"maxMinor":0
    })];
    let fixture = common::Fixture::node_with_capabilities(
        &[("fixture", "dev.teamarness.fixture")],
        &requirements,
    )
    .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let mount = mount(&host, 0).await;
    let appended = call(
        &host,
        mount.clone(),
        "record-append",
        capability(
            "teamarness.synchronization.immutable-record-log",
            "append",
            object([
                ("purpose", PackageValue::String("message.post".into())),
                ("canonical", PackageValue::String("aGVsbG8".into())),
            ]),
        ),
    )
    .await;
    assert_eq!(
        appended.get("durability"),
        Some(&PackageValue::String("committed".into()))
    );
    let sealed = appended.get("sealedRecord").cloned().unwrap();
    let opened = call(
        &host,
        mount,
        "record-open",
        capability(
            "teamarness.synchronization.immutable-record-log",
            "open",
            object([("sealedRecords", PackageValue::Array(vec![sealed]))]),
        ),
    )
    .await;
    let PackageValue::Array(results) = opened.get("results").unwrap() else {
        panic!("open result was not an array")
    };
    assert!(
        matches!(results.first(), Some(PackageValue::Object(result)) if result.get("status") == Some(&PackageValue::String("ready".into())))
    );
}

#[tokio::test]
async fn immutable_record_open_classifies_each_envelope_independently() {
    let requirements = [serde_json::json!({
        "identity":"teamarness.synchronization.immutable-record-log",
        "major":1,"minMinor":0,"maxMinor":0
    })];
    let fixture = common::Fixture::node_with_capabilities(
        &[("fixture", "dev.teamarness.fixture")],
        &requirements,
    )
    .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let mount = mount(&host, 0).await;
    let appended = call(
        &host,
        mount.clone(),
        "mixed-open-append",
        capability(
            "teamarness.synchronization.immutable-record-log",
            "append",
            object([
                ("purpose", PackageValue::String("message.post".into())),
                ("canonical", PackageValue::String("cmVhZHk".into())),
            ]),
        ),
    )
    .await;
    let ready = appended.get("sealedRecord").cloned().unwrap();
    let workspace = fixture.core.list_workspaces().unwrap()[0].clone();
    let wrong_scope = fixture
        .core
        .append_record(
            workspace.id(),
            "dev.teamarness.other",
            "message.post",
            b"wrong scope",
        )
        .unwrap();
    let opened = call(
        &host,
        mount,
        "mixed-open",
        capability(
            "teamarness.synchronization.immutable-record-log",
            "open",
            object([(
                "sealedRecords",
                PackageValue::Array(vec![
                    ready,
                    PackageValue::String("AA".into()),
                    PackageValue::String(URL_SAFE_NO_PAD.encode(wrong_scope.sealed_record())),
                ]),
            )]),
        ),
    )
    .await;
    let PackageValue::Array(results) = opened.get("results").unwrap() else {
        panic!("open result was not an array")
    };
    let statuses = results
        .iter()
        .map(|result| {
            let PackageValue::Object(result) = result else {
                panic!("open item was not an object")
            };
            result.get("status").cloned().unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        statuses,
        vec![
            PackageValue::String("ready".into()),
            PackageValue::String("corrupt".into()),
            PackageValue::String("wrong_scope".into()),
        ]
    );
}

#[tokio::test]
async fn rejects_forged_cross_scope_and_invalid_signing_calls() {
    let requirements = [serde_json::json!({
        "identity":"teamarness.signing","major":1,"minMinor":0,"maxMinor":0
    })];
    let fixture = common::Fixture::node_with_capabilities(
        &[("one", "dev.teamarness.one"), ("two", "dev.teamarness.two")],
        &requirements,
    )
    .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let first = mount(&host, 0).await;
    let second = mount(&host, 1).await;

    let grant = grant(&host, first, "grant", "teamarness.signing").await;
    let cross_scope = call(
        &host,
        second.clone(),
        "cross-scope",
        object([
            (
                "capability",
                PackageValue::String("teamarness.signing".into()),
            ),
            ("method", PackageValue::String("sign".into())),
            ("grant", grant),
            (
                "arguments",
                object([
                    ("purpose", PackageValue::String("fixture.read".into())),
                    ("payload", PackageValue::String("aA".into())),
                ]),
            ),
        ]),
    )
    .await;
    assert_eq!(
        cross_scope.get("error"),
        Some(&PackageValue::String("capability_request_invalid".into()))
    );

    for (request, purpose, payload) in [
        ("empty-purpose", "", "aA".to_owned()),
        ("invalid-purpose", "bad purpose", "aA".to_owned()),
        ("long-purpose", &"x".repeat(65), "aA".to_owned()),
    ] {
        let response = call(
            &host,
            second.clone(),
            request,
            capability(
                "teamarness.signing",
                "sign",
                object([
                    ("purpose", PackageValue::String(purpose.to_owned())),
                    ("payload", PackageValue::String(payload)),
                ]),
            ),
        )
        .await;
        assert_eq!(
            response.get("error"),
            Some(&PackageValue::String("capability_request_invalid".into()))
        );
    }

    let error = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: second,
            request: UiRequestId::new("large-payload").unwrap(),
            operation: "fixture.capability".into(),
            payload: capability(
                "teamarness.signing",
                "sign",
                object([
                    ("purpose", PackageValue::String("fixture.read".into())),
                    ("payload", PackageValue::String("YQ".repeat(174_764))),
                ]),
            ),
        }))
        .await
        .unwrap_err();
    assert_eq!(error.code, PackageErrorCode::UiContractViolation);
}

#[tokio::test]
async fn rejects_cross_workspace_cross_installation_and_stale_generation_grants() {
    let requirements = [serde_json::json!({
        "identity":"teamarness.signing","major":1,"minMinor":0,"maxMinor":0
    })];
    let fixture = common::Fixture::node_with_capabilities(
        &[("fixture", "dev.teamarness.fixture")],
        &requirements,
    )
    .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let original_key = host.snapshot().await.instances[0].key.clone();
    let first_mount = mount(&host, 0).await;
    let first_grant = grant(
        &host,
        first_mount.clone(),
        "scope-grant",
        "teamarness.signing",
    )
    .await;

    fixture.core.create_workspace("Second Workspace").unwrap();
    host.execute(PackageHostCommand::ReconcileWorkspaces(
        ReconcileWorkspaces {},
    ))
    .await
    .unwrap();
    let second_index = host
        .snapshot()
        .await
        .instances
        .iter()
        .position(|instance| instance.key != original_key)
        .unwrap();
    let second_mount = mount(&host, second_index).await;
    let arguments = object([
        ("purpose", PackageValue::String("fixture.read".into())),
        ("payload", PackageValue::String("aA".into())),
    ]);
    let cross_workspace = call(
        &host,
        second_mount,
        "cross-workspace",
        object([
            (
                "capability",
                PackageValue::String("teamarness.signing".into()),
            ),
            ("method", PackageValue::String("sign".into())),
            ("grant", first_grant.clone()),
            ("arguments", arguments.clone()),
        ]),
    )
    .await;
    assert_eq!(
        cross_workspace.get("error"),
        Some(&PackageValue::String("capability_request_invalid".into()))
    );

    let other = common::Fixture::node_with_capabilities(
        &[("fixture", "dev.teamarness.fixture")],
        &requirements,
    )
    .await;
    let other_host = PackageHost::open(other.config.clone(), other.core.clone())
        .await
        .unwrap();
    let other_mount = mount(&other_host, 0).await;
    let cross_installation = call(
        &other_host,
        other_mount,
        "cross-installation",
        object([
            (
                "capability",
                PackageValue::String("teamarness.signing".into()),
            ),
            ("method", PackageValue::String("sign".into())),
            ("grant", first_grant.clone()),
            ("arguments", arguments.clone()),
        ]),
    )
    .await;
    assert_eq!(
        cross_installation.get("error"),
        Some(&PackageValue::String("capability_request_invalid".into()))
    );

    let abort = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: first_mount,
            request: UiRequestId::new("abort-generation").unwrap(),
            operation: "fixture.abort".into(),
            payload: object([]),
        }))
        .await;
    assert!(abort.is_err());
    let snapshot = host.snapshot().await;
    let original_workspace = snapshot
        .instances
        .iter()
        .position(|instance| instance.process_generation == Some(2))
        .expect("the failed generation restarted");
    let replacement_mount = mount(&host, original_workspace).await;
    let stale = call(
        &host,
        replacement_mount,
        "stale-generation",
        object([
            (
                "capability",
                PackageValue::String("teamarness.signing".into()),
            ),
            ("method", PackageValue::String("sign".into())),
            ("grant", first_grant),
            ("arguments", arguments),
        ]),
    )
    .await;
    assert_eq!(
        stale.get("error"),
        Some(&PackageValue::String("capability_request_invalid".into()))
    );
}

#[test]
fn capability_contracts_are_dependency_neutral_and_have_no_unapproved_family() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/package-capabilities/v1");
    for name in [
        "workspace-context.schema.json",
        "membership.schema.json",
        "principals.schema.json",
        "signing.schema.json",
        "immutable-record-log.schema.json",
        "signing-envelope.golden.json",
    ] {
        let bytes = std::fs::read(root.join(name)).unwrap();
        let value = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap();
        if name.ends_with(".schema.json") {
            jsonschema::options()
                .with_draft(jsonschema::Draft::Draft202012)
                .build(&value)
                .unwrap();
        }
    }
    let mut contract = String::new();
    for entry in std::fs::read_dir(&root).unwrap() {
        let path = entry.unwrap().path();
        if matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some(
                "immutable-record-log.schema.json" | "immutable-record-log.test.mjs" | "README.md"
            )
        ) {
            continue;
        }
        contract.push_str(&std::fs::read_to_string(path).unwrap());
    }
    let lowercase = contract.to_ascii_lowercase();
    for forbidden in [
        "privatekey",
        "secret bytes",
        "coreruntime",
        "rusqlite",
        "sqlite",
        "iroh",
        "p2panda",
        "signingcontext",
        concat!("synchro", "nization"),
    ] {
        assert!(
            !lowercase.contains(forbidden),
            "forbidden contract value: {forbidden}"
        );
    }
}

/// Verifies: TEAM.SYS.PKG-R03
#[tokio::test]
async fn rejects_capability_versions_before_process_activation() {
    for (requirement, code) in [
        (
            serde_json::json!({"identity":"teamarness.membership","major":2,"minMinor":0,"maxMinor":0}),
            PackageErrorCode::CapabilityVersionUnsupported,
        ),
        (
            serde_json::json!({"identity":"teamarness.membership","major":1,"minMinor":1,"maxMinor":2}),
            PackageErrorCode::CapabilityVersionUnsupported,
        ),
        (
            serde_json::json!({"identity":"example.neutral","major":1,"minMinor":0,"maxMinor":0}),
            PackageErrorCode::CapabilityUnavailable,
        ),
    ] {
        let fixture = common::Fixture::node_with_capabilities(
            &[("fixture", "dev.teamarness.fixture")],
            &[requirement],
        )
        .await;
        let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
            .await
            .unwrap();
        let snapshot = host.snapshot().await;
        assert_eq!(snapshot.instances.len(), 1);
        assert!(snapshot.diagnostics.iter().any(|error| error.code == code));
    }
}

/// Verifies: TEAM.SYS.TRANS-R07, TEAM.SYS.PKG-R10
#[tokio::test]
async fn capability_call_deadline_fires_while_core_is_busy() {
    let requirements = [serde_json::json!({
        "identity":"teamarness.synchronization.immutable-record-log",
        "major":1,"minMinor":0,"maxMinor":0
    })];
    let fixture = common::Fixture::node_with_capabilities(
        &[("fixture", "dev.teamarness.fixture")],
        &requirements,
    )
    .await;
    let config = fixture
        .config
        .clone()
        .with_capability_call_timeout_for_tests(std::time::Duration::from_millis(200));
    let host = PackageHost::open(config, fixture.core.clone())
        .await
        .unwrap();
    let mount = mount(&host, 0).await;
    let append = capability(
        "teamarness.synchronization.immutable-record-log",
        "append",
        object([
            ("purpose", PackageValue::String("message.post".into())),
            ("canonical", PackageValue::String("aGVsbG8".into())),
        ]),
    );

    // Core is busy for longer than the call deadline, which must still fire.
    let hold = fixture.core.hold_state_for_tests();
    let refused = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        call(&host, mount.clone(), "deadline-append", append.clone()),
    )
    .await
    .expect("the call deadline fired while Core was busy");
    assert_eq!(
        refused.get("error"),
        Some(&PackageValue::String("deadline_exceeded".into()))
    );
    assert_eq!(refused.get("retryable"), Some(&PackageValue::Boolean(true)));
    assert_eq!(
        refused.get("outcomeUncertain"),
        Some(&PackageValue::Boolean(true)),
        "the append had already started, so its outcome is uncertain"
    );
    hold.release();

    // The retry is content-idempotent: the Package sees one record, not two.
    let retried = call(&host, mount, "retried-append", append).await;
    assert_eq!(
        retried.get("durability"),
        Some(&PackageValue::String("committed".into()))
    );
    let workspace = fixture.core.list_workspaces().unwrap()[0].clone();
    let mut records = fixture
        .core
        .subscribe_records(workspace.id(), "dev.teamarness.fixture", None)
        .unwrap();
    let mut committed = 0;
    while let Ok(Some(_)) = records.next_event_timeout(std::time::Duration::ZERO) {
        committed += 1;
    }
    assert_eq!(committed, 1, "the timed-out append committed nothing");
}

/// Runs one fixture request and returns its response object.
async fn fixture_request(
    host: &PackageHost,
    mount: &teamarness_package_host::UiMountId,
    request: &str,
    operation: &str,
    payload: PackageValue,
) -> BTreeMap<String, PackageValue> {
    match host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: mount.clone(),
            request: UiRequestId::new(request).unwrap(),
            operation: operation.into(),
            payload,
        }))
        .await
        .unwrap_or_else(|error| panic!("{request}/{operation}: {error:?}"))
    {
        PackageHostOutcome::UiResponse {
            payload: PackageValue::Object(value),
        } => value,
        other => panic!("unexpected fixture response: {other:?}"),
    }
}

static NEXT_POLL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Polls the fixture's recorded immutable-stream state until `ready` holds.
///
/// The deadline only bounds failure; nothing here sleeps and then asserts.
async fn poll_immutable(
    host: &PackageHost,
    mount: &teamarness_package_host::UiMountId,
    what: &str,
    mut ready: impl FnMut(&BTreeMap<String, PackageValue>) -> bool,
) -> BTreeMap<String, PackageValue> {
    let expiry = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        // A request identifier is used once per mount, so the counter spans
        // every poll in the test rather than restarting with each call.
        let attempt = NEXT_POLL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let response = fixture_request(
            host,
            mount,
            &format!("immutable-poll-{attempt}"),
            "fixture.immutable_events",
            object([]),
        )
        .await;
        if ready(&response) {
            return response;
        }
        assert!(
            std::time::Instant::now() < expiry,
            "timed out waiting until {what}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// How many of the recorded stream events are records rather than state or
/// warning events.
fn record_events(response: &BTreeMap<String, PackageValue>) -> usize {
    events_of(response)
        .iter()
        .filter(|event| {
            matches!(event, PackageValue::Object(event)
                if event.get("kind") == Some(&PackageValue::String("record".into())))
        })
        .count()
}

fn events_of(response: &BTreeMap<String, PackageValue>) -> &[PackageValue] {
    match response.get("events") {
        Some(PackageValue::Array(events)) => events,
        _ => panic!("the fixture always reports an events array"),
    }
}

/// Verifies: TEAM.SYS.PKG-R10
#[tokio::test]
async fn record_stream_closes_with_cursor_invalid_for_a_future_cursor() {
    let requirements = [serde_json::json!({
        "identity":"teamarness.synchronization.immutable-record-log",
        "major":1,"minMinor":0,"maxMinor":0
    })];
    let fixture = common::Fixture::node_with_capabilities(
        &[("fixture", "dev.teamarness.fixture")],
        &requirements,
    )
    .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let mount = mount(&host, 0).await;
    let workspace = fixture.core.list_workspaces().unwrap()[0].clone();
    fixture
        .core
        .append_record(
            workspace.id(),
            "dev.teamarness.fixture",
            "fixture.record",
            b"one",
        )
        .unwrap();
    let mut records = fixture
        .core
        .subscribe_records(workspace.id(), "dev.teamarness.fixture", None)
        .unwrap();
    let committed = records
        .next_event_timeout(std::time::Duration::from_secs(5))
        .unwrap()
        .expect("the record is committed");
    // The same journal, far beyond its head: the Package must reset.
    let mut cursor = committed.cursor().to_string();
    let head = cursor.len() - 16;
    cursor.replace_range(head.., &format!("{:016x}", 999_u64));

    fixture_request(
        &host,
        &mount,
        "immutable-subscribe",
        "fixture.immutable_subscribe",
        object([("afterCursor", PackageValue::String(cursor))]),
    )
    .await;
    let response = poll_immutable(&host, &mount, "the stream is closed", |response| {
        matches!(response.get("closeCode"), Some(PackageValue::String(_)))
    })
    .await;
    assert_eq!(
        response.get("closeCode"),
        Some(&PackageValue::String("cursor_invalid".into()))
    );
    assert!(events_of(&response).is_empty(), "no record was delivered");
}

/// A Package catching up on a backlog returns its whole credit window one event
/// at a time, which is what the SDK's commit-before-credit rule produces. Every
/// one of those frames is in the contract, so none of them may be refused and
/// none may cost the Package its process.
#[tokio::test]
async fn a_whole_credit_window_returned_at_once_keeps_the_backend_running() {
    let requirements = [serde_json::json!({
        "identity":"teamarness.synchronization.immutable-record-log",
        "major":1,"minMinor":0,"maxMinor":0
    })];
    let fixture = common::Fixture::node_with_capabilities(
        &[("fixture", "dev.teamarness.fixture")],
        &requirements,
    )
    .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let mount = mount(&host, 0).await;
    let workspace = fixture.core.list_workspaces().unwrap()[0].clone();
    let package = "dev.teamarness.fixture";

    fixture_request(
        &host,
        &mount,
        "window-subscribe",
        "fixture.immutable_subscribe",
        object([]),
    )
    .await;
    for index in 0..200_u32 {
        fixture
            .core
            .append_record(
                workspace.id(),
                package,
                "fixture.record",
                &index.to_be_bytes(),
            )
            .unwrap();
    }
    // The window carries one state event before the records, so a spent window
    // is 31 records, not 32.
    poll_immutable(&host, &mount, "the initial window is spent", |response| {
        events_of(response).len() >= 32
    })
    .await;

    // The whole window comes back as 32 single-event frames in one burst.
    fixture_request(
        &host,
        &mount,
        "window-credit",
        "fixture.immutable_credit",
        object([
            ("amount", PackageValue::Integer(1)),
            ("frames", PackageValue::Integer(32)),
        ]),
    )
    .await;

    poll_immutable(
        &host,
        &mount,
        "the second window is delivered",
        |response| record_events(response) >= 55,
    )
    .await;
    assert_eq!(
        host.snapshot().await.instances[0].lifecycle,
        teamarness_package_host::LifecycleState::Active,
        "returning the credit window cost the Package its process"
    );
    // The mount outlives the catch-up, so the UI keeps working through it.
    fixture_request(
        &host,
        &mount,
        "window-after",
        "fixture.immutable_events",
        object([]),
    )
    .await;
}

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test]
async fn slow_package_leaves_core_without_buffered_events() {
    let requirements = [serde_json::json!({
        "identity":"teamarness.synchronization.immutable-record-log",
        "major":1,"minMinor":0,"maxMinor":0
    })];
    let fixture = common::Fixture::node_with_capabilities(
        &[("fixture", "dev.teamarness.fixture")],
        &requirements,
    )
    .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let mount = mount(&host, 0).await;
    let workspace = fixture.core.list_workspaces().unwrap()[0].clone();
    let package = "dev.teamarness.fixture";

    fixture_request(
        &host,
        &mount,
        "immutable-subscribe",
        "fixture.immutable_subscribe",
        object([]),
    )
    .await;
    for index in 0..1_000_u32 {
        fixture
            .core
            .append_record(
                workspace.id(),
                package,
                "fixture.record",
                &index.to_be_bytes(),
            )
            .unwrap();
    }
    // The fixture never credits beyond its initial 32, so the stream stops
    // there. Core holds no queue, so nothing further is decrypted.
    poll_immutable(&host, &mount, "the initial credit is spent", |response| {
        events_of(response).len() >= 32
    })
    .await;
    let decoded = teamarness_core::records_decoded_for_tests(workspace.id(), package);
    assert!(
        decoded <= 64,
        "an uncredited stream decrypted {decoded} records"
    );

    for index in 1_000..1_500_u32 {
        fixture
            .core
            .append_record(
                workspace.id(),
                package,
                "fixture.record",
                &index.to_be_bytes(),
            )
            .unwrap();
    }
    let after = teamarness_core::records_decoded_for_tests(workspace.id(), package);
    assert!(
        after <= 64,
        "500 more commits made the stream decrypt {after} records"
    );
    let response = fixture_request(
        &host,
        &mount,
        "immutable-final",
        "fixture.immutable_events",
        object([]),
    )
    .await;
    assert_eq!(
        events_of(&response).len(),
        32,
        "delivery stops exactly at the granted credit"
    );
}

/// Verifies: TEAM.SYS.MEM-R05
#[tokio::test]
async fn membership_stream_does_not_poll() {
    let fixture = common::Fixture::node_with_capabilities(
        &[("fixture", "dev.teamarness.fixture")],
        &[serde_json::json!({
            "identity":"teamarness.membership","major":1,"minMinor":0,"maxMinor":0
        })],
    )
    .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let mount = mount(&host, 0).await;
    let workspace = fixture.core.list_workspaces().unwrap()[0].clone();

    fixture_request(
        &host,
        &mount,
        "membership-open",
        "fixture.membership_open",
        object([("afterEpoch", PackageValue::String("0".into()))]),
    )
    .await;

    // The stream delivers transitions in epoch order, one Core subscription
    // each, so the Package holding the newest one is the task parked on the
    // epoch watch with every earlier subscription already counted. That is the
    // probe for "the loop has run"; a credit round trip cannot be, because
    // credit is only owed for an event the Package has consumed.
    join_one_member(&fixture, &workspace, "membership-joiner-one").await;
    await_membership_events(&host, &mount, "one", 1).await;
    // One event consumed, one credit released: the round trip runs the stream
    // task's command arm without widening the window the stream was opened
    // with.
    fixture_request(
        &host,
        &mount,
        "membership-credit",
        "fixture.membership_credit",
        object([("amount", PackageValue::Integer(1))]),
    )
    .await;
    let idle = teamarness_core::membership_subscriptions_opened_for_tests(workspace.id());

    join_one_member(&fixture, &workspace, "membership-joiner-two").await;
    await_membership_events(&host, &mount, "two", 2).await;
    let after = teamarness_core::membership_subscriptions_opened_for_tests(workspace.id());
    assert_eq!(
        after - idle,
        1,
        "one transition opens exactly one Core subscription"
    );
}

/// Admits one more Installation to the workspace, which is one membership
/// transition.
async fn join_one_member(
    fixture: &common::Fixture,
    workspace: &teamarness_core::WorkspaceSummary,
    directory: &str,
) {
    let joiner = common::open_core(fixture._root.path().join(directory)).await;
    common::introduce(&fixture.core, &joiner);
    let invitation = common::invite(&fixture.core, workspace.id()).await;
    joiner.join_workspace(&invitation).await.unwrap();
    joiner.shutdown().await.unwrap();
}

/// Waits until the Package has recorded `count` membership events.
async fn await_membership_events(
    host: &PackageHost,
    mount: &teamarness_package_host::UiMountId,
    label: &str,
    count: usize,
) {
    let expiry = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let mut attempt = 0_u64;
    loop {
        attempt += 1;
        let response = fixture_request(
            host,
            mount,
            &format!("membership-events-{label}-{attempt}"),
            "fixture.membership_events",
            object([]),
        )
        .await;
        if matches!(response.get("events"), Some(PackageValue::Array(events)) if events.len() >= count)
        {
            return;
        }
        assert!(
            std::time::Instant::now() < expiry,
            "the admitted transition reaches the Package"
        );
        // poll-interval: the interval of this loop, which is bounded by its own deadline.
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// Renders one recorded stream event as a compact, comparable string.
fn describe(event: &PackageValue) -> String {
    let PackageValue::Object(event) = event else {
        return "<not an object>".to_owned();
    };
    let text = |key: &str| match event.get(key) {
        Some(PackageValue::String(value)) => value.clone(),
        _ => String::new(),
    };
    match text("kind").as_str() {
        "record" => "record".to_owned(),
        "state" => format!(
            "state {} {}",
            text("availability"),
            text("pendingRecordCount")
        ),
        "warning" => format!("warning {}", text("code")),
        other => format!("unknown {other}"),
    }
}

fn described(response: &BTreeMap<String, PackageValue>) -> Vec<String> {
    events_of(response).iter().map(describe).collect()
}

/// Verifies: TEAM.SYS.PKG-R10
#[tokio::test]
async fn immutable_record_stream_reports_catching_up_then_current() {
    let requirements = [serde_json::json!({
        "identity":"teamarness.synchronization.immutable-record-log",
        "major":1,"minMinor":0,"maxMinor":0
    })];
    let fixture = common::Fixture::node_with_capabilities(
        &[("fixture", "dev.teamarness.fixture")],
        &requirements,
    )
    .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let mount = mount(&host, 0).await;
    let workspace = fixture.core.list_workspaces().unwrap()[0].clone();
    for index in 0..3_u8 {
        fixture
            .core
            .append_record(
                workspace.id(),
                "dev.teamarness.fixture",
                "fixture.record",
                &[index; 4],
            )
            .unwrap();
    }

    fixture_request(
        &host,
        &mount,
        "immutable-subscribe",
        "fixture.immutable_subscribe",
        object([]),
    )
    .await;
    // A solo Workspace with no concurrent appends produces exactly this.
    let response = poll_immutable(&host, &mount, "the stream reports current", |response| {
        events_of(response).len() >= 5
    })
    .await;
    assert_eq!(
        described(&response),
        vec![
            "state catching_up 3".to_owned(),
            "record".to_owned(),
            "record".to_owned(),
            "record".to_owned(),
            "state current 0".to_owned(),
        ]
    );
}

/// Verifies: TEAM.SYS.PKG-R10, TEAM.SYS.TRANS-R08
#[tokio::test]
async fn immutable_record_stream_is_current_for_creator_before_first_record() {
    let requirements = [serde_json::json!({
        "identity":"teamarness.synchronization.immutable-record-log",
        "major":1,"minMinor":0,"maxMinor":0
    })];
    let fixture = common::Fixture::node_with_capabilities(
        &[("fixture", "dev.teamarness.fixture")],
        &requirements,
    )
    .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let mount = mount(&host, 0).await;
    let workspace = fixture.core.list_workspaces().unwrap()[0].clone();

    fixture_request(
        &host,
        &mount,
        "immutable-subscribe",
        "fixture.immutable_subscribe",
        object([]),
    )
    .await;
    let first = poll_immutable(&host, &mount, "the first state event arrives", |response| {
        !events_of(response).is_empty()
    })
    .await;
    assert_eq!(described(&first), ["state current 0"]);
    let database = rusqlite::Connection::open_with_flags(
        fixture._root.path().join("core/core.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let declared: i64 = database
        .query_row(
            "SELECT COUNT(*) FROM sync_scopes WHERE package_identity=?1",
            ["dev.teamarness.fixture"],
            |row| row.get(0),
        )
        .unwrap();
    let operations: i64 = database
        .query_row("SELECT COUNT(*) FROM immutable_operations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        (declared, operations),
        (1, 0),
        "the Host activation's ordinary capability stream declares an empty generic scope"
    );
    assert_eq!(
        record_events(&first),
        0,
        "an ordinary Host capability stream bootstraps the creator without a Package record"
    );

    for index in 0..3_u8 {
        fixture
            .core
            .append_record(
                workspace.id(),
                "dev.teamarness.fixture",
                "fixture.record",
                &[index; 4],
            )
            .unwrap();
    }
    // Appends race the stream task, so the intermediate states are not fixed.
    let response = poll_immutable(&host, &mount, "every record is delivered", |response| {
        record_events(response) == 3
            && described(response).last().map(String::as_str) == Some("state current 0")
    })
    .await;
    let events = described(&response);
    assert!(
        events[1..]
            .iter()
            .filter(|event| event.starts_with("state "))
            .all(|event| event.starts_with("state catching_up")
                || event.starts_with("state current")),
        "after the creator's first append the stream only reports catching up or current: {events:?}"
    );
}

/// Verifies: TEAM.SYS.PKG-R10
#[tokio::test]
async fn immutable_record_stream_reports_offline_from_unreachable_and_failed_peers() {
    let requirements = [serde_json::json!({
        "identity":"teamarness.synchronization.immutable-record-log",
        "major":1,"minMinor":0,"maxMinor":0
    })];
    let fixture = common::Fixture::node_with_capabilities(
        &[("fixture", "dev.teamarness.fixture")],
        &requirements,
    )
    .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let mount = mount(&host, 0).await;
    let workspace = fixture.core.list_workspaces().unwrap()[0].clone();
    let package = "dev.teamarness.fixture";
    // The scheduler must not overwrite the injected outcomes.
    fixture.core.pause_sync_scheduler_for_tests(true);
    let peer = fixture
        .core
        .admit_unreachable_member_for_tests(workspace.id());
    fixture
        .core
        .append_record(workspace.id(), package, "fixture.record", b"one")
        .unwrap();

    fixture_request(
        &host,
        &mount,
        "immutable-subscribe",
        "fixture.immutable_subscribe",
        object([]),
    )
    .await;
    // With no outcome for the new Member yet, row 8 keeps the stream catching
    // up, so no further state event follows the record.
    let response = poll_immutable(&host, &mount, "the record is delivered", |response| {
        record_events(response) == 1
    })
    .await;
    assert_eq!(
        described(&response),
        vec!["state catching_up 1".to_owned(), "record".to_owned()]
    );

    let mut before = events_of(&response).len();
    for (injected, expected) in [
        ("unreachable", "state offline 0"),
        ("reachable", "state current 0"),
        ("failed", "state offline 0"),
    ] {
        let seen = before;
        fixture
            .core
            .set_peer_outcome_for_tests(workspace.id(), package, peer, injected);
        let next = poll_immutable(
            &host,
            &mount,
            "the injected outcome is reported",
            |response| events_of(response).len() > seen,
        )
        .await;
        assert_eq!(described(&next).last().map(String::as_str), Some(expected));
        before = events_of(&next).len();
    }
}

/// Verifies: TEAM.SYS.PKG-R10
#[tokio::test]
async fn immutable_record_stream_warns_once_when_a_scope_is_quarantined() {
    let requirements = [serde_json::json!({
        "identity":"teamarness.synchronization.immutable-record-log",
        "major":1,"minMinor":0,"maxMinor":0
    })];
    let fixture = common::Fixture::node_with_capabilities(
        &[("fixture", "dev.teamarness.fixture")],
        &requirements,
    )
    .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let mount = mount(&host, 0).await;
    let workspace = fixture.core.list_workspaces().unwrap()[0].clone();
    let package = "dev.teamarness.fixture";
    fixture
        .core
        .append_record(workspace.id(), package, "fixture.record", b"one")
        .unwrap();

    fixture_request(
        &host,
        &mount,
        "immutable-subscribe",
        "fixture.immutable_subscribe",
        object([]),
    )
    .await;
    let response = poll_immutable(&host, &mount, "the stream reports current", |response| {
        described(response).last().map(String::as_str) == Some("state current 0")
    })
    .await;
    let before = events_of(&response).len();

    fixture
        .core
        .quarantine_scope_for_tests(workspace.id(), package);
    let response = poll_immutable(&host, &mount, "the quarantine is reported", |response| {
        events_of(response).len() >= before + 2
    })
    .await;
    assert_eq!(
        described(&response)[before..],
        [
            "warning scope_quarantined".to_owned(),
            "state offline 0".to_owned()
        ]
    );
    let after_warning = events_of(&response).len();

    assert!(
        fixture
            .core
            .append_record(workspace.id(), package, "fixture.record", b"two")
            .is_err(),
        "a quarantined scope refuses appends"
    );
    fixture_request(
        &host,
        &mount,
        "immutable-credit-after-quarantine",
        "fixture.immutable_credit",
        object([("amount", PackageValue::Integer(1))]),
    )
    .await;
    let response = fixture_request(
        &host,
        &mount,
        "immutable-final-events",
        "fixture.immutable_events",
        object([]),
    )
    .await;
    assert_eq!(
        events_of(&response).len(),
        after_warning,
        "the warning is sent once per stream"
    );
}
