use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use resonance_core::{
    CoreRuntime, CoreRuntimeConfig, InvitationClaim, InvitationError, InvitationToken,
    MembershipEpoch, MembershipRole, WorkspaceId,
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

fn decode_hex(value: &str) -> Vec<u8> {
    (0..value.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&value[offset..offset + 2], 16).unwrap())
        .collect()
}

fn tamper_hex_byte(value: &mut String, offset: usize) {
    let replacement = if &value[offset..offset + 2] == "00" {
        "01"
    } else {
        "00"
    };
    value.replace_range(offset..offset + 2, replacement);
}

#[tokio::test]
async fn issue_claim_redeem_and_reopen_preserve_the_complete_proof() {
    let issuer_directory = private_data_directory("invitation-issuer");
    let invitee_directory = private_data_directory("invitation-invitee");
    let issuer_config = CoreRuntimeConfig::new(&issuer_directory);
    let issuer = CoreRuntime::open(issuer_config.clone()).await.unwrap();
    let invitee = CoreRuntime::open(CoreRuntimeConfig::new(&invitee_directory))
        .await
        .unwrap();
    let workspace = issuer.create_workspace("Invitations").unwrap();

    let token = issuer
        .issue_invitation(workspace.id(), now_unix_ms() + 60_000)
        .unwrap();
    let claim = invitee.create_invitation_claim(&token).unwrap();
    let transition = issuer.redeem_invitation(&claim).unwrap();

    assert_eq!(transition.workspace_id(), workspace.id());
    assert_eq!(transition.epoch(), MembershipEpoch::new(1));
    let snapshot = issuer.membership_snapshot(workspace.id()).unwrap();
    assert_eq!(snapshot.members().len(), 2);
    assert!(snapshot.members().iter().any(|member| {
        member.identity() == invitee.public_identity() && member.role() == MembershipRole::Member
    }));

    let connection = database(&issuer_directory);
    let envelope: Vec<u8> = connection
        .query_row(
            "SELECT canonical_envelope FROM membership_operations WHERE action = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let claim_bytes = decode_hex(claim.encoded());
    assert!(
        envelope
            .windows(claim_bytes.len())
            .any(|bytes| bytes == claim_bytes)
    );
    assert_eq!(
        CoreRuntime::authenticate_membership_operation(&envelope).unwrap(),
        transition.cause()
    );
    drop(connection);
    issuer.shutdown().await.unwrap();
    drop(issuer);

    let reopened = CoreRuntime::open(issuer_config).await.unwrap();
    assert_eq!(
        reopened.membership_snapshot(workspace.id()).unwrap(),
        snapshot
    );
    let recovered = reopened.redeem_invitation(&claim).unwrap();
    assert_eq!(recovered.cause(), transition.cause());
    assert_eq!(recovered.epoch(), transition.epoch());

    reopened.shutdown().await.unwrap();
    drop(reopened);
    invitee.shutdown().await.unwrap();
    drop(invitee);
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(invitee_directory).unwrap();
}

#[tokio::test]
async fn invalid_expiry_wrong_issuer_wrong_workspace_and_malformed_proofs_are_rejected() {
    let issuer_directory = private_data_directory("invitation-validation-issuer");
    let issuer_copy_directory = private_data_directory("invitation-validation-copy");
    let invitee_directory = private_data_directory("invitation-validation-invitee");
    let issuer_config = CoreRuntimeConfig::new(&issuer_directory);
    let issuer = CoreRuntime::open(issuer_config.clone()).await.unwrap();
    fs::create_dir_all(&issuer_copy_directory).unwrap();
    fs::copy(
        issuer_directory.join("core.sqlite3"),
        issuer_copy_directory.join("core.sqlite3"),
    )
    .unwrap();
    let workspace = issuer.create_workspace("Validation").unwrap();
    assert_eq!(
        issuer
            .issue_invitation(
                WorkspaceId::from_str("00000000000000000000000000000000").unwrap(),
                now_unix_ms() + 60_000,
            )
            .unwrap_err(),
        InvitationError::WorkspaceNotFound
    );
    assert_eq!(
        issuer
            .issue_invitation(workspace.id(), now_unix_ms() - 1)
            .unwrap_err(),
        InvitationError::Expired
    );
    let token = issuer
        .issue_invitation(workspace.id(), now_unix_ms() + 60_000)
        .unwrap();
    let invitee = CoreRuntime::open(CoreRuntimeConfig::new(&invitee_directory))
        .await
        .unwrap();
    let claim = invitee.create_invitation_claim(&token).unwrap();

    assert_eq!(
        invitee.redeem_invitation(&claim).unwrap_err(),
        InvitationError::WrongIssuer
    );
    let issuer_without_workspace =
        CoreRuntime::open(CoreRuntimeConfig::new(&issuer_copy_directory))
            .await
            .unwrap();
    assert_eq!(
        issuer_without_workspace
            .redeem_invitation(&claim)
            .unwrap_err(),
        InvitationError::WorkspaceNotFound
    );

    let mut tampered_token = token.encoded().to_owned();
    tamper_hex_byte(&mut tampered_token, 140);
    assert!(InvitationToken::from_str(&tampered_token).is_err());
    let mut changed_target = claim.encoded().to_owned();
    tamper_hex_byte(&mut changed_target, 400);
    assert!(InvitationClaim::from_str(&changed_target).is_err());
    assert!(InvitationClaim::from_str(token.encoded()).is_err());

    issuer_without_workspace.shutdown().await.unwrap();
    drop(issuer_without_workspace);
    invitee.shutdown().await.unwrap();
    drop(invitee);
    issuer.shutdown().await.unwrap();
    drop(issuer);
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(issuer_copy_directory).unwrap();
    fs::remove_dir_all(invitee_directory).unwrap();
}

#[tokio::test]
async fn an_expired_token_is_not_consumed() {
    let issuer_directory = private_data_directory("invitation-expired-issuer");
    let invitee_directory = private_data_directory("invitation-expired-invitee");
    let issuer = CoreRuntime::open(CoreRuntimeConfig::new(&issuer_directory))
        .await
        .unwrap();
    let invitee = CoreRuntime::open(CoreRuntimeConfig::new(&invitee_directory))
        .await
        .unwrap();
    let workspace = issuer.create_workspace("Expiry").unwrap();
    let token = issuer
        .issue_invitation(workspace.id(), now_unix_ms() + 5)
        .unwrap();
    let claim = invitee.create_invitation_claim(&token).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));

    assert_eq!(
        issuer.redeem_invitation(&claim).unwrap_err(),
        InvitationError::Expired
    );
    let connection = database(&issuer_directory);
    let counts: (i64, i64, i64) = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM used_invitation_nonces),
                (SELECT COUNT(*) FROM membership_operations WHERE action = 1),
                (SELECT epoch FROM workspace_membership_state LIMIT 1)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(counts, (0, 0, 0));

    drop(connection);
    invitee.shutdown().await.unwrap();
    drop(invitee);
    issuer.shutdown().await.unwrap();
    drop(issuer);
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(invitee_directory).unwrap();
}

#[tokio::test]
async fn an_admitted_non_manager_cannot_issue_an_invitation() {
    let issuer_directory = private_data_directory("invitation-non-manager-issuer");
    let invitee_directory = private_data_directory("invitation-non-manager-invitee");
    let issuer_config = CoreRuntimeConfig::new(&issuer_directory);
    let issuer = CoreRuntime::open(issuer_config.clone()).await.unwrap();
    let invitee = CoreRuntime::open(CoreRuntimeConfig::new(&invitee_directory))
        .await
        .unwrap();
    let workspace = issuer.create_workspace("Manager only").unwrap();
    let token = issuer
        .issue_invitation(workspace.id(), now_unix_ms() + 60_000)
        .unwrap();
    let claim = invitee.create_invitation_claim(&token).unwrap();
    issuer.redeem_invitation(&claim).unwrap();
    issuer.shutdown().await.unwrap();
    drop(issuer);
    invitee.shutdown().await.unwrap();
    drop(invitee);

    let invitee_connection = database(&invitee_directory);
    let invitee_secret: Vec<u8> = invitee_connection
        .query_row(
            "SELECT secret_key FROM installation_identity WHERE singleton_id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(invitee_connection);
    let connection = database(&issuer_directory);
    connection
        .execute(
            "UPDATE installation_identity SET secret_key = ?1 WHERE singleton_id = 1",
            [invitee_secret],
        )
        .unwrap();
    drop(connection);

    let member_runtime = CoreRuntime::open(issuer_config).await.unwrap();
    assert_eq!(
        member_runtime
            .issue_invitation(workspace.id(), now_unix_ms() + 60_000)
            .unwrap_err(),
        InvitationError::NotManager
    );

    member_runtime.shutdown().await.unwrap();
    drop(member_runtime);
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(invitee_directory).unwrap();
}
