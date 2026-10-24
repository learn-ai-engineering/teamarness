//! Package admission, durable continuity, and supervised Package Processes.
//!
//! [`PackageHost`] is the actor facade. Source policy, process handles, protocol
//! tasks, leases, data paths, and dependency types remain private.

#![forbid(unsafe_code)]

mod admission;
mod authority;
#[cfg(feature = "test-hooks")]
mod test_hooks;
#[cfg(feature = "test-hooks")]
pub use test_hooks::AuthorityPause;
mod broker;
mod port_observation;
pub use port_observation::{PortObservation, PortObservations, PortStage};
mod capability;
mod catalog;
mod commands;
mod content_views;
mod data_directory;
mod diag;
mod errors;
mod host;
mod host_store;
mod instance;
mod manifest;
mod protocol;
mod subscriptions;
mod supervisor;
mod ui;
mod values;

use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::{Mutex, watch};

pub use crate::admission::PortAvailability;
pub use crate::commands::{
    AcknowledgeUiEvents, CancelUiRequest, CloseUiMount, CloseUiSubscription, DisposePackage,
    OpenUiMount, OpenUiSubscription, PackageHostCommand, PackageHostOutcome, ReadContent,
    ReadUiResource, ReconcileWorkspaces, RecoverPackage, ReportUiFault, ShutdownHost,
    SuspendPackage, UiLoaderDescriptor, UiPackageContext, UiRequest, UiWorkspaceContext,
};
pub use crate::errors::{
    MAX_PACKAGE_ERROR_DETAILS_BYTES, MAX_PACKAGE_ERROR_DETAILS_DEPTH, PackageErrorCode,
    PackageErrorEnvelope, PackageErrorStage, PackageOwnedError, RecoveryAction,
};
pub use crate::subscriptions::{
    HostStateNotification, PackageHostNotification, PackageHostSubscription,
    PackageHostSubscriptionSelector, UiEventNotification, UiStreamCompletedNotification,
};
pub use crate::ui::MAX_UI_SUBSCRIPTIONS_PER_MOUNT;
pub use crate::values::{
    ContentViewToken, DiagnosticId, HostRevision, InstallationPublicIdentity, LifecycleState,
    MountState, PackageCatalogEntry, PackageHostConfig, PackageHostSnapshot, PackageHostState,
    PackageIdentity, PackageInstanceKey, PackageInstanceSnapshot, PackageSourceKind,
    PackageTrustKind, PackageValue, UiMountId, UiRequestId, UiResourceToken, UiSubscriptionId,
    ValueParseError, WorkspaceId,
};

use crate::errors::host_error;

/// Complete read-only port declaration metadata for authoring tools, separate from UI snapshots
/// and from the generation-bound, availability-filtered runtime view.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthoringCatalogEntry {
    pub package: PackageCatalogEntry,
    pub provided: Vec<teamarness_package_contract::port_contract::ProvidedContract>,
    pub definitions: serde_json::Map<String, serde_json::Value>,
    pub consumed: Vec<teamarness_package_contract::manifest::ConsumedContract>,
    pub availability: Vec<PortAvailability>,
    pub diagnostics: Vec<PackageErrorEnvelope>,
}

use crate::subscriptions::PackageHostSubscriptionKind;

/// Actor facade for admitted and supervised Package Instances.
#[derive(Clone)]
pub struct PackageHost {
    inner: Arc<PackageHostInner>,
}

struct PackageHostInner {
    calls: supervisor::CallEnvironment,
    store: host_store::HostStoreHandle,
    data: Mutex<HostData>,
    revisions: watch::Sender<HostRevision>,
    config: PackageHostConfig,
    core: Arc<teamarness_core::CoreRuntime>,
    templates: Vec<supervisor::ProcessTemplate>,
    admitted_ui: BTreeMap<PackageIdentity, Arc<ui::AdmittedUi>>,
    authoring_catalog: Vec<AuthoringCatalogEntry>,
    port_graph: BTreeMap<PackageIdentity, Vec<PortAvailability>>,
    port_broker: Arc<broker::PortBroker>,
    port_contracts: BTreeMap<
        PackageIdentity,
        Arc<teamarness_package_contract::port_contract::CompiledPortContracts>,
    >,
    reconciliation: Mutex<()>,
    mount_management: Mutex<()>,
}

struct HostData {
    state: PackageHostState,
    revision: HostRevision,
    packages: Vec<PackageCatalogEntry>,
    diagnostics: Vec<PackageErrorEnvelope>,
    instances: BTreeMap<PackageInstanceKey, Arc<instance::InstanceActor>>,
    mounts: BTreeMap<UiMountId, MountLease>,
    ui_routes: BTreeMap<(UiMountId, UiSubscriptionId), Option<protocol::UiEventReceiver>>,
}

#[derive(Clone)]
struct MountLease {
    instance: PackageInstanceKey,
    process_generation: u64,
    token: UiResourceToken,
    ui: Arc<ui::AdmittedUi>,
}

#[allow(clippy::result_large_err)]
impl PackageHost {
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

    /// Returns the installed, admitted declarations without starting any Provider.
    /// Includes installation-wide admission diagnostics and optional availability.
    pub fn authoring_catalog(&self) -> Vec<AuthoringCatalogEntry> {
        let mut entries = self.inner.authoring_catalog.clone();
        for entry in &mut entries {
            if let Some(availability) = self.inner.port_graph.get(&entry.package.identity) {
                entry.availability = availability.clone();
            }
            if let Some(compiled) = self.inner.port_contracts.get(&entry.package.identity) {
                entry.definitions = compiled.definitions.clone();
            }
        }
        entries
    }

    /// Opt in to bounded, process-local Port routing observations. A slow receiver reports lag.
    /// Observations are diagnostic only and cannot establish whether a command committed.
    pub fn observe_ports(&self) -> PortObservations {
        self.inner.port_broker.observe()
    }

    /// Claims a coalesced host-state subscription or one open UI event route.
    pub async fn subscribe(
        &self,
        selector: PackageHostSubscriptionSelector,
    ) -> Result<PackageHostSubscription, PackageErrorEnvelope> {
        let kind = match selector {
            PackageHostSubscriptionSelector::HostState { after_revision } => {
                PackageHostSubscriptionKind::HostState {
                    receiver: self.inner.revisions.subscribe(),
                    after: after_revision,
                }
            }
            PackageHostSubscriptionSelector::UiEvents {
                mount,
                subscription,
            } => {
                let receiver = self
                    .inner
                    .data
                    .lock()
                    .await
                    .ui_routes
                    .get_mut(&(mount.clone(), subscription.clone()))
                    .and_then(Option::take)
                    .ok_or_else(|| {
                        host_error(PackageErrorCode::StaleMount, PackageErrorStage::Ui)
                    })?;
                PackageHostSubscriptionKind::UiEvents {
                    receiver,
                    host: self.clone(),
                    mount,
                    subscription,
                }
            }
        };
        Ok(PackageHostSubscription { kind })
    }

    /// Executes one typed host command.
    pub async fn execute(
        &self,
        command: PackageHostCommand,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        self.ensure_not_stopped().await?;
        match command {
            PackageHostCommand::ReconcileWorkspaces(input) => {
                self.reconcile_workspaces(input).await
            }
            PackageHostCommand::Suspend(input) => self.suspend_instance(input).await,
            PackageHostCommand::Recover(input) => self.recover_instance(input).await,
            PackageHostCommand::Dispose(input) => self.dispose_instance(input).await,
            PackageHostCommand::OpenUiMount(input) => self.open_ui_mount(input).await,
            PackageHostCommand::CloseUiMount(input) => self.close_ui_mount(input).await,
            PackageHostCommand::UiRequest(input) => self.ui_request(input).await,
            PackageHostCommand::CancelUiRequest(input) => self.cancel_ui_request(input).await,
            PackageHostCommand::OpenUiSubscription(input) => self.open_ui_subscription(input).await,
            PackageHostCommand::AcknowledgeUiEvents(input) => {
                self.acknowledge_ui_events(input).await
            }
            PackageHostCommand::CloseUiSubscription(input) => {
                self.close_ui_subscription(input).await
            }
            PackageHostCommand::ReportUiFault(input) => self.report_ui_fault(input).await,
            PackageHostCommand::ReadUiResource(input) => self.read_ui_resource(input).await,
            PackageHostCommand::ReadContent(input) => self.read_content(input).await,
            PackageHostCommand::Shutdown(_) => self.shutdown_host().await,
        }
    }

    /// Refuses every command once the Host has stopped.
    async fn ensure_not_stopped(&self) -> Result<(), PackageErrorEnvelope> {
        if self.inner.data.lock().await.state == PackageHostState::Stopped {
            return Err(host_error(
                PackageErrorCode::ShuttingDown,
                PackageErrorStage::Shutdown,
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
extern crate self as teamarness_package_host;

/// The Package fixtures the integration tests use, so a crate-internal test can
/// open a real Host without a second copy of the fixture writer.
#[cfg(test)]
#[path = "../tests/common/mod.rs"]
mod host_fixture;

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: TEAM.SYS.TRANS-R07
    #[tokio::test]
    async fn host_store_is_opened_once_across_spawn_and_shutdown() {
        let fixture = host_fixture::Fixture::node(&[
            ("one", "dev.teamarness.one"),
            ("two", "dev.teamarness.two"),
        ])
        .await;
        let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
            .await
            .unwrap();
        let instances = host.snapshot().await.instances;
        assert_eq!(instances.len(), 2, "both Packages activated");
        for instance in instances {
            host.execute(PackageHostCommand::OpenUiMount(OpenUiMount {
                instance: instance.key,
            }))
            .await
            .unwrap();
        }
        host.execute(PackageHostCommand::Shutdown(ShutdownHost))
            .await
            .unwrap();
        assert_eq!(
            host.inner.store.opens(),
            1,
            "the Host opened its store once, and every later use shares it"
        );
    }
}
