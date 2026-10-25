//! The closed Package Host command vocabulary and its matching outcomes.

use serde::{Deserialize, Serialize};

use crate::values::{
    HostRevision, LifecycleState, PackageIdentity, PackageInstanceKey, PackageSourceKind,
    PackageTrustKind, PackageUiView, PackageValue, UiMountId, UiRequestId, UiResourceToken,
    UiSubscriptionId, WorkspaceId,
};

/// Activates every admitted Package for every Workspace Core lists. Never disposes
/// instances; see open question `TEAM-DQ9`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileWorkspaces {}
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
/// Opens one UI mount of an Instance, showing `view`.
///
/// `view` defaults to `main`. Any other view must be declared in the Package's manifest, or the
/// mount is refused with `ui_view_not_declared`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenUiMount {
    pub instance: PackageInstanceKey,
    #[serde(default)]
    pub view: PackageUiView,
}

impl OpenUiMount {
    /// Opens the main view of `instance`.
    #[must_use]
    pub const fn main(instance: PackageInstanceKey) -> Self {
        Self {
            instance,
            view: PackageUiView::Main,
        }
    }
}

/// Package presentation values bound to one UI mount.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiPackageContext {
    pub identity: PackageIdentity,
    pub version: String,
    pub display_name: String,
    pub source: PackageSourceKind,
    pub trust: PackageTrustKind,
    pub lifecycle: LifecycleState,
    pub process_generation: u64,
}

/// Workspace presentation values bound to one UI mount.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiWorkspaceContext {
    pub id: WorkspaceId,
    pub name: String,
}

/// Opaque information needed to load one admitted Package UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiLoaderDescriptor {
    pub token: UiResourceToken,
    pub entrypoint: String,
    pub ui_host_version: u16,
    pub contract_digest: String,
    pub package: UiPackageContext,
    pub workspace: UiWorkspaceContext,
    /// The view this mount shows.
    pub view: PackageUiView,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadUiResource {
    pub mount: UiMountId,
    pub token: UiResourceToken,
    pub relative_path: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShutdownHost;

/// Registers and admits a Personal Package while the Host runs (ADR 0031).
///
/// The directory must inspect as `expected_identity`, the risks must be exactly
/// the `trusted_native_code_v1` set, and the identity must not belong to the
/// Team tier: replacing a Team Package is an override, applied on restart. An
/// already registered Personal identity is replaced by the new directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallPersonal {
    pub directory: std::path::PathBuf,
    pub expected_identity: PackageIdentity,
    pub accepted_risks: Vec<String>,
}

/// Re-admits a registered Personal Package from its directory, after a rebuild.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReloadPersonal {
    pub identity: PackageIdentity,
}

/// Stops a Personal Package and removes its registration. Its data is kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovePersonal {
    pub identity: PackageIdentity,
}

/// Replaces the running process of one Personal Package Instance with a new
/// process generation (ADR 0031). Team Instances are refused with
/// `override_required`; an Instance that is not running or failed is refused
/// with `invalid_lifecycle_state`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestartPersonal {
    pub instance: PackageInstanceKey,
}

/// Writes an override registration that replaces a Team Package at the next
/// application start (ADR 0031). No Instance, route, or mount changes in the
/// running Host; its snapshot shows the registration as
/// [`RegistrationStatus::PendingRestart`](crate::RegistrationStatus::PendingRestart)
/// at a new revision.
///
/// The identity must belong to a bundled Package, the directory must inspect as
/// `expected_identity`, and the risks must be exactly the
/// `trusted_native_code_v1` set. The write is serialized with the live
/// Personal operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteOverride {
    pub directory: std::path::PathBuf,
    pub expected_identity: PackageIdentity,
    pub accepted_risks: Vec<String>,
}

/// Removes the registration file entry for a Team Package identity, so the
/// bundled Package returns at the next application start. The entry is an
/// override, or one written by hand without `overrideApplication`, which the
/// Host refused with `override_required`. No Instance, route, or mount changes
/// in the running Host; its snapshot shows the registration as
/// [`RegistrationStatus::PendingRestart`](crate::RegistrationStatus::PendingRestart)
/// at a new revision, or drops it when the bundled Package already runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveOverride {
    pub identity: PackageIdentity,
}

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
    ReadUiResource(ReadUiResource),
    InstallPersonal(InstallPersonal),
    ReloadPersonal(ReloadPersonal),
    RemovePersonal(RemovePersonal),
    RestartPersonal(RestartPersonal),
    WriteOverride(WriteOverride),
    RemoveOverride(RemoveOverride),
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
        loader: UiLoaderDescriptor,
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
    UiResource {
        bytes: Vec<u8>,
        media_type: String,
    },
    PersonalInstalled {
        revision: HostRevision,
    },
    PersonalReloaded {
        revision: HostRevision,
    },
    PersonalRemoved {
        revision: HostRevision,
    },
    PersonalRestarted {
        revision: HostRevision,
    },
    /// The override is in the registration file and applies at the next start.
    OverrideWritten,
    /// The override is gone from the registration file; the bundled Package returns at the next start.
    OverrideRemoved,
    Shutdown,
}
