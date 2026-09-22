use std::fs;

use teamarness_package_sdk::CancellationToken;
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

/// The clause is exercised through `resolve_registration`, which is the only path a
/// `repo_aware.register` request takes. A test that calls `git_root` directly cannot see the
/// ordering, because the step that refuses a subdirectory is the one it skips.
// Verifies: TEAM.SYS.REPO-R02
#[test]
fn register_normalizes_a_subdirectory_to_the_working_tree_root() {
    let git_root = tempfile::tempdir().unwrap();
    let git_path = git_root.path().join("my-repo");
    init_git_repo(&git_path);
    let canonical_git = std::fs::canonicalize(&git_path)
        .unwrap()
        .to_string_lossy()
        .to_string();

    let nested = git_path.join("src").join("deep");
    fs::create_dir_all(&nested).unwrap();

    for candidate in [git_path.join("src"), nested] {
        assert_eq!(
            registration::resolve_registration(
                candidate.to_string_lossy().as_ref(),
                &CancellationToken::new(),
            ),
            Ok(canonical_git.clone()),
            "registering {candidate:?} must resolve to the working-tree root"
        );
    }
}

/// The reordering that satisfies the subdirectory clause must not widen what counts as a
/// checkout: each rejection keeps the code the Package UI already reports.
// Verifies: TEAM.SYS.REPO-R02
#[test]
fn register_still_refuses_everything_that_is_not_a_committed_checkout() {
    let cancel = CancellationToken::new();
    let root = tempfile::tempdir().unwrap();

    let plain = root.path().join("plain");
    fs::create_dir(&plain).unwrap();
    assert_eq!(
        registration::resolve_registration(plain.to_string_lossy().as_ref(), &cancel),
        Err(error::NOT_GIT_REPOSITORY)
    );

    let unborn = root.path().join("unborn");
    fs::create_dir(&unborn).unwrap();
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(&unborn)
        .output()
        .unwrap();
    assert_eq!(
        registration::resolve_registration(unborn.to_string_lossy().as_ref(), &cancel),
        Err(error::REPOSITORY_UNBORN)
    );

    let file = root.path().join("a-file");
    fs::write(&file, b"contents").unwrap();
    assert_eq!(
        registration::resolve_registration(file.to_string_lossy().as_ref(), &cancel),
        Err(error::NOT_DIRECTORY)
    );

    assert_eq!(
        registration::resolve_registration(
            root.path().join("absent").to_string_lossy().as_ref(),
            &cancel,
        ),
        Err(error::PATH_MISSING)
    );
    assert_eq!(
        registration::resolve_registration("", &cancel),
        Err(error::PATH_MISSING)
    );

    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let git_path = root.path().join("my-repo");
    init_git_repo(&git_path);
    assert_eq!(
        registration::resolve_registration(git_path.to_string_lossy().as_ref(), &cancelled),
        Err(error::SNAPSHOT_INTERRUPTED)
    );
}

#[test]
fn reject_relative_missing_path() {
    assert_eq!(
        registration::validate_path("relative/path"),
        Err(error::PATH_NOT_ABSOLUTE)
    );
}

/// A relative path is refused, never resolved against whatever working directory the Backend
/// happens to have — checked on the real entry point, not only on the validator.
// Verifies: TEAM.SYS.REPO-R02
#[test]
fn register_rejects_an_existing_relative_path_without_resolving_it() {
    // The test's working directory is the crate directory, where `src` exists. A relative path
    // that happens to resolve is still refused, and it is refused before any filesystem call.
    assert!(std::path::Path::new("src").is_dir());
    assert_eq!(
        registration::validate_path("src"),
        Err(error::PATH_NOT_ABSOLUTE)
    );
    assert_eq!(
        registration::resolve_registration("src", &CancellationToken::new()),
        Err(error::PATH_NOT_ABSOLUTE)
    );
}

#[cfg(unix)]
#[test]
fn permission_denied_is_path_unavailable() {
    use std::os::unix::fs::PermissionsExt;

    if unsafe { libc_geteuid() } == 0 {
        return; // root reads everything, so the case cannot be produced
    }
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("locked");
    let child = parent.join("inside");
    fs::create_dir_all(&child).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o000)).unwrap();

    let result = registration::validate_path(child.to_string_lossy().as_ref());

    fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(result, Err(error::PATH_UNAVAILABLE));
}

#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "geteuid"]
    fn libc_geteuid() -> u32;
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

/// Verifies: TEAM.SYS.REPO-R03
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
