use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use resonance_core::{
    CoreError, CoreRuntime, CoreRuntimeConfig, MembershipEpoch, MembershipError, MembershipRole,
    StorageError, WorkspaceId,
};
use rusqlite::Connection;

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn private_data_directory(test_name: &str) -> PathBuf {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "resonance-core-{test_name}-{}-{sequence}",
        std::process::id()
    ))
}

fn database(directory: &Path) -> Connection {
    Connection::open(directory.join("core.sqlite3")).unwrap()
}

#[test]
fn workspace_snapshot_is_available_through_core_runtime_and_survives_reopen() {
    let directory = private_data_directory("membership-snapshot");
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open(config.clone()).unwrap();
    let creator = runtime.public_identity();
    let workspace = runtime.create_workspace("Projected").unwrap();

    let before = runtime.membership_snapshot(workspace.id()).unwrap();
    assert_eq!(before.workspace_id(), workspace.id());
    assert_eq!(before.epoch(), MembershipEpoch::new(0));
    assert_eq!(before.members().len(), 1);
    assert_eq!(before.members()[0].identity(), creator);
    assert_eq!(before.members()[0].role(), MembershipRole::Manager);
    drop(runtime);

    let reopened = CoreRuntime::open(config).unwrap();
    assert_eq!(
        reopened.membership_snapshot(workspace.id()).unwrap(),
        before
    );

    drop(reopened);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn invalid_public_membership_commands_leave_the_projection_unchanged() {
    let directory = private_data_directory("membership-invalid");
    let runtime = CoreRuntime::open(CoreRuntimeConfig::new(&directory)).unwrap();
    let workspace = runtime.create_workspace("Guarded membership").unwrap();
    let before = runtime.membership_snapshot(workspace.id()).unwrap();

    assert_eq!(
        runtime
            .revoke_member(workspace.id(), runtime.public_identity())
            .unwrap_err(),
        MembershipError::CannotRevokeManager
    );
    assert_eq!(runtime.membership_snapshot(workspace.id()).unwrap(), before);
    assert_eq!(
        runtime
            .membership_snapshot(WorkspaceId::from_str("00000000000000000000000000000000").unwrap())
            .unwrap_err(),
        MembershipError::WorkspaceNotFound
    );

    drop(runtime);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn derived_rows_are_rebuilt_but_corrupt_canonical_bytes_refuse_open() {
    let directory = private_data_directory("membership-rebuild");
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open(config.clone()).unwrap();
    let workspace = runtime.create_workspace("Repairable").unwrap();
    let expected = runtime.membership_snapshot(workspace.id()).unwrap();
    drop(runtime);

    let connection = database(&directory);
    connection
        .execute("DELETE FROM membership_snapshot_members", [])
        .unwrap();
    connection
        .execute("DELETE FROM workspace_membership_state", [])
        .unwrap();
    drop(connection);

    let reopened = CoreRuntime::open(config.clone()).unwrap();
    assert_eq!(
        reopened.membership_snapshot(workspace.id()).unwrap(),
        expected
    );
    drop(reopened);

    let connection = database(&directory);
    connection
        .execute_batch("DROP TRIGGER membership_operations_no_update;")
        .unwrap();
    connection
        .execute(
            "UPDATE membership_operations SET canonical_envelope = X'00'",
            [],
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        CoreRuntime::open(config),
        Err(CoreError::Storage(StorageError::CorruptWorkspace))
    ));

    fs::remove_dir_all(directory).unwrap();
}
