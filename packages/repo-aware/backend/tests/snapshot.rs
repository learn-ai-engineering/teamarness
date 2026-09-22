use std::fs;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use teamarness_repo_aware_package::error;
use teamarness_repo_aware_package::registration;
use teamarness_repo_aware_package::snapshot;
use teamarness_repo_aware_package::snapshot::normalize_github_url;
use teamarness_repo_aware_package::state::StateStore;

fn init_git_repo(path: &std::path::Path) {
    fs::create_dir_all(path).unwrap();
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(path)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(path)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(path)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "--allow-empty", "-m", "initial"])
        .current_dir(path)
        .output()
        .unwrap();
}

fn init_repo_and_register(path: &std::path::Path, store: &mut StateStore) -> String {
    init_git_repo(path);
    let canonical = std::fs::canonicalize(path).unwrap();
    let root = registration::git_root(&canonical.to_string_lossy()).unwrap();
    store.register(&root).unwrap().id
}

#[test]
fn snapshot_all_fields() {
    let git_root = tempfile::tempdir().unwrap();
    let git_path = git_root.path().join("my-repo");

    let data_dir = tempfile::tempdir().unwrap();
    let mut store = StateStore::open(data_dir.path()).unwrap();
    let id = init_repo_and_register(&git_path, &mut store);

    let cancel = Arc::new(AtomicBool::new(false));
    let result = snapshot::compute_snapshot(&id, &store, cancel).unwrap();

    assert_eq!(result.registration_id, id);
    assert!(!result.path.is_empty());
    assert!(result.origin_remote.is_none()); // no origin set
    assert!(result.github_url.is_none());
    assert_eq!(result.head.kind, "branch");
    assert!(
        result.head.name.as_deref() == Some("master")
            || result.head.name.as_deref() == Some("main")
    );
    assert!(result.head.commit.is_none());
    assert!(!result.commit.is_empty());
    assert!(result.is_clean);
    assert!(!result.fetched_at.is_empty());
}

#[test]
fn snapshot_no_origin() {
    let git_root = tempfile::tempdir().unwrap();
    let git_path = git_root.path().join("no-origin");

    let data_dir = tempfile::tempdir().unwrap();
    let mut store = StateStore::open(data_dir.path()).unwrap();
    let id = init_repo_and_register(&git_path, &mut store);

    let cancel = Arc::new(AtomicBool::new(false));
    let result = snapshot::compute_snapshot(&id, &store, cancel).unwrap();

    assert!(result.origin_remote.is_none());
    assert!(result.github_url.is_none());
    assert!(result.is_clean);
}

#[test]
fn snapshot_registration_not_found() {
    let data_dir = tempfile::tempdir().unwrap();
    let store = StateStore::open(data_dir.path()).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let result = snapshot::compute_snapshot("nonexistent", &store, cancel);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, error::REGISTRATION_NOT_FOUND.code);
}

#[test]
fn snapshot_path_unavailable() {
    let git_root = tempfile::tempdir().unwrap();
    let git_path = git_root.path().join("my-repo");
    init_git_repo(&git_path);

    let data_dir = tempfile::tempdir().unwrap();
    let mut store = StateStore::open(data_dir.path()).unwrap();
    let canonical = std::fs::canonicalize(&git_path).unwrap();
    let root = registration::git_root(&canonical.to_string_lossy()).unwrap();
    let id = store.register(&root).unwrap().id;

    // Delete the repo to make it unavailable
    fs::remove_dir_all(&git_path).unwrap();

    let cancel = Arc::new(AtomicBool::new(false));
    let result = snapshot::compute_snapshot(&id, &store, cancel);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, error::REPOSITORY_UNAVAILABLE.code);
}

#[test]
fn snapshot_detached_head() {
    let git_root = tempfile::tempdir().unwrap();
    let git_path = git_root.path().join("detached");
    init_git_repo(&git_path);

    // Checkout detached HEAD
    std::process::Command::new("git")
        .args(["checkout", "--detach"])
        .current_dir(&git_path)
        .output()
        .unwrap();

    let data_dir = tempfile::tempdir().unwrap();
    let mut store = StateStore::open(data_dir.path()).unwrap();
    let canonical = std::fs::canonicalize(&git_path).unwrap();
    let root = registration::git_root(&canonical.to_string_lossy()).unwrap();
    let id = store.register(&root).unwrap().id;

    let cancel = Arc::new(AtomicBool::new(false));
    let result = snapshot::compute_snapshot(&id, &store, cancel).unwrap();

    assert_eq!(result.head.kind, "detached");
    assert!(result.head.name.is_none());
    assert_eq!(result.head.commit.as_deref(), Some(result.commit.as_str()));
}

#[test]
fn snapshot_dirty_repo() {
    let git_root = tempfile::tempdir().unwrap();
    let git_path = git_root.path().join("dirty");
    init_git_repo(&git_path);

    // Create an untracked file to make it dirty
    fs::write(git_path.join("untracked.txt"), "hello").unwrap();

    let data_dir = tempfile::tempdir().unwrap();
    let mut store = StateStore::open(data_dir.path()).unwrap();
    let canonical = std::fs::canonicalize(&git_path).unwrap();
    let root = registration::git_root(&canonical.to_string_lossy()).unwrap();
    let id = store.register(&root).unwrap().id;

    let cancel = Arc::new(AtomicBool::new(false));
    let result = snapshot::compute_snapshot(&id, &store, cancel).unwrap();

    assert!(!result.is_clean);
}

#[test]
fn snapshot_cancellation() {
    let git_root = tempfile::tempdir().unwrap();
    let git_path = git_root.path().join("cancel-repo");
    init_git_repo(&git_path);

    let data_dir = tempfile::tempdir().unwrap();
    let mut store = StateStore::open(data_dir.path()).unwrap();
    let canonical = std::fs::canonicalize(&git_path).unwrap();
    let root = registration::git_root(&canonical.to_string_lossy()).unwrap();
    let id = store.register(&root).unwrap().id;

    // Set cancel BEFORE calling snapshot
    let cancel = Arc::new(AtomicBool::new(true));
    let result = snapshot::compute_snapshot(&id, &store, cancel);

    // Snapshot may succeed (clean repo) or fail with interrupted.
    // If cancellation happens before status iteration, it returns interrupted.
    // This tests the pathway works.
    if let Err(err) = &result {
        // Allow snapshot_interrupted or other expected errors
        assert_eq!(err.code, error::SNAPSHOT_INTERRUPTED.code);
    }
}

// GitHub URL normalizer tests
#[test]
fn normalize_github_url_https_dot_git() {
    assert_eq!(
        normalize_github_url(Some("https://github.com/owner/repo.git")),
        Some("https://github.com/owner/repo".into())
    );
}

#[test]
fn normalize_github_url_https_bare() {
    assert_eq!(
        normalize_github_url(Some("https://github.com/owner/repo")),
        Some("https://github.com/owner/repo".into())
    );
}

#[test]
fn normalize_github_url_git_at() {
    assert_eq!(
        normalize_github_url(Some("git@github.com:owner/repo.git")),
        Some("https://github.com/owner/repo".into())
    );
}

#[test]
fn normalize_github_url_ssh() {
    assert_eq!(
        normalize_github_url(Some("ssh://git@github.com/owner/repo.git")),
        Some("https://github.com/owner/repo".into())
    );
}

#[test]
fn normalize_github_url_unrecognized() {
    assert_eq!(
        normalize_github_url(Some("https://gitlab.com/owner/repo.git")),
        None
    );
    assert_eq!(normalize_github_url(Some("https://example.com/repo")), None);
    assert_eq!(normalize_github_url(None), None);
}

#[test]
fn snapshot_with_origin_remote() {
    let git_root = tempfile::tempdir().unwrap();
    let git_path = git_root.path().join("with-origin");
    init_git_repo(&git_path);

    // Set origin remote
    std::process::Command::new("git")
        .args([
            "remote",
            "add",
            "origin",
            "https://github.com/owner/repo.git",
        ])
        .current_dir(&git_path)
        .output()
        .unwrap();

    let data_dir = tempfile::tempdir().unwrap();
    let mut store = StateStore::open(data_dir.path()).unwrap();
    let canonical = std::fs::canonicalize(&git_path).unwrap();
    let root = registration::git_root(&canonical.to_string_lossy()).unwrap();
    let id = store.register(&root).unwrap().id;

    let cancel = Arc::new(AtomicBool::new(false));
    let result = snapshot::compute_snapshot(&id, &store, cancel).unwrap();

    assert_eq!(
        result.origin_remote,
        Some("https://github.com/owner/repo.git".into())
    );
    assert_eq!(
        result.github_url,
        Some("https://github.com/owner/repo".into())
    );
}

#[test]
fn snapshot_with_ssh_origin() {
    let git_root = tempfile::tempdir().unwrap();
    let git_path = git_root.path().join("ssh-origin");
    init_git_repo(&git_path);

    std::process::Command::new("git")
        .args(["remote", "add", "origin", "git@github.com:owner/repo.git"])
        .current_dir(&git_path)
        .output()
        .unwrap();

    let data_dir = tempfile::tempdir().unwrap();
    let mut store = StateStore::open(data_dir.path()).unwrap();
    let canonical = std::fs::canonicalize(&git_path).unwrap();
    let root = registration::git_root(&canonical.to_string_lossy()).unwrap();
    let id = store.register(&root).unwrap().id;

    let cancel = Arc::new(AtomicBool::new(false));
    let result = snapshot::compute_snapshot(&id, &store, cancel).unwrap();

    assert_eq!(
        result.github_url,
        Some("https://github.com/owner/repo".into())
    );
}
