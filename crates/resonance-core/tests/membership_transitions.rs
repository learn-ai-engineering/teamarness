use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use resonance_core::{
    CoreError, CoreRuntime, CoreRuntimeConfig, InvitationError, MembershipEpoch, MembershipError,
    StorageError,
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

fn admit(issuer: &CoreRuntime, invitee: &CoreRuntime, workspace_id: resonance_core::WorkspaceId) {
    let token = issuer
        .issue_invitation(workspace_id, now_unix_ms() + 60_000)
        .unwrap();
    let claim = invitee.create_invitation_claim(&token).unwrap();
    issuer.redeem_invitation(&claim).unwrap();
}

#[test]
fn durable_and_live_transitions_resume_in_order_without_startup_duplicates() {
    let issuer_directory = private_data_directory("transitions-issuer");
    let invitee_a_directory = private_data_directory("transitions-a");
    let invitee_b_directory = private_data_directory("transitions-b");
    let config = CoreRuntimeConfig::new(&issuer_directory);
    let issuer = CoreRuntime::open(config.clone()).unwrap();
    let invitee_a = CoreRuntime::open(CoreRuntimeConfig::new(&invitee_a_directory)).unwrap();
    let invitee_b = CoreRuntime::open(CoreRuntimeConfig::new(&invitee_b_directory)).unwrap();
    let workspace = issuer.create_workspace("Transitions").unwrap();
    let mut live = issuer
        .subscribe_membership(workspace.id(), MembershipEpoch::new(0))
        .unwrap();

    let first_token = issuer
        .issue_invitation(workspace.id(), now_unix_ms() + 60_000)
        .unwrap();
    let first_claim = invitee_a.create_invitation_claim(&first_token).unwrap();
    issuer.redeem_invitation(&first_claim).unwrap();
    let first = live.next_transition().unwrap().unwrap();
    assert_eq!(first.epoch(), MembershipEpoch::new(1));
    assert_eq!(
        first.version(),
        issuer.membership_version(workspace.id()).unwrap()
    );
    assert_eq!(
        issuer.redeem_invitation(&first_claim).unwrap_err(),
        InvitationError::AlreadyUsed
    );
    assert_eq!(
        database(&issuer_directory)
            .query_row("SELECT COUNT(*) FROM membership_transitions", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );

    assert_eq!(
        issuer
            .revoke_member(workspace.id(), issuer.public_identity())
            .unwrap_err(),
        MembershipError::CannotRevokeManager
    );
    assert_eq!(
        issuer.membership_snapshot(workspace.id()).unwrap().epoch(),
        MembershipEpoch::new(1)
    );

    admit(&issuer, &invitee_b, workspace.id());
    issuer
        .revoke_member(workspace.id(), invitee_a.public_identity())
        .unwrap();
    let second = live.next_transition().unwrap().unwrap();
    let third = live.next_transition().unwrap().unwrap();
    assert_eq!(second.epoch(), MembershipEpoch::new(2));
    assert_eq!(third.epoch(), MembershipEpoch::new(3));

    let connection = database(&issuer_directory);
    let rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM membership_transitions", [], |row| {
            row.get(0)
        })
        .unwrap();
    let digest: String = connection
        .query_row(
            "SELECT lower(hex(snapshot_digest)) FROM membership_transitions WHERE epoch = 3",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rows, 3);
    assert_eq!(digest, third.snapshot_digest().to_string());
    drop(connection);
    drop(live);
    drop(issuer);

    let reopened = CoreRuntime::open(config.clone()).unwrap();
    let mut from_zero = reopened
        .subscribe_membership(workspace.id(), MembershipEpoch::new(0))
        .unwrap();
    for epoch in 1..=3 {
        assert_eq!(
            from_zero.next_transition().unwrap().unwrap().epoch(),
            MembershipEpoch::new(epoch)
        );
    }
    let mut from_two = reopened
        .subscribe_membership(workspace.id(), MembershipEpoch::new(2))
        .unwrap();
    assert_eq!(
        from_two.next_transition().unwrap().unwrap().epoch(),
        MembershipEpoch::new(3)
    );

    drop(from_two);
    drop(from_zero);
    drop(reopened);
    drop(invitee_a);
    drop(invitee_b);
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(invitee_a_directory).unwrap();
    fs::remove_dir_all(invitee_b_directory).unwrap();
}

#[test]
fn listener_registration_racing_a_commit_is_gap_free_and_slow_delivery_is_ordered() {
    let issuer_directory = private_data_directory("transition-race-issuer");
    let invitee_directory = private_data_directory("transition-race-invitee");
    let issuer = Arc::new(CoreRuntime::open(CoreRuntimeConfig::new(&issuer_directory)).unwrap());
    let invitee = CoreRuntime::open(CoreRuntimeConfig::new(&invitee_directory)).unwrap();
    let workspace = issuer.create_workspace("Transition race").unwrap();
    let token = issuer
        .issue_invitation(workspace.id(), now_unix_ms() + 60_000)
        .unwrap();
    let claim = invitee.create_invitation_claim(&token).unwrap();

    let subscribing = Arc::clone(&issuer);
    let workspace_id = workspace.id();
    let thread = std::thread::spawn(move || {
        subscribing
            .subscribe_membership(workspace_id, MembershipEpoch::new(0))
            .unwrap()
    });
    let committed = issuer.redeem_invitation(&claim).unwrap();
    let mut subscription = thread.join().unwrap();
    assert_eq!(subscription.next_transition().unwrap(), Some(committed));

    drop(subscription);
    drop(invitee);
    drop(issuer);
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(invitee_directory).unwrap();
}

#[test]
fn replay_repairs_derived_rows_but_rejects_missing_or_contradictory_event_evidence() {
    let issuer_directory = private_data_directory("transition-recovery-issuer");
    let invitee_directory = private_data_directory("transition-recovery-invitee");
    let config = CoreRuntimeConfig::new(&issuer_directory);
    let issuer = CoreRuntime::open(config.clone()).unwrap();
    let invitee = CoreRuntime::open(CoreRuntimeConfig::new(&invitee_directory)).unwrap();
    let workspace = issuer.create_workspace("Transition recovery").unwrap();
    admit(&issuer, &invitee, workspace.id());
    let expected = issuer.membership_snapshot(workspace.id()).unwrap();
    drop(issuer);

    let connection = database(&issuer_directory);
    connection
        .execute("DELETE FROM membership_snapshot_members", [])
        .unwrap();
    connection
        .execute("DELETE FROM workspace_membership_state", [])
        .unwrap();
    drop(connection);
    let repaired = CoreRuntime::open(config.clone()).unwrap();
    assert_eq!(
        repaired.membership_snapshot(workspace.id()).unwrap(),
        expected
    );
    drop(repaired);

    let connection = database(&issuer_directory);
    let event: (Vec<u8>, Vec<u8>, Vec<u8>) = connection
        .query_row(
            "SELECT membership_version, cause_operation_id, snapshot_digest
             FROM membership_transitions WHERE epoch = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    connection
        .execute_batch("DROP TRIGGER membership_transitions_no_delete;")
        .unwrap();
    connection
        .execute("DELETE FROM membership_transitions", [])
        .unwrap();
    drop(connection);
    assert!(matches!(
        CoreRuntime::open(config.clone()),
        Err(CoreError::Storage(StorageError::CorruptWorkspace))
    ));

    let connection = database(&issuer_directory);
    let workspace_bytes: Vec<u8> = connection
        .query_row("SELECT workspace_id FROM workspaces", [], |row| row.get(0))
        .unwrap();
    connection
        .execute(
            "INSERT INTO membership_transitions (
                workspace_id, epoch, membership_version, cause_operation_id, snapshot_digest
             ) VALUES (?1, 1, ?2, ?3, ?4)",
            rusqlite::params![workspace_bytes, event.0, event.1, event.2],
        )
        .unwrap();
    connection
        .execute_batch("DROP TRIGGER membership_transitions_no_update;")
        .unwrap();
    connection
        .execute(
            "UPDATE membership_transitions SET snapshot_digest = zeroblob(32)",
            [],
        )
        .unwrap();
    drop(connection);
    assert!(matches!(
        CoreRuntime::open(config),
        Err(CoreError::Storage(StorageError::CorruptWorkspace))
    ));

    drop(invitee);
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(invitee_directory).unwrap();
}
