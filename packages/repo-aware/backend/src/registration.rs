use std::path::Path;

use teamarness_package_sdk::CancellationToken;

use crate::error::{self, ErrorInfo};

/// Accepts one absolute path to an existing directory, and returns it canonicalized.
///
/// TEAM.SYS.REPO-R02: a relative path is refused **before** any filesystem call, so a relative
/// name is never resolved against whatever working directory the Backend happens to have.
///
/// # Errors
///
/// Returns `path_missing`, `path_not_absolute`, `path_unavailable` or `not_directory`.
pub fn validate_path(path: &str) -> Result<String, ErrorInfo> {
    if path.is_empty() {
        return Err(error::PATH_MISSING);
    }
    if !Path::new(path).is_absolute() {
        return Err(error::PATH_NOT_ABSOLUTE);
    }

    let canonical = std::fs::canonicalize(path).map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => error::PATH_MISSING,
        _ => error::PATH_UNAVAILABLE,
    })?;

    if !canonical.is_dir() {
        return Err(error::NOT_DIRECTORY);
    }

    Ok(canonical.to_string_lossy().to_string())
}

/// Resolves one registration request to its checkout root, stopping if the request is cancelled.
///
/// TEAM.SYS.REPO-R02: the working-tree root is found **first**, and the checkout is then validated
/// at that root. `validate_git_checkout` uses `gix::open`, which accepts only a work tree or a git
/// directory itself; validating the caller's path would refuse every subdirectory, which is exactly
/// the input the requirement says to accept. `git_root` searches upward, so a subdirectory — at any
/// depth — resolves to the root that is then validated.
///
/// Every step is a filesystem call, so the cancellation flag is checked between them: a Package UI
/// that walks away never leaves a worker inspecting a repository nobody is waiting for.
///
/// # Errors
///
/// Returns the first validation failure, or `snapshot_interrupted` when the request was cancelled.
pub fn resolve_registration(path: &str, cancel: &CancellationToken) -> Result<String, ErrorInfo> {
    let canonical = validate_path(path)?;
    if cancel.is_cancelled() {
        return Err(error::SNAPSHOT_INTERRUPTED);
    }
    let root = git_root(&canonical)?;
    if cancel.is_cancelled() {
        return Err(error::SNAPSHOT_INTERRUPTED);
    }
    validate_git_checkout(&root)?;
    Ok(root)
}

pub fn validate_git_checkout(path: &str) -> Result<(), ErrorInfo> {
    let repo = gix::open(path).map_err(|_| error::NOT_GIT_REPOSITORY)?;

    repo.workdir().ok_or(error::NOT_GIT_REPOSITORY)?;

    let mut head = repo.head().map_err(|_| error::NOT_GIT_REPOSITORY)?;
    if head.peel_to_commit().is_err() {
        return Err(error::REPOSITORY_UNBORN);
    }

    Ok(())
}

pub fn git_root(path: &str) -> Result<String, ErrorInfo> {
    let repo = gix::discover(path).map_err(|_| error::NOT_GIT_REPOSITORY)?;
    repo.workdir()
        .map(|p| {
            std::fs::canonicalize(p)
                .unwrap_or_else(|_| p.to_path_buf())
                .to_string_lossy()
                .to_string()
        })
        .ok_or(error::NOT_GIT_REPOSITORY)
}
