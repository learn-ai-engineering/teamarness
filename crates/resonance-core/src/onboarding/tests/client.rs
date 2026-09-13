use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

fn block_workspace_import(directory: &std::path::Path) -> Connection {
    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    connection
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
async fn outbound_join_accounting_covers_post_response_work_and_shutdown() {
    let issuer_directory = directory("outbound-accounting-issuer");
    let joiner_directory = directory("outbound-accounting-joiner");
    let issuer = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&issuer_directory))
        .await
        .unwrap();
    let joiner = std::sync::Arc::new(
        CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&joiner_directory))
            .await
            .unwrap(),
    );
    let held_workspace = issuer.create_workspace("Held network join").unwrap();
    let other_workspace = issuer.create_workspace("Other network join").unwrap();
    let held_invitation = issuer
        .create_workspace_invitation(held_workspace.id())
        .await
        .unwrap();
    let other_invitation = issuer
        .create_workspace_invitation(other_workspace.id())
        .await
        .unwrap();

    joiner.hold_next_join_after_response_for_test();
    let joining_runtime = std::sync::Arc::clone(&joiner);
    let joining =
        tokio::spawn(async move { joining_runtime.join_workspace(&held_invitation).await });
    joiner.wait_for_join_held_after_response_for_test().await;

    assert_eq!(joiner.active_onboarding_operations_for_test(), 1);
    assert_eq!(
        joiner.available_outbound_joins_for_test(),
        crate::onboarding::transport::MAX_OUTBOUND_JOINS - 1
    );
    let mut other_permits = Vec::new();
    for _ in 1..crate::onboarding::transport::MAX_OUTBOUND_JOINS {
        other_permits.push(joiner.hold_outbound_join_for_test().await.unwrap());
    }
    assert_eq!(
        joiner.join_workspace(&other_invitation).await.unwrap_err(),
        OnboardingError::Overload
    );
    drop(other_permits);

    let closing_runtime = std::sync::Arc::clone(&joiner);
    let mut shutdown = tokio::spawn(async move { closing_runtime.shutdown().await });
    tokio::time::timeout(Duration::from_secs(2), async {
        tokio::select! {
            result = &mut shutdown => {
                panic!("shutdown completed before the post-response join: {result:?}");
            }
            () = joiner.wait_for_shutdown_idle_waiter_for_test() => {}
        }
    })
    .await
    .expect("shutdown did not wait for the active public join");

    joiner.release_join_held_after_response_for_test();
    joining.await.unwrap().unwrap();
    shutdown.await.unwrap().unwrap();
    issuer.shutdown().await.unwrap();
    drop(issuer);
    let joiner = std::sync::Arc::try_unwrap(joiner).expect("join tasks released runtime");
    drop(joiner);
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(joiner_directory).unwrap();
}

#[tokio::test]
async fn post_response_work_obeys_the_whole_join_budget() {
    let issuer_directory = directory("post-response-timeout-issuer");
    let joiner_directory = directory("post-response-timeout-joiner");
    let issuer = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&issuer_directory))
        .await
        .unwrap();
    let joiner = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&joiner_directory))
        .await
        .unwrap();
    let workspace = issuer.create_workspace("Timed network join").unwrap();
    let invitation = issuer
        .create_workspace_invitation(workspace.id())
        .await
        .unwrap();
    joiner.pause_next_join_after_response_for_test();

    let result = tokio::time::timeout(
        Duration::from_secs(2),
        joiner.join_workspace_with_budget_for_test(&invitation, Duration::from_millis(25)),
    )
    .await
    .expect("join operation outlived its whole-operation budget");
    assert_eq!(result.unwrap_err(), OnboardingError::Timeout);
    assert_eq!(workspace_rows(&joiner_directory), 0);

    issuer.shutdown().await.unwrap();
    joiner.shutdown().await.unwrap();
    drop((issuer, joiner));
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(joiner_directory).unwrap();
}

#[tokio::test]
async fn synchronous_post_response_validation_respects_the_deadline() {
    let issuer_directory = directory("synchronous-validation-timeout-issuer");
    let joiner_directory = directory("synchronous-validation-timeout-joiner");
    let issuer = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&issuer_directory))
        .await
        .unwrap();
    let joiner = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&joiner_directory))
        .await
        .unwrap();
    let workspace = issuer
        .create_workspace("Synchronous validation join")
        .unwrap();
    let invitation = issuer
        .create_workspace_invitation(workspace.id())
        .await
        .unwrap();
    joiner.block_next_join_after_response_for_test(Duration::from_secs(2));

    let started = std::time::Instant::now();
    let result = joiner
        .join_workspace_with_budget_for_test(&invitation, Duration::from_secs(1))
        .await;
    let elapsed = started.elapsed();

    assert_eq!(result.unwrap_err(), OnboardingError::Timeout);
    assert!(
        elapsed < Duration::from_millis(1_500),
        "synchronous validation prevented deadline enforcement: {elapsed:?}"
    );
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert_eq!(workspace_rows(&joiner_directory), 0);

    issuer.shutdown().await.unwrap();
    joiner.shutdown().await.unwrap();
    drop((issuer, joiner));
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(joiner_directory).unwrap();
}

#[tokio::test]
async fn blocked_sqlite_import_obeys_remaining_whole_join_budget() {
    let issuer_directory = directory("blocked-import-timeout-issuer");
    let joiner_directory = directory("blocked-import-timeout-joiner");
    let issuer = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&issuer_directory))
        .await
        .unwrap();
    let joiner = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&joiner_directory))
        .await
        .unwrap();
    let workspace = issuer.create_workspace("Blocked import join").unwrap();
    let invitation = issuer
        .create_workspace_invitation(workspace.id())
        .await
        .unwrap();
    let blocker = block_workspace_import(&joiner_directory);

    let started = std::time::Instant::now();
    let result = joiner
        .join_workspace_with_budget_for_test(&invitation, Duration::from_millis(500))
        .await;
    let elapsed = started.elapsed();
    blocker.execute_batch("ROLLBACK").unwrap();

    assert_eq!(result.unwrap_err(), OnboardingError::Timeout);
    assert!(
        elapsed < Duration::from_secs(2),
        "blocked import exceeded the whole-operation deadline: {elapsed:?}"
    );
    assert_eq!(workspace_rows(&joiner_directory), 0);

    issuer.shutdown().await.unwrap();
    joiner.shutdown().await.unwrap();
    drop((issuer, joiner, blocker));
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
