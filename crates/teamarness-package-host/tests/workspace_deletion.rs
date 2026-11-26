//! Workspace deletion through the Package Host (ADR 0049).

mod common;

use std::fs;
use std::path::PathBuf;

use teamarness_package_host::{
    PackageHost, PackageHostCommand, ShutdownHost, WorkspaceDeletionFailure, WorkspaceId,
};

fn workspace_data(fixture: &common::Fixture, workspace: &str) -> PathBuf {
    fixture.config.private_data_directory().join(format!(
        "packages/data/v1/{}/{}",
        fixture.config.installation_public_identity().as_str(),
        workspace
    ))
}

fn host_rows(fixture: &common::Fixture, table: &str, workspace: &str) -> i64 {
    let connection = rusqlite::Connection::open(
        fixture
            .config
            .private_data_directory()
            .join("packages/package-host.sqlite3"),
    )
    .unwrap();
    connection
        .query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE workspace_id = ?1"),
            [workspace],
            |row| row.get(0),
        )
        .unwrap()
}

/// Verifies: TEAM.SYS.SHELL-R04. Deleting a Workspace disposes only its Instances, erases it
/// in Core, removes its Package Data Directories and Host rows, and leaves a folder its data
/// linked to, and every other Workspace, as they were.
#[tokio::test]
async fn deleting_a_workspace_removes_its_instances_and_data_only() {
    let fixture = common::Fixture::node(&[("one", "dev.teamarness.one")]).await;
    let kept = fixture.core.create_workspace("Kept").unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    assert_eq!(host.snapshot().await.instances.len(), 2);
    let doomed = fixture
        .core
        .list_workspaces()
        .unwrap()
        .into_iter()
        .find(|workspace| workspace.id() != kept.id())
        .unwrap()
        .id()
        .to_string();
    let doomed_data = workspace_data(&fixture, &doomed);
    let kept_data = workspace_data(&fixture, &kept.id().to_string());
    assert!(doomed_data.is_dir());
    assert!(kept_data.is_dir());
    let outside = fixture
        .config
        .private_data_directory()
        .join("mapped-folder");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("note.md"), "keep me").unwrap();
    let package_dir = fs::read_dir(&doomed_data)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::os::unix::fs::symlink(&outside, package_dir.join("link")).unwrap();

    let deletion = host
        .delete_workspace(&WorkspaceId::new(doomed.clone()).unwrap())
        .await
        .unwrap();
    assert_eq!(deletion.removed.len(), 1);
    assert!(!deletion.data_removal_pending);

    let snapshot = host.snapshot().await;
    assert_eq!(snapshot.instances.len(), 1);
    assert_eq!(
        snapshot.instances[0].key.workspace_id.as_str(),
        kept.id().to_string()
    );
    assert_eq!(fixture.core.list_workspaces().unwrap(), vec![kept.clone()]);
    assert!(!doomed_data.exists());
    assert!(kept_data.is_dir());
    assert_eq!(
        fs::read_to_string(outside.join("note.md")).unwrap(),
        "keep me"
    );
    assert_eq!(host_rows(&fixture, "package_data_directories", &doomed), 0);
    assert_eq!(
        host_rows(&fixture, "pending_workspace_deletions", &doomed),
        0
    );
    assert_eq!(
        host_rows(&fixture, "package_data_directories", &kept.id().to_string()),
        1
    );

    host.execute(PackageHostCommand::Shutdown(ShutdownHost))
        .await
        .unwrap();
}

/// Verifies: TEAM.SYS.SHELL-R04. A refused deletion changes nothing.
#[tokio::test]
async fn a_refused_deletion_leaves_every_instance_running() {
    let fixture = common::Fixture::node(&[("one", "dev.teamarness.one")]).await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let before = host.snapshot().await.instances;
    let refused = host
        .delete_workspace(&WorkspaceId::new("07".repeat(16)).unwrap())
        .await;
    assert!(matches!(
        refused,
        Err(WorkspaceDeletionFailure::Refused(
            teamarness_core::WorkspaceError::UnknownWorkspace
        ))
    ));
    assert_eq!(host.snapshot().await.instances, before);
    assert_eq!(fixture.core.list_workspaces().unwrap().len(), 1);
    host.execute(PackageHostCommand::Shutdown(ShutdownHost))
        .await
        .unwrap();
}

/// Verifies: TEAM.SYS.SHELL-R04. Opening the Host settles a deletion that stopped part way:
/// a Workspace Core still lists keeps its data and loses the record; one Core already erased
/// has its Package data removed before any Instance is created.
#[tokio::test]
async fn opening_the_host_settles_interrupted_deletions() {
    let fixture = common::Fixture::node(&[("one", "dev.teamarness.one")]).await;
    let second = fixture.core.create_workspace("Second").unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    host.execute(PackageHostCommand::Shutdown(ShutdownHost))
        .await
        .unwrap();
    drop(host);
    let first = fixture
        .core
        .list_workspaces()
        .unwrap()
        .into_iter()
        .find(|workspace| workspace.id() != second.id())
        .unwrap()
        .id()
        .to_string();
    let second = second.id().to_string();
    {
        let connection = rusqlite::Connection::open(
            fixture
                .config
                .private_data_directory()
                .join("packages/package-host.sqlite3"),
        )
        .unwrap();
        for workspace in [&first, &second] {
            connection
                .execute(
                    "INSERT INTO pending_workspace_deletions(installation_identity, workspace_id)
                     VALUES (?1, ?2)",
                    [
                        fixture.config.installation_public_identity().as_str(),
                        workspace,
                    ],
                )
                .unwrap();
        }
    }
    // The first deletion stopped after Core erased the Workspace; the second before.
    fixture
        .core
        .delete_workspace(first.parse().unwrap())
        .unwrap();

    let reopened = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    assert!(!workspace_data(&fixture, &first).exists());
    assert!(workspace_data(&fixture, &second).is_dir());
    assert_eq!(
        host_rows(&fixture, "pending_workspace_deletions", &first),
        0
    );
    assert_eq!(
        host_rows(&fixture, "pending_workspace_deletions", &second),
        0
    );
    assert_eq!(host_rows(&fixture, "package_data_directories", &first), 0);
    let snapshot = reopened.snapshot().await;
    assert_eq!(snapshot.instances.len(), 1);
    assert_eq!(snapshot.instances[0].key.workspace_id.as_str(), second);
    reopened
        .execute(PackageHostCommand::Shutdown(ShutdownHost))
        .await
        .unwrap();
}
