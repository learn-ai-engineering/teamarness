//! The Package Host error vocabulary and the envelope every command returns.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::protocol;
use crate::values::{DiagnosticId, PackageIdentity, PackageInstanceKey};

/// Stable Package Host error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageErrorCode {
    InvalidRegistration,
    SourceNotFound,
    ManifestMalformed,
    ManifestUnsupported,
    InvalidPackageIdentity,
    IdentityMismatch,
    DuplicateIdentity,
    OverrideRequired,
    TrustEvidenceMissing,
    TargetUnsupported,
    ArtifactMissing,
    ArtifactChanged,
    DataDirectoryUnavailable,
    DataDirectoryPermission,
    DataDirectoryUnrepresentable,
    CapabilityUnavailable,
    CapabilityVersionUnsupported,
    CapabilityRequestInvalid,
    SpawnFailed,
    HandshakeTimeout,
    HandshakeMismatch,
    MalformedFrame,
    FrameTooLarge,
    ProtocolViolation,
    Overloaded,
    DeadlineExceeded,
    Cancelled,
    ProcessExited,
    OutcomeUncertain,
    OrphanUnresolved,
    CrashLoop,
    InvalidLifecycleState,
    BackendError,
    Disposed,
    ShuttingDown,
    UiEntrypointMissing,
    UiContractViolation,
    PortContractViolation,
    UiLoadFailed,
    UiException,
    StaleMount,
    EventLagged,
}

/// Stage that emitted a stable Package Host error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageErrorStage {
    Discovery,
    Admission,
    Storage,
    Capability,
    Process,
    Protocol,
    Lifecycle,
    Ui,
    Shutdown,
}

/// Safe caller action for a Package Host failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAction {
    CorrectRegistration,
    CorrectPackageSource,
    RestorePermissions,
    RestartApplication,
    RecoverBackend,
    ReloadUi,
    Retry,
    None,
}

/// A Package-owned terminal error, forwarded to the Package UI unchanged except for bounds.
///
/// The Host's own `code` stays authoritative; this is the Package's own vocabulary beside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageOwnedError {
    /// Frame-validated against `^[a-z][a-z0-9_.-]{0,127}$`.
    pub code: String,
    pub retryable: bool,
    pub details: serde_json::Map<String, serde_json::Value>,
}

/// The largest compact JSON encoding of Package error details the Host forwards.
pub const MAX_PACKAGE_ERROR_DETAILS_BYTES: usize = 4096;
/// The deepest Package error details the Host forwards.
pub const MAX_PACKAGE_ERROR_DETAILS_DEPTH: usize = 8;

/// Versioned, stable, payload-free Package Host error envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageErrorEnvelope {
    pub version: u16,
    pub namespace: String,
    pub code: PackageErrorCode,
    pub stage: PackageErrorStage,
    pub affected_package: Option<PackageIdentity>,
    pub affected_instance: Option<PackageInstanceKey>,
    pub retryable: bool,
    pub recovery: RecoveryAction,
    pub diagnostic_id: DiagnosticId,
    /// The Package's own error, when the Package owns this failure.
    pub package_error: Option<PackageOwnedError>,
}

impl PackageErrorEnvelope {
    /// Creates an error without exposing source paths, operating-system text, or payloads.
    #[must_use]
    pub fn new(
        code: PackageErrorCode,
        stage: PackageErrorStage,
        retryable: bool,
        recovery: RecoveryAction,
        diagnostic_id: DiagnosticId,
    ) -> Self {
        Self {
            version: 2,
            namespace: "teamarness.package-host".to_owned(),
            code,
            stage,
            affected_package: None,
            affected_instance: None,
            retryable,
            recovery,
            diagnostic_id,
            package_error: None,
        }
    }
}

pub(crate) fn host_error(code: PackageErrorCode, stage: PackageErrorStage) -> PackageErrorEnvelope {
    let retryable = matches!(
        code,
        PackageErrorCode::DeadlineExceeded
            | PackageErrorCode::ProcessExited
            | PackageErrorCode::BackendError
            | PackageErrorCode::Cancelled
    );
    let recovery = match code {
        PackageErrorCode::CrashLoop
        | PackageErrorCode::ProcessExited
        | PackageErrorCode::DeadlineExceeded
        | PackageErrorCode::OutcomeUncertain => RecoveryAction::RecoverBackend,
        PackageErrorCode::StaleMount
        | PackageErrorCode::UiException
        | PackageErrorCode::UiContractViolation
        | PackageErrorCode::EventLagged
        | PackageErrorCode::UiLoadFailed => RecoveryAction::ReloadUi,
        PackageErrorCode::ShuttingDown => RecoveryAction::None,
        _ => RecoveryAction::CorrectPackageSource,
    };
    let mut random = [0_u8; 12];
    let _ = getrandom::fill(&mut random);
    let text = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    PackageErrorEnvelope::new(
        code,
        stage,
        retryable,
        recovery,
        DiagnosticId::new(format!("diag-{text}")).expect("hex diagnostic ID"),
    )
}

/// Builds the envelope for a Package → Host failure, carrying the Package's own error when the
/// Package owns it.
pub(crate) fn wire_error(
    failure: &protocol::WireFailure,
    stage: PackageErrorStage,
) -> PackageErrorEnvelope {
    let mut error = host_error(failure.code, stage);
    if let Some(package_error) = &failure.package_error {
        error.retryable = package_error.retryable;
        error.package_error = Some(package_error.clone());
    }
    error
}

pub(crate) fn activation_error(
    failure: &protocol::WireFailure,
    instance: &PackageInstanceKey,
) -> PackageErrorEnvelope {
    let stage = if matches!(
        failure.code,
        PackageErrorCode::CapabilityUnavailable
            | PackageErrorCode::CapabilityVersionUnsupported
            | PackageErrorCode::CapabilityRequestInvalid
    ) {
        PackageErrorStage::Capability
    } else {
        PackageErrorStage::Process
    };
    let mut error = wire_error(failure, stage);
    error.affected_instance = Some(instance.clone());
    error
}

pub(crate) fn instance_error(
    failure: protocol::WireFailure,
    instance: &PackageInstanceKey,
) -> PackageErrorEnvelope {
    let mut error = wire_error(&failure, PackageErrorStage::Lifecycle);
    error.affected_instance = Some(instance.clone());
    error
}

impl fmt::Display for PackageErrorEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "teamarness.package-host:{:?}", self.code)
    }
}

impl std::error::Error for PackageErrorEnvelope {}
