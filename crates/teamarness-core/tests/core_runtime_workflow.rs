use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use teamarness_core::{CoreRuntime, CoreRuntimeConfig, MembershipEpoch, MembershipRole};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn private_data_directory() -> PathBuf {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "teamarness-core-workflow-{}-{sequence}",
        std::process::id()
    ))
}

#[tokio::test]
async fn public_runtime_local_workflow_is_network_independent_and_survives_reopen() {
    let directory = private_data_directory();
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open_loopback_for_tests(config.clone())
        .await
        .unwrap();
    let identity = runtime.public_identity();

    let workspace = runtime.create_workspace("Acceptance Workspace").unwrap();
    assert_eq!(workspace.created_by(), identity);
    assert_eq!(runtime.list_workspaces().unwrap(), vec![workspace.clone()]);

    let snapshot = runtime.membership_snapshot(workspace.id()).unwrap();
    assert_eq!(snapshot.workspace_id(), workspace.id());
    assert_eq!(snapshot.epoch(), MembershipEpoch::new(0));
    assert_eq!(snapshot.members().len(), 1);
    assert_eq!(snapshot.members()[0].identity(), identity);
    assert_eq!(snapshot.members()[0].role(), MembershipRole::Manager);
    assert_eq!(
        runtime.membership_version(workspace.id()).unwrap(),
        snapshot.version()
    );

    runtime.shutdown().await.unwrap();
    drop(runtime);

    let reopened = CoreRuntime::open_loopback_for_tests(config).await.unwrap();
    assert_eq!(reopened.public_identity(), identity);
    assert_eq!(reopened.list_workspaces().unwrap(), vec![workspace]);
    assert_eq!(
        reopened
            .membership_snapshot(snapshot.workspace_id())
            .unwrap(),
        snapshot
    );
    reopened.shutdown().await.unwrap();
    drop(reopened);

    fs::remove_dir_all(directory).unwrap();
}
