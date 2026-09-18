//! The Tutorial Backend through the SDK harness: requests, lifecycle and stable error codes.

use std::collections::VecDeque;
use std::sync::Mutex;

use teamarness_package_sdk::testing::{Harness, HarnessBuilder, assert_router_matches_contract};
use teamarness_tutorial_package::{SeedError, SeedSource, router, router_with_seed_source};

struct FixedSeeds(Mutex<VecDeque<u32>>);

impl FixedSeeds {
    fn new(seeds: impl IntoIterator<Item = u32>) -> Self {
        Self(Mutex::new(seeds.into_iter().collect()))
    }
}

impl SeedSource for FixedSeeds {
    fn next_seed(&self) -> Result<u32, SeedError> {
        self.0.lock().unwrap().pop_front().ok_or(SeedError)
    }
}

fn empty() -> serde_json::Value {
    serde_json::json!({})
}

async fn start(directory: &std::path::Path, seeds: impl IntoIterator<Item = u32>) -> Harness {
    HarnessBuilder::new(router_with_seed_source(FixedSeeds::new(seeds)).into_backend())
        .package_identity("dev.teamarness.tutorial")
        .data_directory(directory)
        .start()
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backend_runs_the_request_and_lifecycle_contract_with_stable_errors() {
    let root = tempfile::tempdir().unwrap();
    let mut harness = start(root.path(), [7, 91]).await;

    let inactive = harness.request("tutorial.read", empty()).await.unwrap_err();
    assert_eq!(inactive.code(), "package.not_active");
    assert!(inactive.retryable());

    harness.activate().await.unwrap();

    let read = harness.request("tutorial.read", empty()).await.unwrap();
    assert_eq!(read["algorithms"].as_array().unwrap().len(), 4);
    assert_eq!(
        read["algorithms"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["waves", "ripples", "spiral", "patchwork"]
    );
    assert_eq!(
        read["palettes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value["values"].as_array().unwrap().len())
            .collect::<Vec<_>>(),
        [5, 5, 5, 5]
    );
    assert_eq!(read["artworks"], serde_json::json!([]));

    let first = harness
        .request(
            "tutorial.generate",
            serde_json::json!({"algorithm": "waves", "palette": "garden"}),
        )
        .await
        .unwrap();
    let second = harness
        .request(
            "tutorial.generate",
            serde_json::json!({"algorithm": "waves", "palette": "garden"}),
        )
        .await
        .unwrap();
    assert_eq!(first["recipe"]["seed"], 7);
    assert_eq!(second["recipe"]["seed"], 91);
    assert_ne!(first["cells"], second["cells"]);

    let saved = harness
        .request(
            "tutorial.save",
            serde_json::json!({"recipe": first["recipe"]}),
        )
        .await
        .unwrap();
    assert_eq!(saved["id"], "artwork-1");
    assert_eq!(saved["cells"], first["cells"]);

    let invalid = harness
        .request(
            "tutorial.generate",
            serde_json::json!({"algorithm": "unknown", "palette": "garden"}),
        )
        .await
        .unwrap_err();
    assert_eq!(invalid.code(), "tutorial.unknown_algorithm");
    assert!(!invalid.retryable());

    let malformed = harness
        .request(
            "tutorial.generate",
            serde_json::json!({"algorithm": "waves", "palette": "garden", "extra": 1}),
        )
        .await
        .unwrap_err();
    assert_eq!(malformed.code(), "package.invalid_payload");
    assert!(!malformed.retryable());

    let unknown = harness
        .request("tutorial.delete", empty())
        .await
        .unwrap_err();
    assert_eq!(unknown.code(), "package.unknown_operation");
    assert!(!unknown.retryable());

    harness.suspend().await.unwrap();
    harness.dispose().await.unwrap();
    let disposed = harness.request("tutorial.read", empty()).await.unwrap_err();
    assert_eq!(disposed.code(), "package.not_active");
    assert!(disposed.retryable());

    harness.recover().await.unwrap();
    let recovered = harness.request("tutorial.read", empty()).await.unwrap();
    assert_eq!(recovered["artworks"][0]["id"], "artwork-1");
    harness.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn saved_artwork_survives_a_replacement_generation() {
    let root = tempfile::tempdir().unwrap();
    let mut harness = start(root.path(), [7]).await;
    harness.activate().await.unwrap();
    let draft = harness
        .request(
            "tutorial.generate",
            serde_json::json!({"algorithm": "ripples", "palette": "ocean"}),
        )
        .await
        .unwrap();
    harness
        .request(
            "tutorial.save",
            serde_json::json!({"recipe": draft["recipe"]}),
        )
        .await
        .unwrap();
    harness.shutdown().await.unwrap();

    // A new process generation recovers the same Data Directory.
    let mut replacement = start(root.path(), [7]).await;
    replacement.recover().await.unwrap();
    let read = replacement.request("tutorial.read", empty()).await.unwrap();
    assert_eq!(read["artworks"][0]["id"], "artwork-1");
    assert_eq!(read["artworks"][0]["recipe"]["algorithm"], "ripples");
    replacement.shutdown().await.unwrap();
}

#[test]
fn router_matches_the_ui_contract() {
    assert_router_matches_contract(
        &router().operations(),
        include_str!("../../ui-contract.json"),
    );
}
