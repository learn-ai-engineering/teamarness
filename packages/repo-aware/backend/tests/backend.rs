//! Repo-aware through the SDK harness: registration, snapshots, and real cancellation.

use std::fs;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use teamarness_package_sdk::testing::{Harness, HarnessBuilder, assert_router_matches_contract};
use teamarness_repo_aware_package::router;
use teamarness_repo_aware_package::snapshot::SNAPSHOT_INTERRUPTIONS;

fn empty() -> serde_json::Value {
    serde_json::json!({})
}

fn git(path: &Path, arguments: &[&str]) {
    let output = std::process::Command::new("git")
        .args(arguments)
        .current_dir(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_git_repo(path: &Path) {
    fs::create_dir_all(path).unwrap();
    git(path, &["init"]);
    git(path, &["config", "user.email", "test@test.com"]);
    git(path, &["config", "user.name", "Test"]);
    git(path, &["commit", "--allow-empty", "-m", "initial"]);
}

/// A checkout with enough untracked files that a status walk takes real time.
fn init_large_repo(path: &Path, files: usize) {
    init_git_repo(path);
    let tree = path.join("tree");
    for bucket in 0..16 {
        fs::create_dir_all(tree.join(format!("bucket-{bucket}"))).unwrap();
    }
    for index in 0..files {
        fs::write(
            tree.join(format!("bucket-{}/file-{index}.txt", index % 16)),
            format!("contents of file {index}\n").repeat(8),
        )
        .unwrap();
    }
}

async fn start(directory: &Path) -> Harness {
    HarnessBuilder::new(router().into_backend())
        .package_identity("dev.teamarness.repo-aware")
        .data_directory(directory)
        .start()
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn register_list_snapshot_remove_round_trip() {
    let data = tempfile::tempdir().unwrap();
    let checkouts = tempfile::tempdir().unwrap();
    let repository = checkouts.path().join("my-repo");
    init_git_repo(&repository);
    let canonical = fs::canonicalize(&repository).unwrap();

    let mut harness = start(data.path()).await;
    harness.activate().await.unwrap();

    let registered = harness
        .request(
            "repo_aware.register",
            serde_json::json!({ "path": canonical.to_string_lossy() }),
        )
        .await
        .unwrap();
    let id = registered["registration"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        registered["registration"]["path"],
        canonical.to_string_lossy().as_ref()
    );

    let listed = harness.request("repo_aware.list", empty()).await.unwrap();
    assert_eq!(listed["registrations"].as_array().unwrap().len(), 1);

    let snapshot = harness
        .request(
            "repo_aware.snapshot",
            serde_json::json!({ "registrationId": id }),
        )
        .await
        .unwrap();
    assert_eq!(snapshot["registrationId"], id.as_str());
    assert_eq!(snapshot["snapshot"]["isClean"], true);
    assert_eq!(snapshot["snapshot"]["head"]["kind"], "branch");

    let removed = harness
        .request(
            "repo_aware.remove",
            serde_json::json!({ "registrationId": id }),
        )
        .await
        .unwrap();
    assert_eq!(removed["removed"], true);
    let listed = harness.request("repo_aware.list", empty()).await.unwrap();
    assert!(listed["registrations"].as_array().unwrap().is_empty());
    harness.shutdown().await.unwrap();
}

// Verifies: TEAM.SYS.REPO-R02
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn register_relative_path_returns_path_not_absolute_code() {
    let data = tempfile::tempdir().unwrap();
    let mut harness = start(data.path()).await;
    harness.activate().await.unwrap();

    let error = harness
        .request("repo_aware.register", serde_json::json!({ "path": "src" }))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "repo_aware.path_not_absolute");
    assert!(!error.retryable());
    harness.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_operation_uses_the_sdk_code() {
    let data = tempfile::tempdir().unwrap();
    let mut harness = start(data.path()).await;
    harness.activate().await.unwrap();

    let error = harness
        .request("repo_aware.health", empty())
        .await
        .unwrap_err();
    assert_eq!(error.code(), "package.unknown_operation");

    let malformed = harness
        .request(
            "repo_aware.register",
            serde_json::json!({ "path": "/tmp", "extra": 1 }),
        )
        .await
        .unwrap_err();
    assert_eq!(malformed.code(), "package.invalid_payload");
    harness.shutdown().await.unwrap();
}

/// Registers a repository large enough that its status walk is still running when cancelled.
async fn start_with_large_repository(data: &Path, repository: &Path) -> (Harness, String) {
    init_large_repo(repository, 3_000);
    let canonical = fs::canonicalize(repository).unwrap();
    let mut harness = start(data).await;
    harness.activate().await.unwrap();
    let registered = harness
        .request(
            "repo_aware.register",
            serde_json::json!({ "path": canonical.to_string_lossy() }),
        )
        .await
        .unwrap();
    let id = registered["registration"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (harness, id)
}

// Verifies: TEAM.SYS.REPO-R09
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_a_snapshot_interrupts_the_blocking_walk() {
    let data = tempfile::tempdir().unwrap();
    let checkouts = tempfile::tempdir().unwrap();
    let (mut harness, id) =
        start_with_large_repository(data.path(), &checkouts.path().join("large")).await;
    let before = SNAPSHOT_INTERRUPTIONS.load(Ordering::SeqCst);

    let pending = harness
        .start_request(
            "repo_aware.snapshot",
            serde_json::json!({ "registrationId": id }),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    pending.cancel().await;
    let error = pending.result().await.unwrap_err();
    assert_eq!(error.code(), "cancelled");

    // No worker is held: an ordinary request is answered while the walk unwinds.
    let started = Instant::now();
    harness.request("repo_aware.list", empty()).await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));

    // The walk itself observed the interrupt flag rather than running to completion unread.
    let deadline = Instant::now() + Duration::from_secs(2);
    while SNAPSHOT_INTERRUPTIONS.load(Ordering::SeqCst) == before && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        SNAPSHOT_INTERRUPTIONS.load(Ordering::SeqCst) > before,
        "the cancelled snapshot stopped its gix walk"
    );
    harness.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn suspend_interrupts_an_in_flight_snapshot() {
    let data = tempfile::tempdir().unwrap();
    let checkouts = tempfile::tempdir().unwrap();
    let (mut harness, id) =
        start_with_large_repository(data.path(), &checkouts.path().join("suspend")).await;

    let pending = harness
        .start_request(
            "repo_aware.snapshot",
            serde_json::json!({ "registrationId": id }),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    harness.suspend().await.unwrap();

    let error = pending.result().await.unwrap_err();
    assert_eq!(error.code(), "cancelled");
    harness.shutdown().await.unwrap();
}

#[test]
fn router_matches_the_ui_contract() {
    assert_router_matches_contract(
        &router().operations(),
        include_str!("../../ui-contract.json"),
    );
}
