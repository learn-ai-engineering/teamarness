//! Installation-local Workspace rename (ADR 0049).

use std::fs;
#[path = "../test-support/test_directory.rs"]
mod test_directory;
use std::str::FromStr;
use test_directory::TestDirectory;

use teamarness_core::{CoreRuntime, CoreRuntimeConfig, WorkspaceError, WorkspaceId};

fn private_data_directory(test_name: &str) -> TestDirectory {
    TestDirectory::new(test_name)
}

/// Verifies: TEAM.SYS.SHELL-R04. A rename changes only this Installation's stored name, keeps
/// every other Workspace fact, survives reopening (readiness verification still passes), and
/// refuses invalid names and unknown Workspaces without changing anything.
#[tokio::test]
async fn rename_is_local_durable_and_validated() {
    let directory = private_data_directory("workspace-rename");
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open_loopback_for_tests(config.clone())
        .await
        .unwrap();
    let created = runtime.create_workspace("Test bench").unwrap();

    let renamed = runtime
        .rename_workspace(created.id(), "Scratch space")
        .unwrap();
    assert_eq!(renamed.name(), "Scratch space");
    assert_eq!(renamed.id(), created.id());
    assert_eq!(renamed.created_by(), created.created_by());
    assert_eq!(renamed.created_at_unix_ms(), created.created_at_unix_ms());
    assert_eq!(runtime.list_workspaces().unwrap(), vec![renamed.clone()]);

    assert_eq!(
        runtime.rename_workspace(created.id(), " padded"),
        Err(WorkspaceError::InvalidName)
    );
    assert_eq!(
        runtime.rename_workspace(
            WorkspaceId::from_str("07070707070707070707070707070707").unwrap(),
            "Elsewhere"
        ),
        Err(WorkspaceError::UnknownWorkspace)
    );
    assert_eq!(runtime.list_workspaces().unwrap(), vec![renamed.clone()]);

    runtime.shutdown().await.unwrap();
    drop(runtime);
    let reopened = CoreRuntime::open_loopback_for_tests(config).await.unwrap();
    assert_eq!(reopened.list_workspaces().unwrap(), vec![renamed]);
    reopened.shutdown().await.unwrap();
    drop(reopened);
    fs::remove_dir_all(directory).unwrap();
}

/// Verifies: TEAM.SYS.SHELL-R04. After both sides rename, a joiner's exact retry of its
/// invitation still recovers the same admission, and each Installation keeps its own name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn renamed_workspaces_still_recover_a_join_retry() {
    let root = private_data_directory("workspace-rename-join");
    let issuer = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(root.join("issuer")))
        .await
        .unwrap();
    let joiner = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(root.join("joiner")))
        .await
        .unwrap();
    issuer.remember_loopback_peer_for_tests(&joiner);
    joiner.remember_loopback_peer_for_tests(&issuer);
    issuer.pause_sync_scheduler_for_tests(true);
    joiner.pause_sync_scheduler_for_tests(true);
    let workspace = issuer.create_workspace("Shared").unwrap().id();
    let invitation = issuer.create_workspace_invitation(workspace).await.unwrap();
    let (summary, joined) = joiner.join_workspace(&invitation).await.unwrap();
    assert_eq!(summary.name(), "Shared");

    issuer.rename_workspace(workspace, "Ours").unwrap();
    joiner.rename_workspace(workspace, "Mine").unwrap();
    let (_, retried) = joiner.join_workspace(&invitation).await.unwrap();
    assert_eq!(retried, joined);
    assert_eq!(joiner.list_workspaces().unwrap()[0].name(), "Mine");
    assert_eq!(issuer.list_workspaces().unwrap()[0].name(), "Ours");

    issuer.shutdown().await.unwrap();
    joiner.shutdown().await.unwrap();
    drop(issuer);
    drop(joiner);
    fs::remove_dir_all(root).unwrap();
}
