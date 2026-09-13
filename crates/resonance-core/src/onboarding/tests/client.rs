use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::{CoreRuntime, CoreRuntimeConfig, OnboardingError};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn directory(name: &str) -> std::path::PathBuf {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "resonance-client-{name}-{}-{sequence}",
        std::process::id()
    ))
}

fn now_unix_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

fn workspace_rows(directory: &std::path::Path) -> i64 {
    Connection::open(directory.join("core.sqlite3"))
        .unwrap()
        .query_row("SELECT COUNT(*) FROM workspaces", [], |row| row.get(0))
        .unwrap()
}

#[tokio::test]
async fn authenticated_client_uses_one_exchange_and_returns_only_after_durable_import() {
    let issuer_directory = directory("success-issuer");
    let joiner_directory = directory("success-joiner");
    let issuer = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&issuer_directory))
        .await
        .unwrap();
    let joiner = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&joiner_directory))
        .await
        .unwrap();
    let workspace = issuer.create_workspace("Network join").unwrap();
    let invitation = issuer
        .create_workspace_invitation(workspace.id())
        .await
        .unwrap();

    let (joined, snapshot) = joiner.join_workspace(&invitation).await.unwrap();
    assert_eq!(joined, workspace);
    assert_eq!(workspace_rows(&joiner_directory), 1);
    assert_eq!(
        snapshot,
        joiner.membership_snapshot(workspace.id()).unwrap()
    );
    assert_eq!(snapshot.members().len(), 2);

    issuer.shutdown().await.unwrap();
    joiner.shutdown().await.unwrap();
    drop((issuer, joiner));
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(joiner_directory).unwrap();
}

#[tokio::test]
async fn stable_remote_expiry_error_leaves_no_local_workspace_rows() {
    let issuer_directory = directory("expired-issuer");
    let joiner_directory = directory("expired-joiner");
    let issuer = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&issuer_directory))
        .await
        .unwrap();
    let joiner = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&joiner_directory))
        .await
        .unwrap();
    let workspace = issuer.create_workspace("Expired network join").unwrap();
    let now = now_unix_ms();
    let invitation = issuer
        .create_workspace_invitation_for_test(workspace.id(), now - 10_000, now - 1)
        .unwrap();

    assert_eq!(
        joiner.join_workspace(&invitation).await.unwrap_err(),
        OnboardingError::Expired
    );
    assert_eq!(workspace_rows(&joiner_directory), 0);

    issuer.shutdown().await.unwrap();
    joiner.shutdown().await.unwrap();
    drop((issuer, joiner));
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(joiner_directory).unwrap();
}
