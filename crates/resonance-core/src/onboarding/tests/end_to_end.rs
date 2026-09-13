use std::fs;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use iroh_base::EndpointAddr;
use rusqlite::Connection;

use crate::{
    CoreRuntime, CoreRuntimeConfig, MembershipRole, MembershipSnapshot, OnboardingError,
    WorkspaceInvitation,
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn directory(name: &str) -> std::path::PathBuf {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "resonance-end-to-end-{name}-{}-{sequence}",
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

fn admission_rows(directory: &std::path::Path) -> (i64, i64) {
    Connection::open(directory.join("core.sqlite3"))
        .unwrap()
        .query_row(
            "SELECT (SELECT COUNT(*) FROM used_invitation_nonces),
                    (SELECT COUNT(*) FROM membership_operations WHERE action = 1)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

fn assert_same_membership(left: &MembershipSnapshot, right: &MembershipSnapshot) {
    assert_eq!(left.workspace_id(), right.workspace_id());
    assert_eq!(left.version(), right.version());
    assert_eq!(left.members(), right.members());
}

async fn open(name: &str) -> (std::path::PathBuf, CoreRuntime) {
    let path = directory(name);
    let runtime = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&path))
        .await
        .unwrap();
    (path, runtime)
}

async fn close(runtime: CoreRuntime, path: &std::path::Path) {
    runtime.shutdown().await.unwrap();
    drop(runtime);
    fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn fresh_join_matches_the_export_boundary_and_survives_both_restarts() {
    let (issuer_directory, issuer) = open("fresh-issuer").await;
    let (joiner_directory, joiner) = open("fresh-joiner").await;
    let issuer_config = CoreRuntimeConfig::new(&issuer_directory);
    let joiner_config = CoreRuntimeConfig::new(&joiner_directory);
    assert_ne!(issuer.public_identity(), joiner.public_identity());

    let workspace = issuer.create_workspace("End-to-end Workspace").unwrap();
    let invitation = issuer
        .create_workspace_invitation(workspace.id())
        .await
        .unwrap();
    let (joined, joined_snapshot) = joiner.join_workspace(&invitation).await.unwrap();
    let exported_snapshot = issuer.membership_snapshot(workspace.id()).unwrap();

    assert_eq!(joined, workspace);
    assert_same_membership(&exported_snapshot, &joined_snapshot);
    let identities = joined_snapshot
        .members()
        .iter()
        .map(|member| member.identity())
        .collect::<Vec<_>>();
    assert!(identities.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(joined_snapshot.members().iter().any(|member| {
        member.identity() == issuer.public_identity() && member.role() == MembershipRole::Manager
    }));
    assert!(joined_snapshot.members().iter().any(|member| {
        member.identity() == joiner.public_identity() && member.role() == MembershipRole::Member
    }));

    issuer.shutdown().await.unwrap();
    joiner.shutdown().await.unwrap();
    drop((issuer, joiner));
    let reopened_issuer = CoreRuntime::open_for_onboarding_test(issuer_config)
        .await
        .unwrap();
    let reopened_joiner = CoreRuntime::open_for_onboarding_test(joiner_config)
        .await
        .unwrap();
    let issuer_after_restart = reopened_issuer.membership_snapshot(workspace.id()).unwrap();
    let joiner_after_restart = reopened_joiner.membership_snapshot(workspace.id()).unwrap();
    assert_same_membership(&exported_snapshot, &issuer_after_restart);
    assert_same_membership(&exported_snapshot, &joiner_after_restart);

    close(reopened_issuer, &issuer_directory).await;
    close(reopened_joiner, &joiner_directory).await;
}

#[tokio::test]
async fn response_loss_recovers_one_admission_through_the_public_join_operation() {
    let (issuer_directory, issuer) = open("loss-issuer").await;
    let (joiner_directory, joiner) = open("loss-joiner").await;
    let workspace = issuer.create_workspace("Response recovery").unwrap();
    let invitation = issuer
        .create_workspace_invitation(workspace.id())
        .await
        .unwrap();
    issuer.drop_next_onboarding_response_for_test();

    assert!(joiner.join_workspace(&invitation).await.is_err());
    assert_eq!(workspace_rows(&joiner_directory), 0);
    assert_eq!(admission_rows(&issuer_directory), (1, 1));

    let (_, recovered) = joiner.join_workspace(&invitation).await.unwrap();
    let exported = issuer.membership_snapshot(workspace.id()).unwrap();
    assert_same_membership(&exported, &recovered);
    assert_eq!(admission_rows(&issuer_directory), (1, 1));

    close(issuer, &issuer_directory).await;
    close(joiner, &joiner_directory).await;
}

#[tokio::test]
async fn retry_after_expiry_imports_later_current_state_but_refuses_a_revoked_invitee() {
    let (issuer_directory, issuer) = open("current-issuer").await;
    let (first_directory, first) = open("current-first").await;
    let (second_directory, second) = open("current-second").await;
    let workspace = issuer.create_workspace("Current completion").unwrap();
    let now = now_unix_ms();
    let first_invitation = issuer
        .create_workspace_invitation_for_test(workspace.id(), now, now + 25)
        .unwrap();
    let first_claim = first
        .create_claim_for_invitation_test(&first_invitation)
        .unwrap();
    issuer
        .admit_and_export_for_test(&first_claim, first.public_identity(), None)
        .unwrap();

    let second_invitation = issuer
        .create_workspace_invitation(workspace.id())
        .await
        .unwrap();
    second.join_workspace(&second_invitation).await.unwrap();
    std::thread::sleep(Duration::from_millis(35));

    let (_, first_snapshot) = first.join_workspace(&first_invitation).await.unwrap();
    let current = issuer.membership_snapshot(workspace.id()).unwrap();
    assert_same_membership(&current, &first_snapshot);
    assert_eq!(first_snapshot.members().len(), 3);

    let (revoked_directory, revoked) = open("current-revoked").await;
    let revoked_invitation = issuer
        .create_workspace_invitation(workspace.id())
        .await
        .unwrap();
    let revoked_claim = revoked
        .create_claim_for_invitation_test(&revoked_invitation)
        .unwrap();
    issuer
        .admit_and_export_for_test(&revoked_claim, revoked.public_identity(), None)
        .unwrap();
    issuer
        .revoke_member(workspace.id(), revoked.public_identity())
        .unwrap();
    assert_eq!(
        revoked
            .join_workspace(&revoked_invitation)
            .await
            .unwrap_err(),
        OnboardingError::AdmissionNoLongerCurrent
    );
    assert_eq!(workspace_rows(&revoked_directory), 0);

    close(issuer, &issuer_directory).await;
    close(first, &first_directory).await;
    close(second, &second_directory).await;
    close(revoked, &revoked_directory).await;
}

#[tokio::test]
async fn simultaneous_claimants_admit_only_one_installation() {
    let (issuer_directory, issuer) = open("race-issuer").await;
    let (first_directory, first) = open("race-first").await;
    let (second_directory, second) = open("race-second").await;
    let workspace = issuer.create_workspace("Claimant race").unwrap();
    let invitation = issuer
        .create_workspace_invitation(workspace.id())
        .await
        .unwrap();

    let (first_result, second_result) = tokio::join!(
        first.join_workspace(&invitation),
        second.join_workspace(&invitation)
    );
    let successes = usize::from(first_result.is_ok()) + usize::from(second_result.is_ok());
    assert_eq!(successes, 1);
    let failure = first_result.err().or_else(|| second_result.err()).unwrap();
    assert_eq!(failure, OnboardingError::AlreadyUsed);
    assert_eq!(admission_rows(&issuer_directory), (1, 1));
    assert_eq!(
        workspace_rows(&first_directory) + workspace_rows(&second_directory),
        1
    );

    close(issuer, &issuer_directory).await;
    close(first, &first_directory).await;
    close(second, &second_directory).await;
}

#[tokio::test]
async fn every_precommit_rejection_leaves_no_false_local_join() {
    let malformed_directory = directory("rejection-malformed");
    let malformed =
        CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&malformed_directory))
            .await
            .unwrap();
    assert!(WorkspaceInvitation::from_str("not-an-invitation").is_err());
    assert_eq!(workspace_rows(&malformed_directory), 0);
    close(malformed, &malformed_directory).await;

    let (issuer_directory, issuer) = open("rejection-issuer").await;
    let workspace = issuer.create_workspace("Rejections").unwrap();

    let (expired_directory, expired) = open("rejection-expired").await;
    let now = now_unix_ms();
    let expired_invitation = issuer
        .create_workspace_invitation_for_test(workspace.id(), now - 10, now - 1)
        .unwrap();
    assert_eq!(
        expired
            .join_workspace(&expired_invitation)
            .await
            .unwrap_err(),
        OnboardingError::Expired
    );
    assert_eq!(workspace_rows(&expired_directory), 0);

    let (wrong_directory, wrong) = open("rejection-wrong-peer").await;
    let (decoy_directory, decoy) = open("rejection-decoy").await;
    let invitation = issuer
        .create_workspace_invitation(workspace.id())
        .await
        .unwrap();
    let verified = invitation.verified().unwrap();
    let decoy_address = decoy.endpoint_address_for_test();
    let wrong_peer_invitation = WorkspaceInvitation::from_parts(
        &verified.token,
        EndpointAddr::from_parts(verified.endpoint_addr.id, decoy_address.addrs),
    )
    .unwrap();
    assert_eq!(
        wrong
            .join_workspace(&wrong_peer_invitation)
            .await
            .unwrap_err(),
        OnboardingError::Authentication
    );
    assert_eq!(workspace_rows(&wrong_directory), 0);

    let (timeout_directory, timed_out) = open("rejection-timeout").await;
    timed_out.timeout_next_join_for_test();
    assert_eq!(
        timed_out.join_workspace(&invitation).await.unwrap_err(),
        OnboardingError::Timeout
    );
    assert_eq!(workspace_rows(&timeout_directory), 0);

    let (invalid_directory, invalid) = open("rejection-invalid-bootstrap").await;
    invalid.invalidate_next_bootstrap_for_test();
    assert_eq!(
        invalid.join_workspace(&invitation).await.unwrap_err(),
        OnboardingError::InvalidBootstrap
    );
    assert_eq!(workspace_rows(&invalid_directory), 0);

    let (overload_directory, overloaded) = open("rejection-overload").await;
    let permits = overloaded.exhaust_outbound_joins_for_test().await;
    assert_eq!(
        overloaded.join_workspace(&invitation).await.unwrap_err(),
        OnboardingError::Overload
    );
    assert_eq!(workspace_rows(&overload_directory), 0);
    drop(permits);

    let (conflict_directory, conflict) = open("rejection-conflict").await;
    let local = conflict
        .create_workspace("Conflicting local anchor")
        .unwrap();
    let conflict_invitation = issuer
        .create_workspace_invitation_for_workspace_id_for_test(local.id())
        .unwrap();
    assert_eq!(
        conflict
            .join_workspace(&conflict_invitation)
            .await
            .unwrap_err(),
        OnboardingError::LocalConflict
    );
    assert_eq!(workspace_rows(&conflict_directory), 1);

    close(expired, &expired_directory).await;
    close(wrong, &wrong_directory).await;
    close(decoy, &decoy_directory).await;
    close(timed_out, &timeout_directory).await;
    close(invalid, &invalid_directory).await;
    close(overloaded, &overload_directory).await;
    close(conflict, &conflict_directory).await;
    close(issuer, &issuer_directory).await;
}

#[tokio::test]
async fn malformed_frame_and_shutdown_during_active_work_fail_without_admission() {
    let (issuer_directory, issuer) = open("lifecycle-issuer").await;
    let (client_directory, client) = open("lifecycle-client").await;
    let workspace = issuer.create_workspace("Lifecycle failures").unwrap();
    let invitation = issuer
        .create_workspace_invitation(workspace.id())
        .await
        .unwrap();
    let verified = invitation.verified().unwrap();
    let response = client
        .send_raw_onboarding_frame_for_test(verified.endpoint_addr, &[0, 0, 0, 1, 99])
        .await
        .unwrap();
    assert_eq!(
        super::decode_error(&response).unwrap().error,
        OnboardingError::InvalidInput
    );
    assert_eq!(admission_rows(&issuer_directory), (0, 0));

    let client = Arc::new(client);
    let active = client.hold_outbound_join_for_test().await.unwrap();
    let closing = Arc::clone(&client);
    let shutdown = tokio::spawn(async move { closing.shutdown().await });
    while client.onboarding_lifecycle_state_for_test() == 0 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        client.join_workspace(&invitation).await.unwrap_err(),
        OnboardingError::Shutdown
    );
    drop(active);
    shutdown.await.unwrap().unwrap();

    issuer.shutdown().await.unwrap();
    drop(issuer);
    let client = Arc::try_unwrap(client).expect("shutdown task released runtime");
    drop(client);
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(client_directory).unwrap();
}
