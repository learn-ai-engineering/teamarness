use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::Connection;

use super::super::wire::decode_success;
use crate::membership::projection::ImportBoundary;
use crate::{
    CoreRuntime, CoreRuntimeConfig, MembershipVersion, OnboardingError, WorkspaceInvitation,
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn directory(name: &str) -> std::path::PathBuf {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "teamarness-import-{name}-{}-{sequence}",
        std::process::id()
    ))
}

async fn response_fixture(
    name: &str,
) -> (
    std::path::PathBuf,
    std::path::PathBuf,
    CoreRuntime,
    CoreRuntime,
    WorkspaceInvitation,
    super::super::wire::SuccessResponse,
) {
    let issuer_directory = directory(&format!("{name}-issuer"));
    let joiner_directory = directory(&format!("{name}-joiner"));
    let issuer = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&issuer_directory))
        .await
        .unwrap();
    let joiner = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&joiner_directory))
        .await
        .unwrap();
    let workspace = issuer.create_workspace("Imported Workspace").unwrap();
    let invitation = issuer
        .create_workspace_invitation(workspace.id())
        .await
        .unwrap();
    let claim = joiner
        .create_claim_for_invitation_test(&invitation)
        .unwrap();
    let frame = issuer
        .admit_and_export_for_test(&claim, joiner.public_identity(), None)
        .unwrap();
    let response = decode_success(&frame).unwrap();
    (
        issuer_directory,
        joiner_directory,
        issuer,
        joiner,
        invitation,
        response,
    )
}

fn workspace_row_count(directory: &std::path::Path) -> i64 {
    Connection::open(directory.join("core.sqlite3"))
        .unwrap()
        .query_row("SELECT COUNT(*) FROM workspaces", [], |row| row.get(0))
        .unwrap()
}

fn membership_operation_row_count(directory: &std::path::Path) -> i64 {
    Connection::open(directory.join("core.sqlite3"))
        .unwrap()
        .query_row("SELECT COUNT(*) FROM membership_operations", [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[tokio::test]
async fn initial_replay_and_transition_verification_overruns_time_out_and_roll_back() {
    let (issuer_directory, joiner_directory, issuer, joiner, invitation, response) =
        response_fixture("initial-replay-deadline").await;

    for boundary in [
        ImportBoundary::LocalReplay,
        ImportBoundary::TransitionVerification,
    ] {
        let error = joiner
            .import_response_with_deadline_delay_for_test(
                &invitation,
                response.clone(),
                boundary,
                std::time::Duration::from_millis(50),
                std::time::Duration::from_millis(5),
            )
            .unwrap_err();

        assert_eq!(error, OnboardingError::Timeout);
        assert_eq!(workspace_row_count(&joiner_directory), 0);
    }

    issuer.shutdown().await.unwrap();
    joiner.shutdown().await.unwrap();
    drop((issuer, joiner));
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(joiner_directory).unwrap();
}

#[tokio::test]
async fn final_success_path_overrun_times_out_and_rolls_back() {
    let (issuer_directory, joiner_directory, issuer, joiner, invitation, response) =
        response_fixture("final-success-deadline").await;

    let error = joiner
        .import_response_with_deadline_delay_for_test(
            &invitation,
            response,
            ImportBoundary::BeforeCommit,
            std::time::Duration::from_millis(50),
            std::time::Duration::from_millis(5),
        )
        .unwrap_err();

    assert_eq!(error, OnboardingError::Timeout);
    assert_eq!(workspace_row_count(&joiner_directory), 0);

    issuer.shutdown().await.unwrap();
    joiner.shutdown().await.unwrap();
    drop((issuer, joiner));
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(joiner_directory).unwrap();
}

#[tokio::test]
async fn post_commit_deadline_overrun_is_uncertain_until_authenticated_reopen() {
    let (issuer_directory, joiner_directory, issuer, joiner, invitation, response) =
        response_fixture("post-commit-deadline").await;

    let expected_operation_rows = i64::try_from(response.post_genesis.len() + 1).unwrap();
    let error = joiner
        .import_response_with_deadline_delay_for_test(
            &invitation,
            response.clone(),
            ImportBoundary::AfterCommit,
            std::time::Duration::from_secs(2),
            std::time::Duration::from_secs(1),
        )
        .unwrap_err();

    assert_eq!(error, OnboardingError::UncertainStorageOutcome);
    assert_eq!(workspace_row_count(&joiner_directory), 1);
    assert_eq!(
        joiner.list_workspaces().unwrap_err(),
        crate::WorkspaceError::Storage(crate::StorageError::RuntimePoisoned)
    );

    joiner.shutdown().await.unwrap();
    drop(joiner);
    let reopened = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&joiner_directory))
        .await
        .unwrap();
    let reconciled = reopened
        .import_response_for_test(&invitation, response, None)
        .unwrap();
    assert_eq!(reopened.list_workspaces().unwrap(), vec![reconciled.0]);
    assert_eq!(workspace_row_count(&joiner_directory), 1);
    assert_eq!(
        membership_operation_row_count(&joiner_directory),
        expected_operation_rows
    );

    issuer.shutdown().await.unwrap();
    reopened.shutdown().await.unwrap();
    drop((issuer, reopened));
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(joiner_directory).unwrap();
}

#[tokio::test]
async fn absent_workspace_exact_reimport_and_authenticated_reopen_are_idempotent() {
    let (issuer_directory, joiner_directory, issuer, joiner, invitation, response) =
        response_fixture("idempotent").await;
    let (workspace, snapshot) = joiner
        .import_response_for_test(&invitation, response.clone(), None)
        .unwrap();
    assert_eq!(workspace.id(), response.workspace_id);
    assert_eq!(snapshot.version(), response.membership_version);
    assert!(
        snapshot
            .members()
            .iter()
            .any(|member| member.identity() == joiner.public_identity())
    );

    let repeated = joiner
        .import_response_for_test(&invitation, response.clone(), None)
        .unwrap();
    assert_eq!(repeated.0, workspace);
    assert_eq!(repeated.1, snapshot);
    assert_eq!(workspace_row_count(&joiner_directory), 1);

    joiner.shutdown().await.unwrap();
    drop(joiner);
    let reopened = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&joiner_directory))
        .await
        .unwrap();
    assert_eq!(reopened.list_workspaces().unwrap(), vec![workspace]);
    assert_eq!(
        reopened.membership_snapshot(response.workspace_id).unwrap(),
        snapshot
    );

    issuer.shutdown().await.unwrap();
    reopened.shutdown().await.unwrap();
    drop((issuer, reopened));
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(joiner_directory).unwrap();
}

#[tokio::test]
async fn local_and_response_strict_prefixes_reconcile_without_replacement() {
    let (issuer_directory, joiner_directory, issuer, joiner, invitation, original) =
        response_fixture("prefix").await;
    let original_snapshot = joiner
        .import_response_for_test(&invitation, original.clone(), None)
        .unwrap()
        .1;

    let later_directory = directory("prefix-later");
    let later = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&later_directory))
        .await
        .unwrap();
    let later_invitation = issuer
        .create_workspace_invitation(original.workspace_id)
        .await
        .unwrap();
    let later_claim = later
        .create_claim_for_invitation_test(&later_invitation)
        .unwrap();
    issuer
        .admit_and_export_for_test(&later_claim, later.public_identity(), None)
        .unwrap();
    let original_claim = joiner
        .create_claim_for_invitation_test(&invitation)
        .unwrap();
    let current = decode_success(
        &issuer
            .admit_and_export_for_test(&original_claim, joiner.public_identity(), None)
            .unwrap(),
    )
    .unwrap();

    let appended = joiner
        .import_response_for_test(&invitation, current.clone(), None)
        .unwrap()
        .1;
    assert!(appended.epoch() > original_snapshot.epoch());
    assert_eq!(appended.version(), current.membership_version);
    let no_op = joiner
        .import_response_for_test(&invitation, original, None)
        .unwrap()
        .1;
    assert_eq!(no_op, appended);
    assert_eq!(workspace_row_count(&joiner_directory), 1);

    issuer.shutdown().await.unwrap();
    joiner.shutdown().await.unwrap();
    later.shutdown().await.unwrap();
    drop((issuer, joiner, later));
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(joiner_directory).unwrap();
    fs::remove_dir_all(later_directory).unwrap();
}

#[tokio::test]
async fn immutable_name_conflict_rejects_without_replacing_local_state() {
    let (issuer_directory, joiner_directory, issuer, joiner, invitation, response) =
        response_fixture("name-conflict").await;
    let stored = joiner
        .import_response_for_test(&invitation, response.clone(), None)
        .unwrap();
    let mut renamed = response;
    renamed.name = "Different Name".to_owned();
    assert_eq!(
        joiner
            .import_response_for_test(&invitation, renamed, None)
            .unwrap_err(),
        OnboardingError::LocalConflict
    );
    assert_eq!(joiner.list_workspaces().unwrap(), vec![stored.0]);
    assert_eq!(
        joiner.membership_snapshot(stored.1.workspace_id()).unwrap(),
        stored.1
    );

    issuer.shutdown().await.unwrap();
    joiner.shutdown().await.unwrap();
    drop((issuer, joiner));
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(joiner_directory).unwrap();
}

#[tokio::test]
async fn changed_canonical_bytes_for_one_id_fail_authenticated_reopen() {
    let (issuer_directory, joiner_directory, issuer, joiner, invitation, response) =
        response_fixture("changed-bytes").await;
    joiner
        .import_response_for_test(&invitation, response, None)
        .unwrap();
    joiner.shutdown().await.unwrap();
    drop(joiner);

    let connection = Connection::open(joiner_directory.join("core.sqlite3")).unwrap();
    connection
        .execute("DROP TRIGGER membership_operations_no_update", [])
        .unwrap();
    let mut bytes: Vec<u8> = connection
        .query_row(
            "SELECT canonical_envelope FROM membership_operations WHERE action = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    connection
        .execute(
            "UPDATE membership_operations SET canonical_envelope = ?1 WHERE action = 1",
            [bytes],
        )
        .unwrap();
    drop(connection);
    assert!(matches!(
        CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&joiner_directory)).await,
        Err(crate::CoreError::Storage(
            crate::StorageError::CorruptWorkspace
        ))
    ));

    issuer.shutdown().await.unwrap();
    drop(issuer);
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(joiner_directory).unwrap();
}

#[tokio::test]
async fn validation_rejects_anchor_name_genesis_admission_version_order_and_membership_faults() {
    let (issuer_directory, joiner_directory, issuer, joiner, invitation, response) =
        response_fixture("validation").await;

    let mut wrong_workspace = response.clone();
    wrong_workspace.workspace_id = crate::WorkspaceId::from_bytes([91; 16]);
    let mut wrong_creator = response.clone();
    wrong_creator.creator = joiner.public_identity();
    let mut wrong_name = response.clone();
    wrong_name.name = " Changed".to_owned();
    let mut wrong_genesis = response.clone();
    wrong_genesis.genesis_id = response.admission_id;
    let mut wrong_admission = response.clone();
    wrong_admission.admission_id = response.genesis_id;
    let mut wrong_version = response.clone();
    wrong_version.membership_version = MembershipVersion::from_bytes([77; 32]);
    let mut duplicate_operation = response.clone();
    duplicate_operation
        .post_genesis
        .push(response.genesis.clone());
    let mut missing_dependency = response.clone();
    missing_dependency.post_genesis[0].dependencies =
        vec![crate::MembershipOperationId::digest(b"missing dependency")];
    let mut invalid_signature = response.clone();
    invalid_signature.post_genesis[0].signature = iroh_base::Signature::from_bytes(&[0; 64]);

    for (case, invalid) in [
        ("workspace", wrong_workspace),
        ("creator", wrong_creator),
        ("name", wrong_name),
        ("genesis", wrong_genesis),
        ("admission", wrong_admission),
        ("version", wrong_version),
        ("duplicate", duplicate_operation),
        ("dependency", missing_dependency),
        ("signature", invalid_signature),
    ] {
        assert!(
            matches!(
                joiner.import_response_for_test(&invitation, invalid, None),
                Err(OnboardingError::InvalidBootstrap)
            ),
            "case {case} was accepted"
        );
        assert_eq!(workspace_row_count(&joiner_directory), 0);
    }

    issuer
        .revoke_member(response.workspace_id, joiner.public_identity())
        .unwrap();
    let no_longer_current = issuer
        .export_current_bootstrap_for_test(response.workspace_id, response.admission_id)
        .unwrap();
    assert_eq!(
        joiner
            .import_response_for_test(&invitation, no_longer_current, None)
            .unwrap_err(),
        OnboardingError::InvalidBootstrap
    );
    assert_eq!(workspace_row_count(&joiner_directory), 0);

    issuer.shutdown().await.unwrap();
    joiner.shutdown().await.unwrap();
    drop((issuer, joiner));
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(joiner_directory).unwrap();
}

#[tokio::test]
async fn every_import_write_group_rolls_back_and_uncertain_commit_poisons_until_reopen() {
    let boundaries = [
        ImportBoundary::LocalReplay,
        ImportBoundary::TransitionVerification,
        ImportBoundary::CreationRequest,
        ImportBoundary::WorkspaceAnchor,
        ImportBoundary::Operations,
        ImportBoundary::Dependencies,
        ImportBoundary::IdentitySubjects,
        ImportBoundary::Transitions,
        ImportBoundary::CurrentState,
        ImportBoundary::MemberSnapshot,
        ImportBoundary::BeforeCommit,
        ImportBoundary::AfterCommit,
    ];
    for (index, boundary) in boundaries.into_iter().enumerate() {
        let (issuer_directory, joiner_directory, issuer, joiner, invitation, response) =
            response_fixture(&format!("fault-{index}")).await;
        let error = joiner
            .import_response_for_test(&invitation, response.clone(), Some(boundary))
            .unwrap_err();
        if boundary == ImportBoundary::AfterCommit {
            assert_eq!(error, OnboardingError::UncertainStorageOutcome);
            assert_eq!(workspace_row_count(&joiner_directory), 1);
            assert_eq!(
                joiner.list_workspaces().unwrap_err(),
                crate::WorkspaceError::Storage(crate::StorageError::RuntimePoisoned)
            );
            joiner.shutdown().await.unwrap();
            drop(joiner);
            let reopened =
                CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(&joiner_directory))
                    .await
                    .unwrap();
            assert_eq!(reopened.list_workspaces().unwrap().len(), 1);
            assert_eq!(
                reopened
                    .import_response_for_test(&invitation, response, None)
                    .unwrap()
                    .1
                    .version(),
                reopened
                    .membership_snapshot(invitation.verified().unwrap().token.workspace_id)
                    .unwrap()
                    .version()
            );
            reopened.shutdown().await.unwrap();
            drop(reopened);
        } else {
            assert_eq!(error, OnboardingError::LocalConflict);
            assert_eq!(workspace_row_count(&joiner_directory), 0);
            joiner.shutdown().await.unwrap();
            drop(joiner);
            if boundary == ImportBoundary::BeforeCommit {
                let reopened = CoreRuntime::open_for_onboarding_test(CoreRuntimeConfig::new(
                    &joiner_directory,
                ))
                .await
                .unwrap();
                assert_eq!(
                    reopened
                        .import_response_for_test(&invitation, response, None)
                        .unwrap()
                        .1
                        .version(),
                    reopened
                        .membership_snapshot(invitation.verified().unwrap().token.workspace_id)
                        .unwrap()
                        .version()
                );
                reopened.shutdown().await.unwrap();
                drop(reopened);
            }
        }
        issuer.shutdown().await.unwrap();
        drop(issuer);
        fs::remove_dir_all(issuer_directory).unwrap();
        fs::remove_dir_all(joiner_directory).unwrap();
    }
}
