mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use teamarness_package_host::{
    OpenUiMount, PackageHost, PackageHostCommand, PackageHostOutcome, PackageValue, UiRequest,
    UiRequestId,
};

fn object(entries: impl IntoIterator<Item = (&'static str, PackageValue)>) -> PackageValue {
    PackageValue::Object(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

async fn request(
    host: &PackageHost,
    mount: &teamarness_package_host::UiMountId,
    id: &str,
    operation: &str,
    payload: PackageValue,
) -> BTreeMap<String, PackageValue> {
    match host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: mount.clone(),
            request: UiRequestId::new(id).unwrap(),
            operation: operation.into(),
            payload,
        }))
        .await
        .unwrap()
    {
        PackageHostOutcome::UiResponse {
            payload: PackageValue::Object(value),
        } => value,
        other => panic!("unexpected outcome: {other:?}"),
    }
}

#[tokio::test]
async fn membership_stream_resumes_after_credit_exhaustion_without_gaps() {
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
    let workspace = fixture.core.list_workspaces().unwrap()[0].clone();
    // Seventeen admissions and sixteen revocations produce 33 roster events,
    // enough to exhaust the 32-credit window. Issuer-local admissions avoid
    // the unrelated retry traffic from unreachable loopback joiners.
    let members = (0..17)
        .map(|_| {
            fixture
                .core
                .admit_unreachable_member_for_tests(workspace.id())
        })
        .collect::<Vec<_>>();
    for member in members.into_iter().take(16) {
        fixture.core.revoke_member(workspace.id(), member).unwrap();
    }

    let instance = host.snapshot().await.instances[0].key.clone();
    let mount = match host
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount { instance }))
        .await
        .unwrap()
    {
        PackageHostOutcome::UiMountOpened { mount, .. } => mount,
        other => panic!("unexpected mount outcome: {other:?}"),
    };

    request(
        &host,
        &mount,
        "open-one",
        "fixture.membership_open",
        object([("afterEpoch", PackageValue::String("2".into()))]),
    )
    .await;
    let first = tokio::time::timeout(Duration::from_secs(5), async {
        let mut attempt = 0_u64;
        loop {
            attempt += 1;
            let response = request(
                &host,
                &mount,
                &format!("events-one-{attempt}"),
                "fixture.membership_events",
                object([]),
            )
            .await;
            if matches!(response.get("events"), Some(PackageValue::Array(events)) if events.len() == 32)
            {
                break response;
            }
            // poll-interval: the interval of this loop, which is bounded by its own timeout.
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let Some(PackageValue::Array(events)) = first.get("events") else {
        panic!("missing events")
    };
    for (index, event) in events.iter().enumerate() {
        let PackageValue::Object(event) = event else {
            panic!("event was not an object")
        };
        assert_eq!(
            event.get("epoch"),
            Some(&PackageValue::String(if index < 17 {
                (2 * index + 4).to_string()
            } else {
                (index + 20).to_string()
            }))
        );
        assert!(
            matches!(event.get("membershipVersion"), Some(PackageValue::String(value)) if value.len() == 64)
        );
        assert!(
            matches!(event.get("cause"), Some(PackageValue::String(value)) if value.len() == 64)
        );
        assert!(
            matches!(event.get("snapshotDigest"), Some(PackageValue::String(value)) if value.len() == 64)
        );
    }

    request(
        &host,
        &mount,
        "credit-one",
        "fixture.membership_credit",
        object([("amount", PackageValue::Integer(1))]),
    )
    .await;
    let caught_up = tokio::time::timeout(Duration::from_secs(2), async {
        let mut attempt = 0_u64;
        loop {
            attempt += 1;
            let response = request(
                &host,
                &mount,
                &format!("events-catch-up-{attempt}"),
                "fixture.membership_events",
                object([]),
            )
            .await;
            if matches!(response.get("events"), Some(PackageValue::Array(events)) if events.len() == 33)
            {
                break response;
            }
            // poll-interval: the interval of this loop, which is bounded by its own timeout.
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        matches!(caught_up.get("events"), Some(PackageValue::Array(events)) if events.len() == 33)
    );

    request(
        &host,
        &mount,
        "close-one",
        "fixture.membership_close",
        object([]),
    )
    .await;
    request(
        &host,
        &mount,
        "open-two",
        "fixture.membership_open",
        object([("afterEpoch", PackageValue::String("52".into()))]),
    )
    .await;
    // settle: lets the reopened subscription deliver before its events are read.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let resumed = request(
        &host,
        &mount,
        "events-two",
        "fixture.membership_events",
        object([]),
    )
    .await;
    assert!(
        matches!(resumed.get("events"), Some(PackageValue::Array(events)) if events.len() == 33)
    );
    request(
        &host,
        &mount,
        "close-two",
        "fixture.membership_close",
        object([]),
    )
    .await;
}
