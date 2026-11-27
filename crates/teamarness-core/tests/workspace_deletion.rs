//! Sole-creator Workspace deletion (ADR 0050).

use std::fs;
use std::path::Path;
#[path = "../test-support/test_directory.rs"]
mod test_directory;
use std::str::FromStr;
use test_directory::TestDirectory;

use rusqlite::Connection;
use teamarness_core::{CoreRuntime, CoreRuntimeConfig, WorkspaceError, WorkspaceId};

fn private_data_directory(test_name: &str) -> TestDirectory {
    TestDirectory::new(test_name)
}

/// Rows of `workspace` left in any table with a `workspace_id` column, plus every row of the
/// scope-keyed tables (which these tests only populate through the deleted Workspace).
fn rows_left(directory: &Path, workspace: WorkspaceId) -> Vec<(String, i64)> {
    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();
    let tables: Vec<(String, bool)> = connection
        .prepare(
            "SELECT m.name,
                    EXISTS (SELECT 1 FROM pragma_table_info(m.name) WHERE name = 'workspace_id')
             FROM sqlite_master m WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite_%'",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let hex = workspace.to_string();
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
        .collect();
    let mut left = Vec::new();
    for (table, keyed) in tables {
        let count: i64 = if keyed {
            connection
                .query_row(
                    &format!("SELECT COUNT(*) FROM \"{table}\" WHERE workspace_id = ?1"),
                    [&bytes],
                    |row| row.get(0),
                )
                .unwrap()
        } else if [
            "immutable_operations",
            "immutable_records",
            "record_delivery",
            "p2panda_operations",
            "sealed_envelopes",
            "protection_state",
            "protection_grant_slots",
            "protection_pre_keys",
            "protection_control_operations",
            "immutable_author_heads",
            "immutable_idempotency",
            "immutable_operation_dependencies",
            "membership_dependencies",
        ]
        .contains(&table.as_str())
        {
            connection
                .query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |row| {
                    row.get(0)
                })
                .unwrap()
        } else {
            0
        };
        if count != 0 {
            left.push((table, count));
        }
    }
    left
}

/// Verifies: TEAM.SYS.SHELL-R04. The sole creator deletes a renamed Workspace that holds
/// records and an Agent; every row of it is erased, the Installation reopens cleanly, and a new
/// Workspace can be created afterwards.
#[tokio::test]
async fn sole_creator_erases_the_whole_workspace() {
    let directory = private_data_directory("workspace-delete");
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open_loopback_for_tests(config.clone())
        .await
        .unwrap();
    let workspace = runtime.create_workspace("Throwaway").unwrap().id();
    runtime
        .rename_workspace(workspace, "Still throwaway")
        .unwrap();
    runtime.add_agent(workspace, "Helper").unwrap();
    runtime
        .append_record(workspace, "teamarness.test", "message.post", b"one")
        .unwrap();
    runtime
        .append_record(workspace, "teamarness.test", "message.post", b"two")
        .unwrap();
    assert!(!rows_left(&directory, workspace).is_empty());

    runtime.check_workspace_deletable(workspace).unwrap();
    runtime.delete_workspace(workspace).unwrap();
    assert!(runtime.list_workspaces().unwrap().is_empty());
    assert!(runtime.membership_snapshot(workspace).is_err());
    assert_eq!(
        runtime.delete_workspace(workspace),
        Err(WorkspaceError::UnknownWorkspace)
    );
    assert_eq!(
        rows_left(&directory, workspace),
        Vec::<(String, i64)>::new()
    );

    runtime.shutdown().await.unwrap();
    drop(runtime);
    let reopened = CoreRuntime::open_loopback_for_tests(config).await.unwrap();
    assert!(reopened.list_workspaces().unwrap().is_empty());
    let next = reopened.create_workspace("Next").unwrap();
    assert_eq!(reopened.list_workspaces().unwrap(), vec![next]);
    reopened.shutdown().await.unwrap();
    drop(reopened);
    fs::remove_dir_all(directory).unwrap();
}

/// Verifies: TEAM.SYS.SHELL-R04. Deleting one Workspace leaves another one's records, and
/// the append-only guards, untouched.
#[tokio::test]
async fn deletion_leaves_other_workspaces_and_guards_intact() {
    let directory = private_data_directory("workspace-delete-other");
    let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
        .await
        .unwrap();
    let doomed = runtime.create_workspace("Doomed").unwrap().id();
    let kept = runtime.create_workspace("Kept").unwrap();
    runtime
        .append_record(doomed, "teamarness.test", "message.post", b"gone")
        .unwrap();
    let record = runtime
        .append_record(kept.id(), "teamarness.test", "message.post", b"kept")
        .unwrap();
    runtime.delete_workspace(doomed).unwrap();
    assert_eq!(runtime.list_workspaces().unwrap(), vec![kept.clone()]);

    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();
    let records: i64 = connection
        .query_row("SELECT COUNT(*) FROM immutable_records", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(records, 1);
    let foreign_scopes: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sync_scopes WHERE lower(hex(workspace_id)) <> ?1",
            [kept.id().to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(foreign_scopes, 0);
    assert_eq!(record.durability(), "committed");
    for statement in [
        "DELETE FROM membership_operations",
        "DELETE FROM workspaces",
        "DELETE FROM immutable_records",
        "UPDATE workspaces SET creator_public_identity = creator_public_identity",
    ] {
        assert!(
            connection.execute(statement, []).is_err(),
            "{statement} must stay refused outside an erase"
        );
    }
    connection
        .execute("UPDATE workspaces SET name = 'Renamed by hand'", [])
        .unwrap();
    drop(connection);

    runtime.shutdown().await.unwrap();
    drop(runtime);
    fs::remove_dir_all(directory).unwrap();
}

/// Verifies: TEAM.SYS.SHELL-R04. A Workspace with another Person cannot be deleted by its
/// creator, nor by the joined Member, and the refusals change nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deletion_is_refused_with_other_members_or_for_a_non_creator() {
    let root = private_data_directory("workspace-delete-shared");
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
    joiner.join_workspace(&invitation).await.unwrap();

    assert_eq!(
        issuer.delete_workspace(workspace),
        Err(WorkspaceError::WorkspaceHasOtherMembers)
    );
    assert_eq!(
        joiner.delete_workspace(workspace),
        Err(WorkspaceError::NotWorkspaceCreator)
    );
    assert_eq!(issuer.list_workspaces().unwrap().len(), 1);
    assert_eq!(joiner.list_workspaces().unwrap().len(), 1);
    assert_eq!(
        issuer.delete_workspace(WorkspaceId::from_str("07070707070707070707070707070707").unwrap()),
        Err(WorkspaceError::UnknownWorkspace)
    );

    issuer.shutdown().await.unwrap();
    joiner.shutdown().await.unwrap();
    drop(issuer);
    drop(joiner);
    fs::remove_dir_all(root).unwrap();
}
