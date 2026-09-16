use std::fs;

use teamarness_repo_aware_package::error;
use teamarness_repo_aware_package::registration;
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

#[test]
fn register_valid_checkout() {
    let git_root = tempfile::tempdir().unwrap();
    let git_path = git_root.path().join("my-repo");
    init_git_repo(&git_path);

    let data_dir = tempfile::tempdir().unwrap();
    let mut store = StateStore::open(data_dir.path()).unwrap();
    let canonical = std::fs::canonicalize(&git_path).unwrap();
    let canonical_str = canonical.to_string_lossy().to_string();

    let validated = registration::validate_path(&canonical_str).unwrap();
    assert_eq!(validated, canonical_str);
    let root = registration::git_root(&validated).unwrap();
    assert_eq!(root, canonical_str);

    let registration = store.register(&root).unwrap();
    assert!(!registration.id.is_empty());
    assert_eq!(registration.path, root);
    assert!(!registration.added_at.is_empty());
    assert_eq!(store.registrations().len(), 1);
}

#[test]
fn normalize_subdirectory_to_root() {
    let git_root = tempfile::tempdir().unwrap();
    let git_path = git_root.path().join("my-repo");
    init_git_repo(&git_path);

    let subdir = git_path.join("src");
    fs::create_dir(&subdir).unwrap();

    let canonical = registration::validate_path(subdir.to_string_lossy().as_ref()).unwrap();
    // The canonical path is the subdirectory
    let canonical_git = std::fs::canonicalize(&git_path).unwrap();
    assert_eq!(canonical, std::fs::canonicalize(&subdir).unwrap().to_string_lossy());

    // Git root should be the checkout root
    let root = registration::git_root(&canonical).unwrap();
    assert_eq!(root, canonical_git.to_string_lossy());
}

#[test]
fn reject_relative_path() {
    let result = registration::validate_path("relative/path");
    assert!(result.is_err());
}

#[test]
fn reject_missing_path() {
    let result = registration::validate_path("/nonexistent/path/that/does/not/exist");
    assert!(result.is_err());
}

#[test]
fn reject_non_git_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().to_string();
    let validated = registration::validate_path(&path).unwrap();
    let result = registration::validate_git_checkout(&validated);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, error::NOT_GIT_REPOSITORY.code);
}

#[test]
fn reject_unborn_repository() {
    let git_root = tempfile::tempdir().unwrap();
    let git_path = git_root.path().join("unborn");
    fs::create_dir(&git_path).unwrap();
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(&git_path)
        .output()
        .unwrap();

    let validated = registration::validate_path(git_path.to_string_lossy().as_ref()).unwrap();
    let result = registration::validate_git_checkout(&validated);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, error::REPOSITORY_UNBORN.code);
}

#[test]
fn reject_duplicate_canonical_path() {
    let git_root = tempfile::tempdir().unwrap();
    let git_path = git_root.path().join("my-repo");
    init_git_repo(&git_path);

    let data_dir = tempfile::tempdir().unwrap();
    let mut store = StateStore::open(data_dir.path()).unwrap();
    let canonical = std::fs::canonicalize(&git_path).unwrap();
    let canonical_str = canonical.to_string_lossy().to_string();

    store.register(&canonical_str).unwrap();
    let result = store.register(&canonical_str);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, error::DUPLICATE_REGISTRATION.code);
}

#[test]
fn remove_registration() {
    let git_root = tempfile::tempdir().unwrap();
    let git_path = git_root.path().join("my-repo");
    init_git_repo(&git_path);

    let data_dir = tempfile::tempdir().unwrap();
    let mut store = StateStore::open(data_dir.path()).unwrap();
    let canonical = std::fs::canonicalize(&git_path).unwrap();
    let canonical_str = canonical.to_string_lossy().to_string();

    let registration = store.register(&canonical_str).unwrap();
    assert_eq!(store.registrations().len(), 1);
    store.remove(&registration.id).unwrap();
    assert!(store.registrations().is_empty());
}

#[test]
fn remove_unknown_registration_fails() {
    let data_dir = tempfile::tempdir().unwrap();
    let mut store = StateStore::open(data_dir.path()).unwrap();

    let result = store.remove("nonexistent-id");
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, error::REGISTRATION_NOT_FOUND.code);
}

#[test]
fn state_survives_reopen() {
    let git_root = tempfile::tempdir().unwrap();
    let git_path = git_root.path().join("my-repo");
    init_git_repo(&git_path);

    let data_dir = tempfile::tempdir().unwrap();
    let canonical = std::fs::canonicalize(&git_path).unwrap();
    let canonical_str = canonical.to_string_lossy().to_string();

    // First session
    let mut store_a = StateStore::open(data_dir.path()).unwrap();
    let registration = store_a.register(&canonical_str).unwrap();
    let first_id = registration.id;
    drop(store_a);

    // Second session (simulate restart)
    let store_b = StateStore::open(data_dir.path()).unwrap();
    let registrations = store_b.registrations();
    assert_eq!(registrations.len(), 1);
    assert_eq!(registrations[0].id, first_id);
    assert_eq!(registrations[0].path, canonical_str);
}

#[test]
fn open_initializes_empty_state() {
    let data_dir = tempfile::tempdir().unwrap();
    let store = StateStore::open(data_dir.path()).unwrap();
    assert!(store.registrations().is_empty());
    assert!(data_dir.path().join("repo-aware.v1.json").is_file());
}

#[test]
fn lifecycle_state_persistence() {
    let data_dir = tempfile::tempdir().unwrap();
    let mut store = StateStore::open(data_dir.path()).unwrap();
    let git_root = tempfile::tempdir().unwrap();
    let git_path = git_root.path().join("repo");
    init_git_repo(&git_path);
    let canonical = std::fs::canonicalize(&git_path).unwrap();
    store.register(&canonical.to_string_lossy()).unwrap();
    drop(store);
    // Reopen and verify
    let reopened = StateStore::open(data_dir.path()).unwrap();
    assert_eq!(reopened.registrations().len(), 1);
}