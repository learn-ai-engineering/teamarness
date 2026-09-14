//! Package source admission, durable continuity, and the Package Host value contract.
//!
//! Source parsing, schema compilation, storage, and data-directory allocation stay
//! private. Operational host behavior lands in the next package-foundation phase.

#![forbid(unsafe_code)]

// Phase 2 builds these private adapters before Phase 3 wires them into PackageHost::open.
#[allow(dead_code)]
mod admission;
#[allow(dead_code)]
mod catalog;
#[allow(dead_code)]
mod data_directory;
#[allow(dead_code)]
mod host_store;
#[allow(dead_code)]
mod manifest;
#[allow(dead_code)]
mod ui;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A failure to parse one of the Package Host's public string values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueParseError {
    expected: &'static str,
}

impl ValueParseError {
    const fn new(expected: &'static str) -> Self {
        Self { expected }
    }
}

impl fmt::Display for ValueParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid {}", self.expected)
    }
}

impl std::error::Error for ValueParseError {}

/// A stable lowercase reverse-DNS Package Identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PackageIdentity(String);

impl PackageIdentity {
    /// Returns the validated identity text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PackageIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for PackageIdentity {
    type Err = ValueParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if !(3..=128).contains(&value.len()) || !value.is_ascii() {
            return Err(ValueParseError::new("package identity"));
        }
        let labels: Vec<_> = value.split('.').collect();
        if labels.len() < 2
            || labels.iter().any(|label| {
                label.is_empty()
                    || label.len() > 63
                    || !label.as_bytes()[0].is_ascii_lowercase()
                    || label.ends_with('-')
                    || !label.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                    })
            })
        {
            return Err(ValueParseError::new("package identity"));
        }
        Ok(Self(value.to_owned()))
    }
}

impl TryFrom<String> for PackageIdentity {
    type Error = ValueParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<PackageIdentity> for String {
    fn from(value: PackageIdentity) -> Self {
        value.0
    }
}

macro_rules! bounded_id {
    ($name:ident, $expected:literal) => {
        #[doc = concat!("A validated ", $expected, ".")]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// Validates and constructs the value.
            pub fn new(value: impl Into<String>) -> Result<Self, ValueParseError> {
                let value = value.into();
                if value.is_empty()
                    || matches!(value.as_str(), "." | "..")
                    || value.len() > 128
                    || !value.is_ascii()
                    || !value.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
                    })
                {
                    return Err(ValueParseError::new($expected));
                }
                Ok(Self(value))
            }

            /// Returns the validated text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl TryFrom<String> for $name {
            type Error = ValueParseError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

bounded_id!(InstallationPublicIdentity, "installation identity");
bounded_id!(WorkspaceId, "workspace identity");
bounded_id!(UiMountId, "UI mount identity");
bounded_id!(UiRequestId, "UI request identity");
bounded_id!(UiSubscriptionId, "UI subscription identity");
bounded_id!(DiagnosticId, "diagnostic identity");

/// One Package Identity and Workspace pair inside the opened Installation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageInstanceKey {
    /// Stable Package Identity.
    pub package_identity: PackageIdentity,
    /// Workspace bound to this Package Instance.
    pub workspace_id: WorkspaceId,
}

impl PackageInstanceKey {
    /// Constructs a Package Instance key.
    #[must_use]
    pub const fn new(package_identity: PackageIdentity, workspace_id: WorkspaceId) -> Self {
        Self {
            package_identity,
            workspace_id,
        }
    }
}

/// Configuration values needed to open Package Host storage and source registries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageHostConfig {
    application_root: PathBuf,
    application_index: PathBuf,
    private_data_directory: PathBuf,
    installation_public_identity: InstallationPublicIdentity,
    target: String,
}

impl PackageHostConfig {
    /// Creates a configuration for the supported Apple silicon target.
    #[must_use]
    pub fn new(
        application_root: impl Into<PathBuf>,
        application_index: impl Into<PathBuf>,
        private_data_directory: impl Into<PathBuf>,
        installation_public_identity: InstallationPublicIdentity,
    ) -> Self {
        Self {
            application_root: application_root.into(),
            application_index: application_index.into(),
            private_data_directory: private_data_directory.into(),
            installation_public_identity,
            target: "aarch64-apple-darwin".to_owned(),
        }
    }

    /// Returns the application resource root.
    #[must_use]
    pub fn application_root(&self) -> &Path {
        &self.application_root
    }

    /// Returns the build-generated application Package index.
    #[must_use]
    pub fn application_index(&self) -> &Path {
        &self.application_index
    }

    /// Returns the Package Host private data root.
    #[must_use]
    pub fn private_data_directory(&self) -> &Path {
        &self.private_data_directory
    }

    /// Returns the opened Installation's Public Identity.
    #[must_use]
    pub const fn installation_public_identity(&self) -> &InstallationPublicIdentity {
        &self.installation_public_identity
    }

    /// Returns the exact supported Backend target.
    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }
}

/// A dependency-neutral JSON value used at the public bridge boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PackageValue {
    /// JSON null.
    Null,
    /// JSON boolean.
    Boolean(bool),
    /// JSON signed integer.
    Integer(i64),
    /// JSON unsigned integer.
    Unsigned(u64),
    /// JSON finite floating-point number.
    Float(f64),
    /// JSON string.
    String(String),
    /// JSON array.
    Array(Vec<Self>),
    /// JSON object.
    Object(BTreeMap<String, Self>),
}

/// Monotonic revision of an authoritative Package Host snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HostRevision(pub u64);

impl HostRevision {
    /// Revision of the first empty snapshot.
    pub const INITIAL: Self = Self(0);
}

/// Package Backend lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    Admitted,
    Activating,
    Active,
    Suspending,
    Suspended,
    Recovering,
    Failed,
    CrashLoop,
    Disposing,
    Disposed,
}

impl LifecycleState {
    /// Returns the stable wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::Activating => "activating",
            Self::Active => "active",
            Self::Suspending => "suspending",
            Self::Suspended => "suspended",
            Self::Recovering => "recovering",
            Self::Failed => "failed",
            Self::CrashLoop => "crash_loop",
            Self::Disposing => "disposing",
            Self::Disposed => "disposed",
        }
    }
}

/// Package UI Mount state, independent of Backend lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MountState {
    Opening,
    Mounted,
    Failed,
    Closing,
    Closed,
}

impl MountState {
    /// Returns the stable wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Opening => "opening",
            Self::Mounted => "mounted",
            Self::Failed => "failed",
            Self::Closing => "closing",
            Self::Closed => "closed",
        }
    }
}

/// Origin of an admitted Package Source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageSourceKind {
    Bundled,
    Local,
}

/// Trust granted by Package Admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageTrustKind {
    ApplicationBundled,
    TrustedNativeCodeV1,
}

/// Public catalog entry. Filesystem paths and loader URLs stay private.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageCatalogEntry {
    pub identity: PackageIdentity,
    pub version: String,
    pub display_name: String,
    pub source: PackageSourceKind,
    pub trust: PackageTrustKind,
    pub admission_digest: String,
    pub ui_contract_digest: String,
}

/// Public state for one Package Instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageInstanceSnapshot {
    pub key: PackageInstanceKey,
    pub lifecycle: LifecycleState,
    pub process_generation: Option<u64>,
    pub mount_state: Option<MountState>,
}

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
            version: 1,
            namespace: "teamarness.package-host".to_owned(),
            code,
            stage,
            affected_package: None,
            affected_instance: None,
            retryable,
            recovery,
            diagnostic_id,
        }
    }
}

/// Overall state of the Package Host actor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageHostState {
    Opening,
    Running,
    ShuttingDown,
    Stopped,
    Failed,
}

/// Authoritative, URL-neutral Package Host state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageHostSnapshot {
    pub revision: HostRevision,
    pub state: PackageHostState,
    pub packages: Vec<PackageCatalogEntry>,
    pub instances: Vec<PackageInstanceSnapshot>,
    pub diagnostics: Vec<PackageErrorEnvelope>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileWorkspaces {
    pub workspaces: Vec<WorkspaceId>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuspendPackage {
    pub instance: PackageInstanceKey,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoverPackage {
    pub instance: PackageInstanceKey,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisposePackage {
    pub instance: PackageInstanceKey,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenUiMount {
    pub instance: PackageInstanceKey,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseUiMount {
    pub mount: UiMountId,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiRequest {
    pub mount: UiMountId,
    pub request: UiRequestId,
    pub operation: String,
    pub payload: PackageValue,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelUiRequest {
    pub mount: UiMountId,
    pub request: UiRequestId,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenUiSubscription {
    pub mount: UiMountId,
    pub subscription: UiSubscriptionId,
    pub operation: String,
    pub payload: PackageValue,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcknowledgeUiEvents {
    pub mount: UiMountId,
    pub subscription: UiSubscriptionId,
    pub through_sequence: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseUiSubscription {
    pub mount: UiMountId,
    pub subscription: UiSubscriptionId,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportUiFault {
    pub mount: UiMountId,
    pub code: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShutdownHost;

/// Complete closed command vocabulary for the operational Package Host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "input", rename_all = "snake_case")]
pub enum PackageHostCommand {
    ReconcileWorkspaces(ReconcileWorkspaces),
    Suspend(SuspendPackage),
    Recover(RecoverPackage),
    Dispose(DisposePackage),
    OpenUiMount(OpenUiMount),
    CloseUiMount(CloseUiMount),
    UiRequest(UiRequest),
    CancelUiRequest(CancelUiRequest),
    OpenUiSubscription(OpenUiSubscription),
    AcknowledgeUiEvents(AcknowledgeUiEvents),
    CloseUiSubscription(CloseUiSubscription),
    ReportUiFault(ReportUiFault),
    Shutdown(ShutdownHost),
}

/// Complete matching outcome vocabulary for [`PackageHostCommand`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PackageHostOutcome {
    Reconciled { revision: HostRevision },
    Suspended { revision: HostRevision },
    Recovered { revision: HostRevision },
    Disposed { revision: HostRevision },
    UiMountOpened { revision: HostRevision },
    UiMountClosed { revision: HostRevision },
    UiResponse { payload: PackageValue },
    UiRequestCancelled,
    UiSubscriptionOpened,
    UiEventsAcknowledged,
    UiSubscriptionClosed,
    UiFaultRecorded { revision: HostRevision },
    Shutdown,
}
