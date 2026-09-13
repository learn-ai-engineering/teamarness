use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use resonance_core::{
    CoreRuntime, CoreRuntimeConfig, MembershipEpoch, MembershipRole, PublicIdentity,
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn private_data_directory(installation: &str) -> PathBuf {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "resonance-core-workflow-{installation}-{}-{sequence}",
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

fn wait_for_later_millisecond(earlier: i64) -> i64 {
    loop {
        let now = now_unix_ms();
        if now > earlier {
            return now;
        }
        std::thread::yield_now();
    }
}

fn has_member(snapshot: &resonance_core::MembershipSnapshot, identity: PublicIdentity) -> bool {
    snapshot
        .members()
        .iter()
        .any(|member| member.identity() == identity && member.role() == MembershipRole::Member)
}

#[tokio::test]
async fn public_runtime_workflow_survives_membership_lifecycle_and_reopen() {
    let installation_a_directory = private_data_directory("a");
    let installation_b_directory = private_data_directory("b");
    let installation_a_config = CoreRuntimeConfig::new(&installation_a_directory);
    let installation_a = CoreRuntime::open(installation_a_config.clone())
        .await
        .unwrap();
    let installation_b = CoreRuntime::open(CoreRuntimeConfig::new(&installation_b_directory))
        .await
        .unwrap();
    let identity_a = installation_a.public_identity();
    let identity_b = installation_b.public_identity();

    let workspace = installation_a
        .create_workspace("Acceptance workspace")
        .unwrap();
    assert_eq!(workspace.created_by(), identity_a);
    assert_eq!(
        installation_a.list_workspaces().unwrap(),
        vec![workspace.clone()]
    );
    let genesis = installation_a.membership_snapshot(workspace.id()).unwrap();
    assert_eq!(genesis.epoch(), MembershipEpoch::new(0));
    assert_eq!(genesis.members().len(), 1);
    assert_eq!(genesis.members()[0].role(), MembershipRole::Manager);

    let mut transitions = installation_a
        .subscribe_membership(workspace.id(), MembershipEpoch::new(0))
        .unwrap();
    let token = installation_a
        .issue_invitation(workspace.id(), now_unix_ms() + 60_000)
        .unwrap();
    let claim = installation_b.create_invitation_claim(&token).unwrap();
    let admitted = installation_a.redeem_invitation(&claim).unwrap();
    assert_eq!(
        transitions.next_transition().unwrap(),
        Some(admitted.clone())
    );
    assert_eq!(admitted.epoch(), MembershipEpoch::new(1));

    let admitted_snapshot = installation_a.membership_snapshot(workspace.id()).unwrap();
    assert!(has_member(&admitted_snapshot, identity_b));
    assert_eq!(
        installation_a.membership_version(workspace.id()).unwrap(),
        admitted_snapshot.version()
    );
    assert!(
        !installation_a
            .was_member(workspace.id(), identity_b, i64::MIN)
            .unwrap()
    );
    let active_at = wait_for_later_millisecond(now_unix_ms());
    assert!(
        installation_a
            .was_member(workspace.id(), identity_b, active_at)
            .unwrap()
    );

    wait_for_later_millisecond(active_at);
    let revoked = installation_a
        .revoke_member(workspace.id(), identity_b)
        .unwrap();
    assert_eq!(
        transitions.next_transition().unwrap(),
        Some(revoked.clone())
    );
    assert_eq!(revoked.epoch(), MembershipEpoch::new(2));
    assert!(admitted.epoch() < revoked.epoch());

    let revoked_snapshot = installation_a.membership_snapshot(workspace.id()).unwrap();
    assert!(!has_member(&revoked_snapshot, identity_b));
    assert!(
        installation_a
            .was_member(workspace.id(), identity_b, active_at)
            .unwrap()
    );
    assert!(
        !installation_a
            .was_member(workspace.id(), identity_b, i64::MAX)
            .unwrap()
    );
    assert_eq!(
        installation_a.membership_version(workspace.id()).unwrap(),
        revoked_snapshot.version()
    );

    drop(transitions);
    installation_a.shutdown().await.unwrap();
    drop(installation_a);
    let reopened_a = CoreRuntime::open(installation_a_config).await.unwrap();
    assert_eq!(reopened_a.public_identity(), identity_a);
    let reopened_snapshot = reopened_a.membership_snapshot(workspace.id()).unwrap();
    assert_eq!(reopened_snapshot, revoked_snapshot);
    assert_eq!(reopened_snapshot.epoch(), revoked_snapshot.epoch());
    assert_eq!(reopened_snapshot.version(), revoked_snapshot.version());

    reopened_a.shutdown().await.unwrap();
    drop(reopened_a);
    installation_b.shutdown().await.unwrap();
    drop(installation_b);
    fs::remove_dir_all(installation_a_directory).unwrap();
    fs::remove_dir_all(installation_b_directory).unwrap();
}
