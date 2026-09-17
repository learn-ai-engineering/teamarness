use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;
use serde::Serialize;

use crate::error::{self, ErrorInfo};
use crate::state::StateStore;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotHead {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub registration_id: String,
    pub path: String,
    pub origin_remote: Option<String>,
    pub github_url: Option<String>,
    pub head: SnapshotHead,
    pub commit: String,
    pub is_clean: bool,
    pub fetched_at: String,
}

/// Normalize a Git remote URL to a GitHub HTTPS URL, or return `None` for
/// unrecognized URLs.
pub fn normalize_github_url(origin: Option<&str>) -> Option<String> {
    let url = origin?;

    // https://github.com/owner/repo.git -> https://github.com/owner/repo
    if let Some(rest) = url.strip_prefix("https://github.com/") {
        return Some(format!(
            "https://github.com/{}",
            rest.strip_suffix(".git").unwrap_or(rest)
        ));
    }

    // git@github.com:owner/repo.git -> https://github.com/owner/repo
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        return Some(format!(
            "https://github.com/{}",
            rest.strip_suffix(".git").unwrap_or(rest)
        ));
    }

    // ssh://git@github.com/owner/repo.git -> https://github.com/owner/repo
    if let Some(rest) = url.strip_prefix("ssh://git@github.com/") {
        return Some(format!(
            "https://github.com/{}",
            rest.strip_suffix(".git").unwrap_or(rest)
        ));
    }

    None
}

struct SnapshotDropGuard {
    cancelled: Arc<AtomicBool>,
}

impl SnapshotDropGuard {
    fn new(cancelled: Arc<AtomicBool>) -> Self {
        Self { cancelled }
    }
}

impl Drop for SnapshotDropGuard {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

pub fn compute_snapshot(
    registration_id: &str,
    store: &StateStore,
    cancel: Arc<AtomicBool>,
) -> Result<Snapshot, ErrorInfo> {
    let registration = store
        .registrations()
        .into_iter()
        .find(|r| r.id == registration_id)
        .ok_or(error::REGISTRATION_NOT_FOUND)?;
    compute_snapshot_impl(registration_id, registration.path.as_str(), cancel)
}

/// Core snapshot computation — I/O-bound gix work. Call inside
/// `tokio::task::spawn_blocking` when invoked from an async context.
pub fn compute_snapshot_impl(
    registration_id: &str,
    path: &str,
    cancel: Arc<AtomicBool>,
) -> Result<Snapshot, ErrorInfo> {
    let path_ref = Path::new(path);

    if !path_ref.exists() {
        return Err(error::REPOSITORY_UNAVAILABLE);
    }

    let repo = gix::open(path).map_err(|_| error::NOT_GIT_REPOSITORY)?;

    // Read origin remote URL from config snapshot
    let origin_remote = repo
        .config_snapshot()
        .string("remote.origin.url")
        .map(|s| s.to_string());

    let github_url = normalize_github_url(origin_remote.as_deref());

    // Resolve HEAD
    let mut head_ref = repo.head().map_err(|_| error::NOT_GIT_REPOSITORY)?;

    let (head, commit_sha) =
        if let Some(branch_name) = head_ref.referent_name().map(|n| n.to_string()) {
            // Branch — use the short branch name (e.g., "main" from "refs/heads/main")
            let short_name = branch_name
                .strip_prefix("refs/heads/")
                .unwrap_or(&branch_name)
                .to_string();
            let commit = head_ref
                .peel_to_commit()
                .map_err(|_| error::NOT_GIT_REPOSITORY)?;
            let sha = commit.id().to_string();
            (
                SnapshotHead {
                    kind: "branch".into(),
                    name: Some(short_name),
                    commit: None,
                },
                sha,
            )
        } else {
            // Detached HEAD
            let commit = head_ref
                .peel_to_commit()
                .map_err(|_| error::NOT_GIT_REPOSITORY)?;
            let sha = commit.id().to_string();
            (
                SnapshotHead {
                    kind: "detached".into(),
                    name: None,
                    commit: Some(sha.clone()),
                },
                sha,
            )
        };

    // Compute status with cooperative cancellation
    let _guard = SnapshotDropGuard::new(cancel.clone());

    let platform = repo
        .status(gix::progress::Discard)
        .map_err(|_| error::NOT_GIT_REPOSITORY)?
        .should_interrupt_owned(cancel.clone());

    // Collect status entries. An empty patterns list checks the full worktree.
    let empty_patterns: Vec<gix::bstr::BString> = Vec::new();
    let status_iter = platform.into_iter(empty_patterns);

    let status_count = match status_iter {
        Ok(iter) => {
            let mut count = 0;
            for item in iter {
                if cancel.load(Ordering::SeqCst) {
                    return Err(error::SNAPSHOT_INTERRUPTED);
                }
                if item.is_ok() {
                    count += 1;
                }
            }
            count
        }
        Err(_) => return Err(error::NOT_GIT_REPOSITORY),
    };

    if cancel.load(Ordering::SeqCst) {
        return Err(error::SNAPSHOT_INTERRUPTED);
    }

    let is_clean = status_count == 0;
    let fetched_at = Utc::now().to_rfc3339();

    Ok(Snapshot {
        registration_id: registration_id.to_owned(),
        path: path.to_owned(),
        origin_remote,
        github_url,
        head,
        commit: commit_sha,
        is_clean,
        fetched_at,
    })
}
