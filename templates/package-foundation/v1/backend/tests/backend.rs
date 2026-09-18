//! The Backend through the SDK harness: no frames, a real directory, real persistence.

use std::path::Path;

use teamarness_generated_package_backend::{READ, router};
use teamarness_package_sdk::testing::{Harness, HarnessBuilder, assert_router_matches_contract};

// Token substitution changes the literal width, so rustfmt must leave this declaration stable.
#[rustfmt::skip]
const EXPECTED_DISPLAY_NAME: &str = __PACKAGE_DISPLAY_NAME_RUST__;

const UNKNOWN: &str = "__REQUEST_NAMESPACE__.unknown";
const GREETING: &str = "Hello from __REQUEST_NAMESPACE__";

fn empty() -> serde_json::Value {
    serde_json::json!({})
}

async fn start(directory: &Path) -> Harness {
    HarnessBuilder::new(router().into_backend())
        .package_identity("__PACKAGE_IDENTITY__")
        .data_directory(directory)
        .start()
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_greets_and_counts_reads_in_the_data_directory() {
    let directory = tempfile::tempdir().unwrap();
    let mut harness = start(directory.path()).await;

    let inactive = harness.request(READ, empty()).await.unwrap_err();
    assert_eq!(inactive.code(), "package.not_active");

    harness.activate().await.unwrap();

    let first = harness.request(READ, empty()).await.unwrap();
    assert_eq!(first["message"], GREETING);
    assert_eq!(first["displayName"], EXPECTED_DISPLAY_NAME);
    assert_eq!(first["readCount"], 1);

    let second = harness.request(READ, empty()).await.unwrap();
    assert_eq!(second["readCount"], 2);

    let unknown = harness.request(UNKNOWN, empty()).await.unwrap_err();
    assert_eq!(unknown.code(), "package.unknown_operation");
    assert!(!unknown.retryable());

    harness.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_count_survives_a_new_process_generation() {
    let directory = tempfile::tempdir().unwrap();
    let mut first = start(directory.path()).await;
    first.activate().await.unwrap();
    first.request(READ, empty()).await.unwrap();
    first.request(READ, empty()).await.unwrap();
    first.shutdown().await.unwrap();

    // A new process generation on the same Package Data Directory continues where it left off.
    let mut second = start(directory.path()).await;
    second.recover().await.unwrap();
    let third = second.request(READ, empty()).await.unwrap();
    assert_eq!(third["readCount"], 3);

    second.shutdown().await.unwrap();
}

#[test]
fn router_matches_the_ui_contract() {
    assert_router_matches_contract(
        &router().operations(),
        include_str!("../../ui-contract.json"),
    );
}
