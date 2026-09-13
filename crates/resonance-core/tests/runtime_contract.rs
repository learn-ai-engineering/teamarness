use std::path::PathBuf;
use std::str::FromStr;

use resonance_core::{
    CoreRuntimeConfig, IdentityError, MembershipEpoch, MembershipOperationId, MembershipVersion,
    OnboardingError, SigningContext, WorkspaceId,
};

#[test]
fn runtime_config_preserves_the_private_data_directory() {
    let data_directory = PathBuf::from("/tmp/resonance-contract");

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
        SigningContext::new("resonance.🔑").unwrap_err(),
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
