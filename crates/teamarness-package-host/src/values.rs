//! Public value types of the Package Host: identities, configuration, catalog
//! entries and the authoritative host snapshot.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::errors::PackageErrorEnvelope;
use crate::protocol;

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

// Keep in sync with Core's private synchronization::PRINCIPALS_SCOPE. The
// colon is outside the historical Package Identity grammar, but valid in Core
// record and peer codecs; this is never an admissible Package Identity.
pub(crate) const RESERVED_PRINCIPALS_SCOPE: &str = "internal:teamarness.principals";

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
        if value != RESERVED_PRINCIPALS_SCOPE
            && teamarness_package_contract::identity::is_valid_package_identity(value)
        {
            Ok(Self(value.to_owned()))
        } else {
            Err(ValueParseError::new("package identity"))
        }
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

/// A mount-scoped opaque UI resource token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct UiResourceToken(String);

impl UiResourceToken {
    /// Validates an exact 192-bit unpadded base64url token.
    pub fn new(value: impl Into<String>) -> Result<Self, ValueParseError> {
        let value = value.into();
        if value.len() != 32
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(ValueParseError::new("UI resource token"));
        }
        Ok(Self(value))
    }

    /// Returns the opaque token text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for UiResourceToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("UiResourceToken([redacted])")
    }
}

impl TryFrom<String> for UiResourceToken {
    type Error = ValueParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<UiResourceToken> for String {
    fn from(value: UiResourceToken) -> Self {
        value.0
    }
}

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
    pub(crate) capability_call_timeout: std::time::Duration,
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
            capability_call_timeout: protocol::ProtocolLimits::CURRENT.capability_call,
        }
    }

    /// Shortens the Package capability-call deadline so a test can observe it.
    #[doc(hidden)]
    #[must_use]
    pub fn with_capability_call_timeout_for_tests(mut self, timeout: std::time::Duration) -> Self {
        self.capability_call_timeout = timeout;
        self
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    pub source: PackageSourceKind,
    pub trust: PackageTrustKind,
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
    /// Every installation registration the Host read, with its tier and admission result.
    pub registrations: Vec<InstallationRegistrationSnapshot>,
}

/// The admission tier a Package belongs to (ADR 0031).
///
/// The Team tier is the bundled Packages plus installation registrations with
/// `overrideApplication`. It is fixed when the Host opens. The Personal tier is
/// every other installation registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageTier {
    Team,
    Personal,
}

/// Whether one installation registration was admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RegistrationStatus {
    Admitted,
    Refused {
        code: crate::errors::PackageErrorCode,
    },
}

/// One entry of the installation registration file, as the Host last admitted it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationRegistrationSnapshot {
    /// The registered Package Directory, canonical when it could be resolved.
    pub directory: String,
    /// The Package Identity the registration expects, exactly as written.
    pub expected_identity: String,
    /// Whether the registration replaces a bundled Package.
    #[serde(rename = "override")]
    pub override_application: bool,
    pub tier: PackageTier,
    pub status: RegistrationStatus,
}

/// What registering an inspected Package Directory would do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InspectedPackageKind {
    /// No registration or bundled Package has this identity.
    New,
    /// The identity is already registered at this directory.
    Reload,
    /// The identity is already registered at a different directory.
    ReplacesRegistration,
    /// A bundled Package has this identity, so registering it needs an override.
    OverridesTeam,
}

/// The display metadata of an inspected Package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectedPresentation {
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
}

/// One Semantic Capability an inspected Package requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectedCapability {
    pub identity: String,
    pub major: u16,
    pub min_minor: u16,
    pub max_minor: u16,
}

/// A Package Directory read for a preview. Inspecting never runs Package code.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectedPackage {
    /// The canonical Package Directory.
    pub directory: PathBuf,
    pub identity: PackageIdentity,
    pub version: String,
    pub presentation: InspectedPresentation,
    pub capabilities: Vec<InspectedCapability>,
    pub provided: Vec<teamarness_package_contract::port_contract::ProvidedContract>,
    pub consumed: Vec<teamarness_package_contract::manifest::ConsumedContract>,
    pub kind: InspectedPackageKind,
}

/// A request to register a Package Directory in the installation registration file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationRequest {
    /// An absolute path to the Package Directory.
    pub directory: PathBuf,
    /// The identity the person saw in the preview.
    pub expected_identity: PackageIdentity,
    /// The `trusted_native_code_v1` risks the person accepted.
    pub accepted_risks: Vec<String>,
    /// Whether the person confirmed replacing a bundled Package.
    pub override_application: bool,
}
