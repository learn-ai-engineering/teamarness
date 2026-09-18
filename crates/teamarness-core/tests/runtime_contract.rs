use std::path::PathBuf;
use std::str::FromStr;

use teamarness_core::{
    CoreRuntime, CoreRuntimeConfig, IdentityError, MembershipChange, MembershipEpoch,
    MembershipOperationId, MembershipVersion, OnboardingError, SigningContext, WorkspaceId,
};

/// Issues an invitation, waiting out the endpoint readiness this machine needs
/// under load.
///
/// Readiness is a real-network condition, not the behaviour under test, and the
/// deadline only bounds failure.
async fn invite(
    runtime: &CoreRuntime,
    workspace: WorkspaceId,
) -> teamarness_core::WorkspaceInvitation {
    let expiry = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match runtime.create_workspace_invitation(workspace).await {
            Ok(invitation) => return invitation,
            Err(OnboardingError::Readiness) if std::time::Instant::now() < expiry => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(error) => panic!("the invitation could not be issued: {error}"),
        }
    }
}

#[test]
fn runtime_config_preserves_the_private_data_directory() {
    let data_directory = PathBuf::from("/tmp/teamarness-contract");

    let config = CoreRuntimeConfig::new(&data_directory);

    assert_eq!(config.private_data_directory(), data_directory);
}

#[test]
fn domain_identifiers_round_trip_through_their_public_text_form() {
    let workspace = WorkspaceId::from_str("00112233445566778899aabbccddeeff").unwrap();
    let operation = MembershipOperationId::from_str(
        "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff",
    )
    .unwrap();
    let version = MembershipVersion::from_str(
        "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100",
    )
    .unwrap();

    assert_eq!(workspace.to_string(), "00112233445566778899aabbccddeeff");
    assert_eq!(operation.to_string().len(), 64);
    assert_eq!(version.to_string().len(), 64);
    assert!(WorkspaceId::from_str("not-an-id").is_err());
    assert!(WorkspaceId::from_str(&format!("aé{}", "a".repeat(29))).is_err());
}

#[test]
fn membership_epoch_is_an_explicit_local_cursor() {
    let epoch = MembershipEpoch::new(7);

    assert_eq!(epoch.value(), 7);
}

#[test]
fn signing_context_rejects_an_empty_or_non_ascii_domain() {
    assert_eq!(
        SigningContext::new("").unwrap_err(),
        IdentityError::InvalidSigningContext
    );
    assert_eq!(
        SigningContext::new("teamarness.🔑").unwrap_err(),
        IdentityError::InvalidSigningContext
    );
}

#[test]
fn onboarding_errors_have_a_stable_retryability_contract() {
    let cases = [
        (OnboardingError::InvalidInput, false),
        (OnboardingError::Authentication, false),
        (OnboardingError::Expired, false),
        (OnboardingError::AlreadyUsed, false),
        (OnboardingError::Readiness, true),
        (OnboardingError::Timeout, true),
        (OnboardingError::Overload, true),
        (OnboardingError::InvalidBootstrap, false),
        (OnboardingError::LocalConflict, false),
        (OnboardingError::AdmissionNoLongerCurrent, false),
        (OnboardingError::UncertainStorageOutcome, false),
        (OnboardingError::Shutdown, true),
    ];

    for (error, expected) in cases {
        assert_eq!(error.is_retryable(), expected, "{error:?}");
        assert!(!error.to_string().is_empty());
    }
}

// Verifies: TEAM.SYS.MEM-R05
#[tokio::test]
async fn durable_and_live_transitions_report_the_same_change_and_author() {
    let root = std::env::temp_dir().join(format!(
        "teamarness-transition-change-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let alice = CoreRuntime::open(CoreRuntimeConfig::new(root.join("alice")))
        .await
        .unwrap();
    let workspace = alice.create_workspace("Transition change").unwrap();
    let mut live = alice
        .subscribe_membership(workspace.id(), MembershipEpoch::new(0))
        .unwrap();

    let bob_invitation = invite(&alice, workspace.id()).await;
    let bob = CoreRuntime::open(CoreRuntimeConfig::new(root.join("bob")))
        .await
        .unwrap();
    bob.join_workspace(&bob_invitation).await.unwrap();
    let carol_invitation = invite(&alice, workspace.id()).await;
    let carol = CoreRuntime::open(CoreRuntimeConfig::new(root.join("carol")))
        .await
        .unwrap();
    carol.join_workspace(&carol_invitation).await.unwrap();
    alice
        .revoke_member(workspace.id(), bob.public_identity())
        .unwrap();

    let expected = vec![
        MembershipChange::Admitted {
            member: bob.public_identity(),
        },
        MembershipChange::Admitted {
            member: carol.public_identity(),
        },
        MembershipChange::Revoked {
            member: bob.public_identity(),
        },
    ];

    let mut live_seen = Vec::new();
    while live_seen.len() < expected.len() {
        let transition = live
            .next_transition_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .expect("live transition");
        assert_eq!(transition.author(), alice.public_identity());
        live_seen.push(transition.change());
    }
    assert_eq!(live_seen, expected);

    let mut durable = alice
        .subscribe_membership(workspace.id(), MembershipEpoch::new(0))
        .unwrap();
    let mut durable_seen = Vec::new();
    while durable_seen.len() < expected.len() {
        let transition = durable
            .next_transition_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .expect("durable transition");
        assert_eq!(transition.author(), alice.public_identity());
        durable_seen.push(transition.change());
    }
    assert_eq!(durable_seen, expected);

    drop(live);
    drop(durable);
    carol.shutdown().await.unwrap();
    bob.shutdown().await.unwrap();
    alice.shutdown().await.unwrap();
    drop(carol);
    drop(bob);
    drop(alice);
    std::fs::remove_dir_all(root).unwrap();
}
