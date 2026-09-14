//! Package admission, durable continuity, and supervised Package Processes.
//!
//! [`PackageHost`] is the actor facade. Source policy, process handles, protocol
//! tasks, leases, data paths, and dependency types remain private.

#![forbid(unsafe_code)]

mod admission;
mod capability;
mod catalog;
mod data_directory;
mod host_store;
mod instance;
mod manifest;
mod protocol;
mod supervisor;
mod ui;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, watch};

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
    Reconciled {
        revision: HostRevision,
    },
    Suspended {
        revision: HostRevision,
    },
    Recovered {
        revision: HostRevision,
    },
    Disposed {
        revision: HostRevision,
    },
    UiMountOpened {
        mount: UiMountId,
        revision: HostRevision,
    },
    UiMountClosed {
        revision: HostRevision,
    },
    UiResponse {
        payload: PackageValue,
    },
    UiRequestCancelled,
    UiSubscriptionOpened,
    UiEventsAcknowledged,
    UiSubscriptionClosed,
    UiFaultRecorded {
        revision: HostRevision,
    },
    Shutdown,
}

/// A coalesced host-state notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostStateNotification {
    pub revision: HostRevision,
    pub revision_gap: bool,
}

/// A bounded subscription to the latest Package Host revision.
pub struct PackageHostSubscription {
    receiver: watch::Receiver<HostRevision>,
    after: HostRevision,
}

#[allow(clippy::result_large_err)]
impl PackageHostSubscription {
    /// Waits for the newest revision. `revision_gap` requires a fresh snapshot.
    pub async fn changed(&mut self) -> Result<HostStateNotification, PackageErrorEnvelope> {
        let current = *self.receiver.borrow();
        let revision = if current > self.after {
            current
        } else {
            self.receiver.changed().await.map_err(|_| {
                host_error(PackageErrorCode::ShuttingDown, PackageErrorStage::Shutdown)
            })?;
            *self.receiver.borrow()
        };
        let notification = HostStateNotification {
            revision,
            revision_gap: revision.0 > self.after.0.saturating_add(1),
        };
        self.after = revision;
        Ok(notification)
    }
}

/// Actor facade for admitted and supervised Package Instances.
#[derive(Clone)]
pub struct PackageHost {
    inner: Arc<PackageHostInner>,
}

struct PackageHostInner {
    data: Mutex<HostData>,
    revisions: watch::Sender<HostRevision>,
    config: PackageHostConfig,
    core: Arc<teamarness_core::CoreRuntime>,
    templates: Vec<supervisor::ProcessTemplate>,
    reconciliation: Mutex<()>,
}

struct HostData {
    state: PackageHostState,
    revision: HostRevision,
    packages: Vec<PackageCatalogEntry>,
    diagnostics: Vec<PackageErrorEnvelope>,
    instances: BTreeMap<PackageInstanceKey, Arc<instance::InstanceActor>>,
    mounts: BTreeMap<UiMountId, PackageInstanceKey>,
}

#[allow(clippy::result_large_err)]
impl PackageHost {
    /// Opens host metadata, admits sources, reconciles leases, and eagerly activates
    /// every admitted Package for every Workspace known to Core.
    pub async fn open(
        config: PackageHostConfig,
        core: Arc<teamarness_core::CoreRuntime>,
    ) -> Result<Self, PackageErrorEnvelope> {
        let mut report = admission::admit(&config)
            .map_err(|failure| host_error(failure.code, PackageErrorStage::Admission))?;
        let unresolved = supervisor::reconcile_orphans(config.private_data_directory())
            .map_err(|code| host_error(code, PackageErrorStage::Process))?;
        for lease in &unresolved {
            let package = lease.package_identity.parse().ok();
            let mut error = host_error(
                PackageErrorCode::OrphanUnresolved,
                PackageErrorStage::Process,
            );
            error.affected_package = package;
            report.snapshot.diagnostics.push(error);
        }
        let blocked: std::collections::BTreeSet<_> = unresolved
            .iter()
            .map(|lease| (lease.package_identity.clone(), lease.workspace_id.clone()))
            .collect();
        let workspaces = core.list_workspaces().map_err(|_| {
            host_error(
                PackageErrorCode::DataDirectoryUnavailable,
                PackageErrorStage::Storage,
            )
        })?;
        let mut templates = Vec::new();
        for admission in report.admissions {
            let catalog = report
                .snapshot
                .packages
                .iter()
                .find(|entry| entry.identity == admission.manifest.identity)
                .expect("every admission has a catalog entry");
            let source_label = match catalog.source {
                PackageSourceKind::Bundled => "bundled",
                PackageSourceKind::Local => "local",
            };
            let trust_label = match catalog.trust {
                PackageTrustKind::ApplicationBundled => "application_bundled",
                PackageTrustKind::TrustedNativeCodeV1 => "trusted_native_code_v1",
            };
            templates.push(supervisor::ProcessTemplate {
                package_identity: admission.manifest.identity.clone(),
                package_version: admission.manifest.version.to_string(),
                manifest_digest: admission.manifest.manifest_digest.clone(),
                backend_digest: admission.manifest.backend.digest.clone(),
                ui_contract_digest: admission.manifest.ui.contract.digest.clone(),
                backend_path: admission.backend_path,
                private_data_directory: config.private_data_directory().to_owned(),
                source_label: source_label.to_owned(),
                trust_label: trust_label.to_owned(),
                protocol: (
                    admission.manifest.protocol.major,
                    admission.manifest.protocol.max_minor,
                ),
                capabilities: admission
                    .manifest
                    .capabilities
                    .iter()
                    .map(|requirement| capability::CapabilityRequirement {
                        identity: requirement.identity.clone(),
                        major: requirement.major,
                        min_minor: requirement.min_minor,
                        max_minor: requirement.max_minor,
                    })
                    .collect(),
            });
        }
        let mut instances = BTreeMap::new();
        for template in &templates {
            for workspace in &workspaces {
                let workspace_id = WorkspaceId::new(workspace.id().to_string()).map_err(|_| {
                    host_error(
                        PackageErrorCode::DataDirectoryUnrepresentable,
                        PackageErrorStage::Storage,
                    )
                })?;
                if blocked.contains(&(
                    template.package_identity.to_string(),
                    workspace_id.to_string(),
                )) {
                    continue;
                }
                let (key, actor) = prepare_instance(&config, &core, template, workspace).await?;
                if let Err(failure) = actor.activate().await {
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
                    let mut error = host_error(failure.code, stage);
                    error.affected_instance = Some(key.clone());
                    report.snapshot.diagnostics.push(error);
                }
                instances.insert(key, actor);
            }
        }
        let revision = HostRevision(1);
        let (revisions, _) = watch::channel(revision);
        let host = Self {
            inner: Arc::new(PackageHostInner {
                data: Mutex::new(HostData {
                    state: PackageHostState::Running,
                    revision,
                    packages: report.snapshot.packages,
                    diagnostics: report.snapshot.diagnostics,
                    instances,
                    mounts: BTreeMap::new(),
                }),
                revisions,
                config,
                core,
                templates,
                reconciliation: Mutex::new(()),
            }),
        };
        Ok(host)
    }

    /// Returns the authoritative Package Host state.
    pub async fn snapshot(&self) -> PackageHostSnapshot {
        let (state, revision, packages, diagnostics, instances) = {
            let data = self.inner.data.lock().await;
            (
                data.state,
                data.revision,
                data.packages.clone(),
                data.diagnostics.clone(),
                data.instances.values().cloned().collect::<Vec<_>>(),
            )
        };
        let mut snapshots = Vec::with_capacity(instances.len());
        for actor in instances {
            snapshots.push(actor.snapshot().await);
        }
        PackageHostSnapshot {
            revision,
            state,
            packages,
            instances: snapshots,
            diagnostics,
        }
    }

    /// Subscribes to coalesced state revisions after the caller's last snapshot.
    #[must_use]
    pub fn subscribe(&self, after_revision: HostRevision) -> PackageHostSubscription {
        PackageHostSubscription {
            receiver: self.inner.revisions.subscribe(),
            after: after_revision,
        }
    }

    /// Executes one typed host command.
    pub async fn execute(
        &self,
        command: PackageHostCommand,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        if self.inner.data.lock().await.state == PackageHostState::Stopped {
            return Err(host_error(
                PackageErrorCode::ShuttingDown,
                PackageErrorStage::Shutdown,
            ));
        }
        match command {
            PackageHostCommand::ReconcileWorkspaces(_) => {
                let _gate = self.inner.reconciliation.lock().await;
                let workspaces = self.inner.core.list_workspaces().map_err(|_| {
                    host_error(
                        PackageErrorCode::DataDirectoryUnavailable,
                        PackageErrorStage::Storage,
                    )
                })?;
                for template in &self.inner.templates {
                    for workspace in &workspaces {
                        let workspace_id =
                            WorkspaceId::new(workspace.id().to_string()).map_err(|_| {
                                host_error(
                                    PackageErrorCode::DataDirectoryUnrepresentable,
                                    PackageErrorStage::Storage,
                                )
                            })?;
                        let key = PackageInstanceKey::new(
                            template.package_identity.clone(),
                            workspace_id,
                        );
                        if self.inner.data.lock().await.instances.contains_key(&key) {
                            continue;
                        }
                        let (key, actor) = prepare_instance(
                            &self.inner.config,
                            &self.inner.core,
                            template,
                            workspace,
                        )
                        .await?;
                        if let Err(failure) = actor.activate().await {
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
                            let mut error = host_error(failure.code, stage);
                            error.affected_instance = Some(key.clone());
                            self.inner.data.lock().await.diagnostics.push(error);
                        }
                        self.inner.data.lock().await.instances.insert(key, actor);
                    }
                }
                let revision = self.bump_revision().await;
                Ok(PackageHostOutcome::Reconciled { revision })
            }
            PackageHostCommand::Suspend(input) => {
                let actor = self.actor(&input.instance).await?;
                actor
                    .suspend()
                    .await
                    .map_err(|failure| instance_error(failure, &input.instance))?;
                Ok(PackageHostOutcome::Suspended {
                    revision: self.bump_revision().await,
                })
            }
            PackageHostCommand::Recover(input) => {
                let actor = self.actor(&input.instance).await?;
                actor
                    .recover()
                    .await
                    .map_err(|failure| instance_error(failure, &input.instance))?;
                Ok(PackageHostOutcome::Recovered {
                    revision: self.bump_revision().await,
                })
            }
            PackageHostCommand::Dispose(input) => {
                let actor = self.actor(&input.instance).await?;
                actor
                    .dispose()
                    .await
                    .map_err(|failure| instance_error(failure, &input.instance))?;
                Ok(PackageHostOutcome::Disposed {
                    revision: self.bump_revision().await,
                })
            }
            PackageHostCommand::OpenUiMount(input) => {
                let actor = self.actor(&input.instance).await?;
                let mount = actor
                    .open_mount()
                    .await
                    .map_err(|failure| instance_error(failure, &input.instance))?;
                self.inner
                    .data
                    .lock()
                    .await
                    .mounts
                    .insert(mount.clone(), input.instance);
                Ok(PackageHostOutcome::UiMountOpened {
                    mount,
                    revision: self.bump_revision().await,
                })
            }
            PackageHostCommand::CloseUiMount(input) => {
                if let Some(actor) = self.actor_for_mount(&input.mount).await {
                    actor.close_mount(&input.mount).await;
                    self.inner.data.lock().await.mounts.remove(&input.mount);
                }
                Ok(PackageHostOutcome::UiMountClosed {
                    revision: self.bump_revision().await,
                })
            }
            PackageHostCommand::UiRequest(input) => {
                let actor = self.actor_for_mount(&input.mount).await.ok_or_else(|| {
                    host_error(PackageErrorCode::StaleMount, PackageErrorStage::Ui)
                })?;
                let payload = actor
                    .request(&input.mount, input.request, input.operation, input.payload)
                    .await
                    .map_err(|failure| host_error(failure.code, PackageErrorStage::Process))?;
                Ok(PackageHostOutcome::UiResponse { payload })
            }
            PackageHostCommand::CancelUiRequest(input) => {
                let actor = self.actor_for_mount(&input.mount).await.ok_or_else(|| {
                    host_error(PackageErrorCode::StaleMount, PackageErrorStage::Ui)
                })?;
                actor
                    .cancel(&input.request)
                    .await
                    .map_err(|failure| host_error(failure.code, PackageErrorStage::Protocol))?;
                Ok(PackageHostOutcome::UiRequestCancelled)
            }
            PackageHostCommand::OpenUiSubscription(input) => {
                let actor = self.actor_for_mount(&input.mount).await.ok_or_else(|| {
                    host_error(PackageErrorCode::StaleMount, PackageErrorStage::Ui)
                })?;
                actor
                    .stream_open(
                        &input.mount,
                        input.subscription,
                        input.operation,
                        input.payload,
                    )
                    .await
                    .map_err(|failure| host_error(failure.code, PackageErrorStage::Protocol))?;
                Ok(PackageHostOutcome::UiSubscriptionOpened)
            }
            PackageHostCommand::AcknowledgeUiEvents(input) => {
                let actor = self.actor_for_mount(&input.mount).await.ok_or_else(|| {
                    host_error(PackageErrorCode::StaleMount, PackageErrorStage::Ui)
                })?;
                actor
                    .acknowledge(&input.mount, &input.subscription, input.through_sequence)
                    .await
                    .map_err(|failure| host_error(failure.code, PackageErrorStage::Protocol))?;
                Ok(PackageHostOutcome::UiEventsAcknowledged)
            }
            PackageHostCommand::CloseUiSubscription(input) => {
                let actor = self.actor_for_mount(&input.mount).await.ok_or_else(|| {
                    host_error(PackageErrorCode::StaleMount, PackageErrorStage::Ui)
                })?;
                actor
                    .close_subscription(&input.mount, &input.subscription)
                    .await
                    .map_err(|failure| host_error(failure.code, PackageErrorStage::Protocol))?;
                Ok(PackageHostOutcome::UiSubscriptionClosed)
            }
            PackageHostCommand::ReportUiFault(input) => {
                if let Some(actor) = self.actor_for_mount(&input.mount).await {
                    actor.close_mount(&input.mount).await;
                    self.inner.data.lock().await.mounts.remove(&input.mount);
                }
                Ok(PackageHostOutcome::UiFaultRecorded {
                    revision: self.bump_revision().await,
                })
            }
            PackageHostCommand::Shutdown(_) => {
                {
                    let mut data = self.inner.data.lock().await;
                    data.state = PackageHostState::ShuttingDown;
                }
                self.bump_revision().await;
                let actors = self
                    .inner
                    .data
                    .lock()
                    .await
                    .instances
                    .values()
                    .cloned()
                    .collect::<Vec<_>>();
                for actor in actors {
                    actor.shutdown().await;
                }
                {
                    let mut data = self.inner.data.lock().await;
                    data.state = PackageHostState::Stopped;
                }
                self.bump_revision().await;
                Ok(PackageHostOutcome::Shutdown)
            }
        }
    }

    async fn actor(
        &self,
        key: &PackageInstanceKey,
    ) -> Result<Arc<instance::InstanceActor>, PackageErrorEnvelope> {
        self.inner
            .data
            .lock()
            .await
            .instances
            .get(key)
            .cloned()
            .ok_or_else(|| {
                host_error(
                    PackageErrorCode::InvalidLifecycleState,
                    PackageErrorStage::Lifecycle,
                )
            })
    }

    async fn actor_for_mount(&self, mount: &UiMountId) -> Option<Arc<instance::InstanceActor>> {
        let data = self.inner.data.lock().await;
        let key = data.mounts.get(mount)?;
        data.instances.get(key).cloned()
    }

    async fn bump_revision(&self) -> HostRevision {
        let mut data = self.inner.data.lock().await;
        data.revision.0 = data.revision.0.saturating_add(1);
        let revision = data.revision;
        self.inner.revisions.send_replace(revision);
        revision
    }
}

#[allow(clippy::result_large_err)]
async fn prepare_instance(
    config: &PackageHostConfig,
    core: &Arc<teamarness_core::CoreRuntime>,
    template: &supervisor::ProcessTemplate,
    workspace: &teamarness_core::WorkspaceSummary,
) -> Result<(PackageInstanceKey, Arc<instance::InstanceActor>), PackageErrorEnvelope> {
    let workspace_id = WorkspaceId::new(workspace.id().to_string()).map_err(|_| {
        host_error(
            PackageErrorCode::DataDirectoryUnrepresentable,
            PackageErrorStage::Storage,
        )
    })?;
    let key = PackageInstanceKey::new(template.package_identity.clone(), workspace_id.clone());
    let store = host_store::HostStore::open(config.private_data_directory())
        .map_err(|failure| host_error(failure.code, PackageErrorStage::Storage))?;
    let grant = data_directory::allocate(
        &store,
        config.private_data_directory(),
        config.installation_public_identity(),
        &workspace_id,
        &template.package_identity,
    )
    .map_err(|failure| host_error(failure.code, PackageErrorStage::Storage))?;
    let spec = supervisor::ProcessSpec {
        package_identity: template.package_identity.clone(),
        package_version: template.package_version.clone(),
        manifest_digest: template.manifest_digest.clone(),
        backend_digest: template.backend_digest.clone(),
        ui_contract_digest: template.ui_contract_digest.clone(),
        backend_path: template.backend_path.clone(),
        workspace_id,
        workspace_name: workspace.name().to_owned(),
        installation: config.installation_public_identity().clone(),
        data_directory: grant.path,
        continuity_id: grant.continuity_id,
        private_data_directory: template.private_data_directory.clone(),
        source_label: template.source_label.clone(),
        trust_label: template.trust_label.clone(),
        protocol: template.protocol,
        capabilities: template.capabilities.clone(),
        core: core.clone(),
    };
    Ok((
        key.clone(),
        Arc::new(instance::InstanceActor::new(key, spec)),
    ))
}

fn host_error(code: PackageErrorCode, stage: PackageErrorStage) -> PackageErrorEnvelope {
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
        PackageErrorCode::StaleMount | PackageErrorCode::UiException => RecoveryAction::ReloadUi,
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

fn instance_error(
    failure: protocol::WireFailure,
    instance: &PackageInstanceKey,
) -> PackageErrorEnvelope {
    let mut error = host_error(failure.code, PackageErrorStage::Lifecycle);
    error.affected_instance = Some(instance.clone());
    error
}

impl fmt::Display for PackageErrorEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "teamarness.package-host:{:?}", self.code)
    }
}

impl std::error::Error for PackageErrorEnvelope {}
