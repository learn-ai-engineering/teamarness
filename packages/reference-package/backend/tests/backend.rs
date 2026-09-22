//! The Reference Backend through the SDK harness, with scripted Semantic Capabilities.

use std::time::Duration;

use teamarness_package_sdk::capabilities::{membership, signing, workspace_context};
use teamarness_package_sdk::testing::{
    Harness, HarnessBuilder, ScriptedCapability, assert_router_matches_contract,
};
use teamarness_reference_package::router;

const INSTALLATION: &str = "9999999999999999999999999999999999999999999999999999999999999999";
const MEMBERSHIP_VERSION: &str = "3333333333333333333333333333333333333333333333333333333333333333";

fn empty() -> serde_json::Value {
    serde_json::json!({})
}

fn scripted_capabilities() -> Vec<(&'static str, ScriptedCapability)> {
    vec![
        (
            workspace_context::IDENTITY,
            ScriptedCapability::new().calls(|_, _| {
                Ok(serde_json::json!({
                    "workspaceId": "workspace1",
                    "workspaceName": "Studio",
                    "installationIdentity": INSTALLATION,
                    "installationDisplayName": "Laptop",
                }))
            }),
        ),
        (
            membership::IDENTITY,
            ScriptedCapability::new().calls(|_, _| {
                Ok(serde_json::json!({
                    "workspaceId": "workspace1",
                    "epoch": "4",
                    "membershipVersion": MEMBERSHIP_VERSION,
                    "members": [{ "identity": INSTALLATION, "role": "manager" }],
                }))
            }),
        ),
        (
            signing::IDENTITY,
            ScriptedCapability::new().calls(|method, _| match method {
                "sign" => Ok(serde_json::json!({
                    "signer": INSTALLATION,
                    "envelopeVersion": 1,
                    "signature": "s".repeat(86),
                })),
                "verify" => Ok(serde_json::json!({ "valid": true })),
                other => panic!("unexpected signing method {other}"),
            }),
        ),
    ]
}

async fn start(directory: &std::path::Path) -> Harness {
    start_with(directory, scripted_capabilities()).await
}

async fn start_with(
    directory: &std::path::Path,
    capabilities: Vec<(&'static str, ScriptedCapability)>,
) -> Harness {
    let mut builder = HarnessBuilder::new(router().into_backend())
        .package_identity("dev.teamarness.reference")
        .package_version("1.2.3")
        .data_directory(directory);
    for (identity, scripted) in capabilities {
        builder = builder.capability(identity, scripted);
    }
    builder.start().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn activation_uses_typed_capabilities_and_reads_state() {
    let root = tempfile::tempdir().unwrap();
    let mut harness = start(root.path()).await;
    harness.activate().await.unwrap();

    let snapshot = harness.request("reference.read", empty()).await.unwrap();
    assert_eq!(snapshot["counter"], 0);
    assert_eq!(snapshot["membershipVersion"], MEMBERSHIP_VERSION);
    assert_eq!(snapshot["signingVerified"], true);
    assert_eq!(snapshot["processGeneration"], "1");
    assert_eq!(snapshot["packageVersion"], "1.2.3");
    harness.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn increment_persists_across_recover_in_a_new_harness() {
    let root = tempfile::tempdir().unwrap();
    let mut harness = start(root.path()).await;
    harness.activate().await.unwrap();
    assert_eq!(
        harness
            .request("reference.increment", empty())
            .await
            .unwrap()["counter"],
        1
    );
    harness.shutdown().await.unwrap();

    let mut replacement = start(root.path()).await;
    replacement.recover().await.unwrap();
    assert_eq!(
        replacement
            .request("reference.read", empty())
            .await
            .unwrap()["counter"],
        1
    );
    replacement.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fail_returns_the_requested_package_error() {
    let root = tempfile::tempdir().unwrap();
    let mut harness = start(root.path()).await;
    harness.activate().await.unwrap();

    let error = harness
        .request("reference.fail", empty())
        .await
        .unwrap_err();
    assert_eq!(error.code(), "reference.requested_failure");
    assert!(!error.retryable());
    assert_eq!(error.details()["reason"], "requested");
    harness.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_capability_failure_keeps_the_underlying_code_in_details() {
    let root = tempfile::tempdir().unwrap();
    let mut capabilities = scripted_capabilities();
    capabilities[1] = (
        membership::IDENTITY,
        ScriptedCapability::new().calls(|_, _| {
            Err(teamarness_package_sdk::BackendError::new(
                "capability_unavailable",
                false,
                serde_json::json!({}),
            ))
        }),
    );
    let mut harness = start_with(root.path(), capabilities).await;

    let error = harness.activate().await.unwrap_err();
    assert_eq!(error.code(), "reference.capability_failed");
    assert_eq!(error.details()["capabilityCode"], "capability_unavailable");
    harness.shutdown().await.unwrap();
}

/// The Backend's background marker, read through a request.
async fn marker(harness: &mut Harness) -> i64 {
    harness.request("reference.read", empty()).await.unwrap()["backgroundMarker"]
        .as_i64()
        .unwrap()
}

/// Waits until the background marker reaches `at_least`, and returns it.
async fn wait_for_marker(harness: &mut Harness, at_least: i64, what: &str) -> i64 {
    let expiry = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let value = marker(harness).await;
        if value >= at_least {
            return value;
        }
        assert!(
            std::time::Instant::now() < expiry,
            "timed out waiting until {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn suspend_stops_background_activity_and_resume_restarts_it() {
    let root = tempfile::tempdir().unwrap();
    let mut harness = start(root.path()).await;
    harness.activate().await.unwrap();

    wait_for_marker(&mut harness, 1, "background activity advances the marker").await;
    harness.suspend().await.unwrap();
    // The marker is read through a request, so recover first, read, then suspend again.
    harness.recover().await.unwrap();
    let running = marker(&mut harness).await;
    assert!(running >= 1, "background activity advanced the marker");

    harness.suspend().await.unwrap();
    // negative-wait: a bounded window proving a suspended Backend's marker stays put.
    tokio::time::sleep(Duration::from_millis(300)).await;
    harness.recover().await.unwrap();
    let stopped = marker(&mut harness).await;

    let resumed = wait_for_marker(
        &mut harness,
        stopped + 1,
        "a recovered Backend advances its marker again",
    )
    .await;
    assert!(
        resumed > stopped,
        "recover restarts the background activity: {stopped} then {resumed}"
    );
    harness.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changes_stream_delivers_a_snapshot_per_change() {
    let root = tempfile::tempdir().unwrap();
    let mut harness = start(root.path()).await;
    harness.activate().await.unwrap();

    let mut changes = harness
        .subscribe("reference.changes", empty())
        .await
        .unwrap();
    let first = changes.next_event().await.unwrap().unwrap();
    assert!(first["counter"].as_i64().unwrap() >= 0);
    harness
        .request("reference.increment", empty())
        .await
        .unwrap();
    let mut seen = 0;
    for _ in 0..40 {
        let event = changes.next_event().await.unwrap().unwrap();
        if event["counter"] == 1 {
            seen = 1;
            break;
        }
    }
    assert_eq!(seen, 1, "the increment reached the stream");
    changes.close().await;
    harness.shutdown().await.unwrap();
}

#[test]
fn router_matches_the_ui_contract() {
    assert_router_matches_contract(
        &router().operations(),
        include_str!("../../ui-contract.json"),
    );
}
