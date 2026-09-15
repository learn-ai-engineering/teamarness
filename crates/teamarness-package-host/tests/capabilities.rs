mod common;

use std::collections::BTreeMap;

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
        ReconcileWorkspaces { workspaces: vec![] },
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
