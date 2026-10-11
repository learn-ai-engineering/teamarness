use serde::{Deserialize, Serialize};
use teamarness_core::{
    MembershipError, OnboardingError, PersonNameError, StorageError, WorkspaceError,
};
use teamarness_package_host::{PackageErrorCode, PackageErrorEnvelope};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeError {
    pub code: BridgeErrorCode,
    pub retryable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeErrorCode {
    InvalidInput,
    Authentication,
    Expired,
    AlreadyUsed,
    Readiness,
    Timeout,
    Overload,
    InvalidBootstrap,
    LocalConflict,
    AdmissionNoLongerCurrent,
    UncertainStorageOutcome,
    /// A Package Instance could not be activated for a Workspace. The Workspace
    /// itself is usable; some of its Packages are not.
    PackageReconciliation,
    Shutdown,
}

impl BridgeError {
    pub const fn new(code: BridgeErrorCode, retryable: bool) -> Self {
        Self { code, retryable }
    }

    pub const fn invalid_input() -> Self {
        Self::new(BridgeErrorCode::InvalidInput, false)
    }

    pub const fn shutdown() -> Self {
        Self::new(BridgeErrorCode::Shutdown, true)
    }

    /// Reports a Package reconciliation failure as what it is. Only a Host that
    /// is shutting down becomes [`BridgeErrorCode::Shutdown`]; every other
    /// failure says that Package activation failed, and carries the Host's own
    /// retryability.
    pub fn reconcile(error: &PackageErrorEnvelope) -> Self {
        match error.code {
            PackageErrorCode::ShuttingDown => Self::shutdown(),
            _ => Self::new(BridgeErrorCode::PackageReconciliation, error.retryable),
        }
    }
}

impl From<OnboardingError> for BridgeError {
    fn from(value: OnboardingError) -> Self {
        let retryable = value.is_retryable();
        let code = match value {
            OnboardingError::InvalidInput => BridgeErrorCode::InvalidInput,
            OnboardingError::Authentication => BridgeErrorCode::Authentication,
            OnboardingError::Expired => BridgeErrorCode::Expired,
            OnboardingError::AlreadyUsed => BridgeErrorCode::AlreadyUsed,
            OnboardingError::Readiness => BridgeErrorCode::Readiness,
            OnboardingError::Timeout => BridgeErrorCode::Timeout,
            OnboardingError::Overload => BridgeErrorCode::Overload,
            OnboardingError::InvalidBootstrap => BridgeErrorCode::InvalidBootstrap,
            OnboardingError::LocalConflict => BridgeErrorCode::LocalConflict,
            OnboardingError::AdmissionNoLongerCurrent => BridgeErrorCode::AdmissionNoLongerCurrent,
            OnboardingError::UncertainStorageOutcome => BridgeErrorCode::UncertainStorageOutcome,
            OnboardingError::Shutdown => BridgeErrorCode::Shutdown,
            _ => BridgeErrorCode::InvalidInput,
        };
        Self::new(code, retryable)
    }
}

impl From<PersonNameError> for BridgeError {
    fn from(value: PersonNameError) -> Self {
        match value {
            PersonNameError::InvalidName
            | PersonNameError::WorkspaceNotFound
            | PersonNameError::NotOwnPerson => Self::invalid_input(),
            PersonNameError::Storage(error) => error.into(),
        }
    }
}

impl From<WorkspaceError> for BridgeError {
    fn from(value: WorkspaceError) -> Self {
        match value {
            WorkspaceError::InvalidName => Self::invalid_input(),
            WorkspaceError::Conflict => Self::new(BridgeErrorCode::LocalConflict, false),
            WorkspaceError::CorruptTrustAnchor => {
                Self::new(BridgeErrorCode::InvalidBootstrap, false)
            }
            WorkspaceError::Storage(error) => error.into(),
            _ => Self::invalid_input(),
        }
    }
}

impl From<MembershipError> for BridgeError {
    fn from(value: MembershipError) -> Self {
        match value {
            MembershipError::WorkspaceNotFound => Self::invalid_input(),
            MembershipError::Storage(error) => error.into(),
            _ => Self::new(BridgeErrorCode::InvalidBootstrap, false),
        }
    }
}

impl From<StorageError> for BridgeError {
    fn from(value: StorageError) -> Self {
        match value {
            StorageError::UncertainOutcome | StorageError::RuntimePoisoned => {
                Self::new(BridgeErrorCode::UncertainStorageOutcome, false)
            }
            _ => Self::new(BridgeErrorCode::LocalConflict, false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use teamarness_package_host::{DiagnosticId, PackageErrorStage, RecoveryAction};

    #[test]
    fn maps_every_public_onboarding_error_to_the_shared_fixture() {
        let errors = [
            OnboardingError::InvalidInput,
            OnboardingError::Authentication,
            OnboardingError::Expired,
            OnboardingError::AlreadyUsed,
            OnboardingError::Readiness,
            OnboardingError::Timeout,
            OnboardingError::Overload,
            OnboardingError::InvalidBootstrap,
            OnboardingError::LocalConflict,
            OnboardingError::AdmissionNoLongerCurrent,
            OnboardingError::UncertainStorageOutcome,
            OnboardingError::Shutdown,
        ];
        let actual = serde_json::to_value(errors.map(BridgeError::from)).unwrap();
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../contracts/fixtures/bridge-contract.json"
        ))
        .unwrap();
        assert_eq!(actual, fixture["errors"]);
    }

    /// A reconcile failure that is not a Host shutdown must not tell the person
    /// that Teamarness is shutting down.
    #[test]
    fn a_reconcile_failure_is_reported_as_itself_not_as_shutdown() {
        let failed = PackageErrorEnvelope::new(
            PackageErrorCode::CrashLoop,
            PackageErrorStage::Process,
            false,
            RecoveryAction::RecoverBackend,
            DiagnosticId::new("diag-1").unwrap(),
        );
        assert_eq!(
            BridgeError::reconcile(&failed),
            BridgeError::new(BridgeErrorCode::PackageReconciliation, false)
        );

        let retryable = PackageErrorEnvelope::new(
            PackageErrorCode::DeadlineExceeded,
            PackageErrorStage::Process,
            true,
            RecoveryAction::RecoverBackend,
            DiagnosticId::new("diag-2").unwrap(),
        );
        assert_eq!(
            BridgeError::reconcile(&retryable),
            BridgeError::new(BridgeErrorCode::PackageReconciliation, true)
        );

        let stopping = PackageErrorEnvelope::new(
            PackageErrorCode::ShuttingDown,
            PackageErrorStage::Shutdown,
            true,
            RecoveryAction::None,
            DiagnosticId::new("diag-3").unwrap(),
        );
        assert_eq!(BridgeError::reconcile(&stopping), BridgeError::shutdown());
    }

    #[test]
    fn structured_errors_never_serialize_internal_text() {
        let encoded =
            serde_json::to_string(&BridgeError::from(OnboardingError::Authentication)).unwrap();
        assert_eq!(encoded, r#"{"code":"authentication","retryable":false}"#);
    }
}
