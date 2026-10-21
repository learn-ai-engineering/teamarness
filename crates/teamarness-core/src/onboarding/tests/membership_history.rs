use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{CoreRuntime, CoreRuntimeConfig, PublicIdentity};
use rusqlite::Connection;

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn private_data_directory(test_name: &str) -> PathBuf {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "teamarness-core-{test_name}-{}-{sequence}",
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

fn database(directory: &Path) -> Connection {
    Connection::open(directory.join("core.sqlite3")).unwrap()
}

fn operation_time(directory: &Path, action: i64) -> i64 {
    database(directory)
        .query_row(
            "SELECT authored_at_unix_ms FROM membership_operations WHERE action = ?1",
            [action],
            |row| row.get(0),
        )
        .unwrap()
}

#[tokio::test]
async fn declared_history_uses_inclusive_admission_and_exclusive_revocation_boundaries() {
    let issuer_directory = private_data_directory("history-issuer");
    let invitee_directory = private_data_directory("history-invitee");
    let never_directory = private_data_directory("history-never");
    let issuer = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&issuer_directory))
        .await
        .unwrap();
    let invitee = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&invitee_directory))
        .await
        .unwrap();
    let never_runtime =
        CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&never_directory))
            .await
            .unwrap();
    let workspace = issuer.create_workspace("History").unwrap();
    let creator = issuer.public_identity();
    let never: PublicIdentity = never_runtime.public_identity();

    assert!(
        !issuer
            .was_member(workspace.id(), invitee.public_identity(), i64::MIN)
            .unwrap()
    );
    assert!(
        issuer
            .was_member(workspace.id(), creator, workspace.created_at_unix_ms())
            .unwrap()
    );
    assert!(!issuer.was_member(workspace.id(), never, i64::MAX).unwrap());

    let token = issuer
        .issue_invitation(workspace.id(), now_unix_ms() + 60_000)
        .unwrap();
    let claim = invitee
        .create_invitation_claim(&token, issuer.genesis_id_for_test(workspace.id()))
        .unwrap();
    issuer.redeem_invitation(&claim).unwrap();
    let admitted_at = operation_time(&issuer_directory, 1);

    assert!(
        !issuer
            .was_member(workspace.id(), invitee.public_identity(), admitted_at - 1)
            .unwrap()
    );
    assert!(
        issuer
            .was_member(workspace.id(), invitee.public_identity(), admitted_at)
            .unwrap()
    );
    assert!(
        issuer
            .was_member(workspace.id(), invitee.public_identity(), admitted_at + 1)
            .unwrap()
    );

    while now_unix_ms() <= admitted_at {
        // negative-wait: waits for the wall clock to leave the admission's millisecond.
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    issuer
        .revoke_member(workspace.id(), invitee.public_identity())
        .unwrap();
    let revoked_at = operation_time(&issuer_directory, 2);
    assert!(
        issuer
            .was_member(workspace.id(), invitee.public_identity(), revoked_at - 1)
            .unwrap()
    );
    assert!(
        !issuer
            .was_member(workspace.id(), invitee.public_identity(), revoked_at)
            .unwrap()
    );
    assert!(
        !issuer
            .was_member(workspace.id(), invitee.public_identity(), i64::MAX)
            .unwrap()
    );

    assert_eq!(
        issuer.membership_version(workspace.id()).unwrap(),
        issuer
            .membership_snapshot(workspace.id())
            .unwrap()
            .version()
    );

    never_runtime.shutdown().await.unwrap();
    drop(never_runtime);
    invitee.shutdown().await.unwrap();
    drop(invitee);
    issuer.shutdown().await.unwrap();
    drop(issuer);
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(invitee_directory).unwrap();
    fs::remove_dir_all(never_directory).unwrap();
}
