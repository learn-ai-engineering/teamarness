use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use iroh_base::SecretKey;
use rusqlite::Connection;

use super::{JoinRequest, MAX_OPERATIONS, decode_error, decode_success, encode_request};
use crate::membership::projection::{BootstrapLimits, RedemptionBoundary};
use crate::onboarding::transport::Transport;
use crate::{CoreRuntime, CoreRuntimeConfig, OnboardingError};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn directory(name: &str) -> std::path::PathBuf {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "resonance-admission-recovery-{name}-{}-{sequence}",
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

fn count_admissions(directory: &std::path::Path) -> (i64, i64) {
    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();
    connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM used_invitation_nonces),
                    (SELECT COUNT(*) FROM membership_operations WHERE action = 1)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

#[tokio::test]
async fn exact_retry_recovers_after_expiry_without_a_duplicate_admission() {
    let issuer_directory = directory("exact-issuer");
    let invitee_directory = directory("exact-invitee");
    let issuer = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&issuer_directory))
        .await
        .unwrap();
    let invitee = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&invitee_directory))
        .await
        .unwrap();
    let workspace = issuer.create_workspace("Exact recovery").unwrap();
    let token = issuer
        .issue_invitation(workspace.id(), now_unix_ms() + 50)
        .unwrap();
    let claim = invitee.create_invitation_claim(&token).unwrap();

    let first = issuer
        .admit_and_export_for_test(&claim, invitee.public_identity(), None)
        .unwrap();
    let first_response = decode_success(&first).unwrap();
    std::thread::sleep(Duration::from_millis(75));
    let recovered = issuer
        .admit_and_export_for_test(&claim, invitee.public_identity(), None)
        .unwrap();
    let recovered_response = decode_success(&recovered).unwrap();

    assert_eq!(first_response.admission_id, recovered_response.admission_id);
    assert_eq!(count_admissions(&issuer_directory), (1, 1));

    issuer.shutdown().await.unwrap();
    invitee.shutdown().await.unwrap();
    drop(issuer);
    drop(invitee);
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(invitee_directory).unwrap();
}

#[tokio::test]
async fn peer_binding_fault_boundaries_and_response_loss_preserve_one_commit() {
    let issuer_directory = directory("fault-issuer");
    let invitee_directory = directory("fault-invitee");
    let issuer = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&issuer_directory))
        .await
        .unwrap();
    let invitee = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&invitee_directory))
        .await
        .unwrap();
    let workspace = issuer.create_workspace("Fault recovery").unwrap();
    let token = issuer.issue_invitation(workspace.id(), i64::MAX).unwrap();
    let claim = invitee.create_invitation_claim(&token).unwrap();
    let wrong_peer = SecretKey::from_bytes(&[99; 32]).public();

    assert_eq!(
        issuer
            .admit_and_export_for_test(
                &claim,
                crate::PublicIdentity::from_bytes(*wrong_peer.as_bytes()).unwrap(),
                None,
            )
            .unwrap_err(),
        OnboardingError::Authentication
    );
    assert_eq!(count_admissions(&issuer_directory), (0, 0));
    assert_eq!(
        issuer
            .admit_and_export_for_test(
                &claim,
                invitee.public_identity(),
                Some(RedemptionBoundary::BeforeCommit),
            )
            .unwrap_err(),
        OnboardingError::InvalidInput
    );
    assert_eq!(count_admissions(&issuer_directory), (0, 0));

    let _lost_response = issuer
        .admit_and_export_for_test(&claim, invitee.public_identity(), None)
        .unwrap();
    let recovered = issuer
        .admit_and_export_for_test(&claim, invitee.public_identity(), None)
        .unwrap();
    assert!(decode_success(&recovered).is_ok());
    assert_eq!(count_admissions(&issuer_directory), (1, 1));

    issuer.shutdown().await.unwrap();
    invitee.shutdown().await.unwrap();
    drop(issuer);
    drop(invitee);
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(invitee_directory).unwrap();
}

#[tokio::test]
async fn uncertain_after_commit_recovers_exactly_after_authenticated_reopen() {
    let issuer_directory = directory("uncertain-issuer");
    let invitee_directory = directory("uncertain-invitee");
    let config = CoreRuntimeConfig::new(&issuer_directory);
    let issuer = CoreRuntime::open_for_onboarding_test(config.clone())
        .await
        .unwrap();
    let invitee = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&invitee_directory))
        .await
        .unwrap();
    let workspace = issuer.create_workspace("Uncertain commit").unwrap();
    let token = issuer.issue_invitation(workspace.id(), i64::MAX).unwrap();
    let claim = invitee.create_invitation_claim(&token).unwrap();

    assert_eq!(
        issuer
            .admit_and_export_for_test(
                &claim,
                invitee.public_identity(),
                Some(RedemptionBoundary::AfterCommit),
            )
            .unwrap_err(),
        OnboardingError::UncertainStorageOutcome
    );
    assert_eq!(count_admissions(&issuer_directory), (1, 1));
    issuer.shutdown().await.unwrap();
    drop(issuer);

    let reopened = CoreRuntime::open_for_onboarding_test(config).await.unwrap();
    let response = reopened
        .admit_and_export_for_test(&claim, invitee.public_identity(), None)
        .unwrap();
    assert!(decode_success(&response).is_ok());
    assert_eq!(count_admissions(&issuer_directory), (1, 1));

    reopened.shutdown().await.unwrap();
    invitee.shutdown().await.unwrap();
    drop((reopened, invitee));
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(invitee_directory).unwrap();
}

#[tokio::test]
async fn authenticated_handler_returns_bootstrap_after_releasing_runtime_state() {
    let issuer_directory = directory("handler-issuer");
    let issuer = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&issuer_directory))
        .await
        .unwrap();
    let workspace = issuer.create_workspace("Handler response").unwrap();
    let invitation = issuer
        .create_workspace_invitation(workspace.id())
        .await
        .unwrap();
    let verified = invitation.verified().unwrap();
    let invitee_secret = SecretKey::from_bytes(&[88; 32]);
    let claim = crate::invitation::sign_claim(&invitee_secret, &verified.token).unwrap();
    let client = Transport::bind_loopback(invitee_secret, Duration::from_millis(20), true)
        .await
        .unwrap();
    let (connection, permit) = client.connect(verified.endpoint_addr).await.unwrap();
    let (mut send, mut receive) = connection.open_bi().await.unwrap();
    let request = encode_request(&JoinRequest { claim }).unwrap();
    send.write_all(&request).await.unwrap();
    send.finish().unwrap();
    let response = receive.read_to_end(8 * 1024 * 1024 + 4).await.unwrap();
    let decoded = decode_success(&response).unwrap();
    assert_eq!(decoded.workspace_id, workspace.id());
    assert_eq!(count_admissions(&issuer_directory), (1, 1));

    drop(permit);
    client.shutdown().await.unwrap();
    issuer.shutdown().await.unwrap();
    drop(issuer);
    fs::remove_dir_all(issuer_directory).unwrap();
}

#[tokio::test]
async fn different_claimant_and_revoked_recovery_are_rejected() {
    let issuer_directory = directory("claimant-issuer");
    let first_directory = directory("claimant-first");
    let second_directory = directory("claimant-second");
    let issuer = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&issuer_directory))
        .await
        .unwrap();
    let first = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&first_directory))
        .await
        .unwrap();
    let second = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&second_directory))
        .await
        .unwrap();
    let workspace = issuer.create_workspace("Claim binding").unwrap();
    let token = issuer.issue_invitation(workspace.id(), i64::MAX).unwrap();
    let first_claim = first.create_invitation_claim(&token).unwrap();
    let second_claim = second.create_invitation_claim(&token).unwrap();
    issuer
        .admit_and_export_for_test(&first_claim, first.public_identity(), None)
        .unwrap();
    assert_eq!(
        issuer
            .admit_and_export_for_test(&second_claim, second.public_identity(), None)
            .unwrap_err(),
        OnboardingError::AlreadyUsed
    );
    issuer
        .revoke_member(workspace.id(), first.public_identity())
        .unwrap();
    assert_eq!(
        issuer
            .admit_and_export_for_test(&first_claim, first.public_identity(), None)
            .unwrap_err(),
        OnboardingError::AdmissionNoLongerCurrent
    );
    assert_eq!(count_admissions(&issuer_directory), (1, 1));

    issuer.shutdown().await.unwrap();
    first.shutdown().await.unwrap();
    second.shutdown().await.unwrap();
    drop((issuer, first, second));
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(first_directory).unwrap();
    fs::remove_dir_all(second_directory).unwrap();
}

#[tokio::test]
async fn recovered_export_includes_later_operations_and_current_version() {
    let issuer_directory = directory("current-export-issuer");
    let first_directory = directory("current-export-first");
    let second_directory = directory("current-export-second");
    let issuer = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&issuer_directory))
        .await
        .unwrap();
    let first = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&first_directory))
        .await
        .unwrap();
    let second = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&second_directory))
        .await
        .unwrap();
    let workspace = issuer.create_workspace("Current export").unwrap();
    let first_token = issuer.issue_invitation(workspace.id(), i64::MAX).unwrap();
    let first_claim = first.create_invitation_claim(&first_token).unwrap();
    let first_response = decode_success(
        &issuer
            .admit_and_export_for_test(&first_claim, first.public_identity(), None)
            .unwrap(),
    )
    .unwrap();
    let second_token = issuer.issue_invitation(workspace.id(), i64::MAX).unwrap();
    let second_claim = second.create_invitation_claim(&second_token).unwrap();
    issuer
        .admit_and_export_for_test(&second_claim, second.public_identity(), None)
        .unwrap();

    let recovered = decode_success(
        &issuer
            .admit_and_export_for_test(&first_claim, first.public_identity(), None)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(recovered.admission_id, first_response.admission_id);
    assert_eq!(recovered.post_genesis.len(), 2);
    assert_eq!(
        recovered.membership_version,
        issuer.membership_version(workspace.id()).unwrap()
    );

    issuer.shutdown().await.unwrap();
    first.shutdown().await.unwrap();
    second.shutdown().await.unwrap();
    drop((issuer, first, second));
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(first_directory).unwrap();
    fs::remove_dir_all(second_directory).unwrap();
}

#[tokio::test]
async fn admission_and_revocation_fail_before_commit_when_bootstrap_is_over_limit() {
    let issuer_directory = directory("limits-issuer");
    let first_directory = directory("limits-first");
    let second_directory = directory("limits-second");
    let issuer = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&issuer_directory))
        .await
        .unwrap();
    let first = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&first_directory))
        .await
        .unwrap();
    let second = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&second_directory))
        .await
        .unwrap();
    let workspace = issuer.create_workspace("Mutation limits").unwrap();
    let first_token = issuer.issue_invitation(workspace.id(), i64::MAX).unwrap();
    let first_claim = first.create_invitation_claim(&first_token).unwrap();
    issuer
        .admit_and_export_for_test(&first_claim, first.public_identity(), None)
        .unwrap();
    let before = issuer.membership_snapshot(workspace.id()).unwrap();
    let second_token = issuer.issue_invitation(workspace.id(), i64::MAX).unwrap();
    let second_claim = second.create_invitation_claim(&second_token).unwrap();
    let tiny_operations = BootstrapLimits {
        max_body_bytes: 8 * 1024 * 1024,
        max_operations: 1,
    };
    let tiny_bytes = BootstrapLimits {
        max_body_bytes: 1,
        max_operations: 1_024,
    };

    assert_eq!(
        issuer
            .admit_and_export_with_limits_for_test(
                &second_claim,
                second.public_identity(),
                tiny_operations,
                None,
            )
            .unwrap_err(),
        OnboardingError::Overload
    );
    assert_eq!(count_admissions(&issuer_directory), (1, 1));
    assert_eq!(issuer.membership_snapshot(workspace.id()).unwrap(), before);
    assert_eq!(
        issuer
            .admit_and_export_with_limits_for_test(
                &second_claim,
                second.public_identity(),
                tiny_bytes,
                None,
            )
            .unwrap_err(),
        OnboardingError::Overload
    );
    assert_eq!(count_admissions(&issuer_directory), (1, 1));
    for limits in [tiny_operations, tiny_bytes] {
        assert_eq!(
            issuer
                .revoke_member_with_limits_for_test(
                    workspace.id(),
                    first.public_identity(),
                    limits,
                )
                .unwrap_err(),
            crate::MembershipError::DecodeLimit
        );
        assert_eq!(issuer.membership_snapshot(workspace.id()).unwrap(), before);
    }

    issuer.shutdown().await.unwrap();
    first.shutdown().await.unwrap();
    second.shutdown().await.unwrap();
    drop((issuer, first, second));
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(first_directory).unwrap();
    fs::remove_dir_all(second_directory).unwrap();
}

#[tokio::test]
async fn contradictory_spent_binding_fails_closed_as_corruption() {
    let issuer_directory = directory("corrupt-issuer");
    let invitee_directory = directory("corrupt-invitee");
    let issuer = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&issuer_directory))
        .await
        .unwrap();
    let invitee = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&invitee_directory))
        .await
        .unwrap();
    let workspace = issuer.create_workspace("Corrupt binding").unwrap();
    let token = issuer.issue_invitation(workspace.id(), i64::MAX).unwrap();
    let claim = invitee.create_invitation_claim(&token).unwrap();
    issuer.redeem_invitation(&claim).unwrap();

    let connection = Connection::open(issuer_directory.join("core.sqlite3")).unwrap();
    connection
        .execute_batch(
            "DROP TRIGGER used_invitation_nonces_no_update;
             UPDATE used_invitation_nonces SET invitation_token_id = zeroblob(32);",
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        issuer.redeem_invitation(&claim).unwrap_err(),
        crate::InvitationError::Storage(crate::StorageError::CorruptWorkspace)
    );
    assert_eq!(count_admissions(&issuer_directory), (1, 1));

    issuer.shutdown().await.unwrap();
    invitee.shutdown().await.unwrap();
    drop((issuer, invitee));
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(invitee_directory).unwrap();
}

#[tokio::test]
async fn malformed_handler_request_gets_a_stable_error_without_state_change() {
    let issuer_directory = directory("malformed-handler");
    let issuer = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&issuer_directory))
        .await
        .unwrap();
    let workspace = issuer.create_workspace("Malformed handler").unwrap();
    let invitation = issuer
        .create_workspace_invitation(workspace.id())
        .await
        .unwrap();
    let verified = invitation.verified().unwrap();
    let client = Transport::bind_loopback(
        SecretKey::from_bytes(&[77; 32]),
        Duration::from_millis(20),
        true,
    )
    .await
    .unwrap();
    let (connection, permit) = client.connect(verified.endpoint_addr).await.unwrap();
    let (mut send, mut receive) = connection.open_bi().await.unwrap();
    send.write_all(&[0, 0, 0, 1, 99]).await.unwrap();
    send.finish().unwrap();
    let response = receive.read_to_end(1_028).await.unwrap();
    assert_eq!(
        decode_error(&response).unwrap().error,
        OnboardingError::InvalidInput
    );
    assert_eq!(count_admissions(&issuer_directory), (0, 0));

    drop(permit);
    client.shutdown().await.unwrap();
    issuer.shutdown().await.unwrap();
    drop(issuer);
    fs::remove_dir_all(issuer_directory).unwrap();
}

#[test]
fn protocol_operation_limit_is_fixed() {
    assert_eq!(MAX_OPERATIONS, 1_024);
}
