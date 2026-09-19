//! Opening the Host, and the Package Instance lifecycle commands.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use tokio::sync::{Mutex, watch};

use crate::errors::{
    PackageErrorCode, PackageErrorEnvelope, PackageErrorStage, activation_error, host_error,
    instance_error,
};
use crate::values::{
    HostRevision, PackageHostConfig, PackageHostState, PackageIdentity, PackageInstanceKey,
    PackageSourceKind, PackageTrustKind, WorkspaceId,
};
use crate::{
    DisposePackage, HostData, PackageHost, PackageHostInner, PackageHostOutcome,
    ReconcileWorkspaces, RecoverPackage, SuspendPackage, admission, capability, data_directory,
    host_store, instance, protocol, supervisor, ui,
};

/// A Package Identity and Workspace pair whose orphan lease is unresolved, so no
/// Instance may be activated for it.
type BlockedInstances = BTreeSet<(String, String)>;

/// The spawn templates and admitted UI bundles one admission report yields.
type AdmittedPackages = (
    Vec<supervisor::ProcessTemplate>,
    BTreeMap<PackageIdentity, Arc<ui::AdmittedUi>>,
);

#[allow(clippy::result_large_err)]
impl PackageHost {
    /// Opens host metadata, admits sources, reconciles leases, and eagerly activates
    /// every admitted Package for every Workspace known to Core.
    pub async fn open(
        config: PackageHostConfig,
        core: Arc<teamarness_core::CoreRuntime>,
    ) -> Result<Self, PackageErrorEnvelope> {
        let store = Self::open_host_store(&config).await?;
        let mut report = Self::admit_sources(&config, &store).await?;
        let blocked = Self::reconcile_orphan_leases(&store, &mut report).await?;
        let workspaces = Self::list_core_workspaces(&core).await?;
        let (templates, admitted_ui) = Self::build_process_templates(&config, &mut report)?;
        // One permit pool and one set of protocol bounds for every Package in
        // this Host.
        let calls = supervisor::CallEnvironment {
            limiter: capability::CoreCallLimiter::new(),
            limits: protocol::ProtocolLimits {
                capability_call: config.capability_call_timeout,
            },
        };
        let instances = Self::activate_initial_instances(
            &config,
            &core,
            &calls,
            &store,
            &templates,
            &workspaces,
            &blocked,
            &mut report.snapshot.diagnostics,
        )
        .await?;
        Ok(Self::assemble(
            config,
            core,
            store,
            calls,
            templates,
            admitted_ui,
            report.snapshot.packages,
            report.snapshot.diagnostics,
            instances,
        ))
    }

    /// One store for the whole Host: opening runs directory checks and the
    /// migration batch, and every use after this crosses to a blocking thread.
    async fn open_host_store(
        config: &PackageHostConfig,
    ) -> Result<host_store::HostStoreHandle, PackageErrorEnvelope> {
        host_store::HostStoreHandle::open(config.private_data_directory().to_owned())
            .await
            .map_err(|failure| host_error(failure.code, PackageErrorStage::Storage))
    }

    async fn admit_sources(
        config: &PackageHostConfig,
        store: &host_store::HostStoreHandle,
    ) -> Result<admission::AdmissionReport, PackageErrorEnvelope> {
        let admitting = config.clone();
        store
            .run(move |store| admission::admit(&admitting, store))
            .await
            .map_err(|failure| host_error(failure.code, PackageErrorStage::Storage))?
            .map_err(|failure| host_error(failure.code, PackageErrorStage::Admission))
    }

    /// Records one diagnostic per unresolved orphan lease and reports the pairs
    /// that must not be activated.
    async fn reconcile_orphan_leases(
        store: &host_store::HostStoreHandle,
        report: &mut admission::AdmissionReport,
    ) -> Result<BlockedInstances, PackageErrorEnvelope> {
        let unresolved = store
            .run(supervisor::reconcile_orphans)
            .await
            .map_err(|failure| host_error(failure.code, PackageErrorStage::Storage))?
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
        Ok(unresolved
            .iter()
            .map(|lease| (lease.package_identity.clone(), lease.workspace_id.clone()))
            .collect())
    }

    async fn list_core_workspaces(
        core: &Arc<teamarness_core::CoreRuntime>,
    ) -> Result<Vec<teamarness_core::WorkspaceSummary>, PackageErrorEnvelope> {
        let listing = core.clone();
        tokio::task::spawn_blocking(move || listing.list_workspaces())
            .await
            .map_err(|_| {
                host_error(
                    PackageErrorCode::DataDirectoryUnavailable,
                    PackageErrorStage::Storage,
                )
            })?
            .map_err(|_| {
                host_error(
                    PackageErrorCode::DataDirectoryUnavailable,
                    PackageErrorStage::Storage,
                )
            })
    }

    /// Turns every admission into the process template its Instances are spawned
    /// from, paired with the admitted UI bundle the mounts serve.
    fn build_process_templates(
        config: &PackageHostConfig,
        report: &mut admission::AdmissionReport,
    ) -> Result<AdmittedPackages, PackageErrorEnvelope> {
        let mut templates = Vec::new();
        let mut admitted_ui = BTreeMap::new();
        for admission in std::mem::take(&mut report.admissions) {
            debug_assert!(
                report
                    .snapshot
                    .packages
                    .iter()
                    .any(|entry| entry.identity == admission.manifest.identity),
                "every admission has a catalog entry"
            );
            let catalog = report
                .snapshot
                .packages
                .iter()
                .find(|entry| entry.identity == admission.manifest.identity)
                .ok_or_else(|| {
                    host_error(
                        PackageErrorCode::InvalidRegistration,
                        PackageErrorStage::Admission,
                    )
                })?;
            let source_label = match catalog.source {
                PackageSourceKind::Bundled => "bundled",
                PackageSourceKind::Local => "local",
            };
            let trust_label = match catalog.trust {
                PackageTrustKind::ApplicationBundled => "application_bundled",
                PackageTrustKind::TrustedNativeCodeV1 => "trusted_native_code_v1",
            };
            admitted_ui.insert(admission.manifest.identity.clone(), admission.ui.clone());
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
                        family: requirement.family.clone(),
                        major: requirement.major,
                        min_minor: requirement.min_minor,
                        max_minor: requirement.max_minor,
                    })
                    .collect(),
            });
        }
        Ok((templates, admitted_ui))
    }

    /// The eager activation loop that runs before the Host exists. It skips a
    /// pair whose orphan lease is unresolved, and collects activation failures as
    /// diagnostics rather than failing the open. `reconcile_workspaces` runs the
    /// same shape against a live Host, with a different skip rule; the two
    /// destinations and skip rules are why they are not one function.
    #[allow(clippy::too_many_arguments)]
    async fn activate_initial_instances(
        config: &PackageHostConfig,
        core: &Arc<teamarness_core::CoreRuntime>,
        calls: &supervisor::CallEnvironment,
        store: &host_store::HostStoreHandle,
        templates: &[supervisor::ProcessTemplate],
        workspaces: &[teamarness_core::WorkspaceSummary],
        blocked: &BlockedInstances,
        diagnostics: &mut Vec<PackageErrorEnvelope>,
    ) -> Result<BTreeMap<PackageInstanceKey, Arc<instance::InstanceActor>>, PackageErrorEnvelope>
    {
        let mut instances = BTreeMap::new();
        for template in templates {
            for workspace in workspaces {
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
                let (key, actor) =
                    prepare_instance(config, core, calls, store, template, workspace).await?;
                if let Err(failure) = actor.activate().await {
                    diagnostics.push(activation_error(&failure, &key));
                }
                instances.insert(key, actor);
            }
        }
        Ok(instances)
    }

    #[allow(clippy::too_many_arguments)]
    fn assemble(
        config: PackageHostConfig,
        core: Arc<teamarness_core::CoreRuntime>,
        store: host_store::HostStoreHandle,
        calls: supervisor::CallEnvironment,
        templates: Vec<supervisor::ProcessTemplate>,
        admitted_ui: BTreeMap<PackageIdentity, Arc<ui::AdmittedUi>>,
        packages: Vec<crate::PackageCatalogEntry>,
        diagnostics: Vec<PackageErrorEnvelope>,
        instances: BTreeMap<PackageInstanceKey, Arc<instance::InstanceActor>>,
    ) -> Self {
        let revision = HostRevision(1);
        let (revisions, _) = watch::channel(revision);
        Self {
            inner: Arc::new(PackageHostInner {
                calls,
                store,
                data: Mutex::new(HostData {
                    state: PackageHostState::Running,
                    revision,
                    packages,
                    diagnostics,
                    instances,
                    mounts: BTreeMap::new(),
                    ui_routes: BTreeMap::new(),
                }),
                revisions,
                config,
                core,
                templates,
                admitted_ui,
                reconciliation: Mutex::new(()),
                mount_management: Mutex::new(()),
            }),
        }
    }

    /// Activates every admitted Package for every Workspace Core now lists, and
    /// leaves every existing Instance alone.
    pub(crate) async fn reconcile_workspaces(
        &self,
        _input: ReconcileWorkspaces,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        let _gate = self.inner.reconciliation.lock().await;
        let workspaces = Self::list_core_workspaces(&self.inner.core).await?;
        for template in &self.inner.templates {
            for workspace in &workspaces {
                let workspace_id = WorkspaceId::new(workspace.id().to_string()).map_err(|_| {
                    host_error(
                        PackageErrorCode::DataDirectoryUnrepresentable,
                        PackageErrorStage::Storage,
                    )
                })?;
                let key = PackageInstanceKey::new(template.package_identity.clone(), workspace_id);
                if self.inner.data.lock().await.instances.contains_key(&key) {
                    continue;
                }
                let (key, actor) = prepare_instance(
                    &self.inner.config,
                    &self.inner.core,
                    &self.inner.calls,
                    &self.inner.store,
                    template,
                    workspace,
                )
                .await?;
                if let Err(failure) = actor.activate().await {
                    self.inner
                        .data
                        .lock()
                        .await
                        .diagnostics
                        .push(activation_error(&failure, &key));
                }
                self.inner.data.lock().await.instances.insert(key, actor);
            }
        }
        let revision = self.bump_revision().await;
        Ok(PackageHostOutcome::Reconciled { revision })
    }

    pub(crate) async fn suspend_instance(
        &self,
        input: SuspendPackage,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        let actor = self.actor(&input.instance).await?;
        actor
            .suspend()
            .await
            .map_err(|failure| instance_error(failure, &input.instance))?;
        Ok(PackageHostOutcome::Suspended {
            revision: self.bump_revision().await,
        })
    }

    pub(crate) async fn recover_instance(
        &self,
        input: RecoverPackage,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        let actor = self.actor(&input.instance).await?;
        actor
            .recover()
            .await
            .map_err(|failure| instance_error(failure, &input.instance))?;
        Ok(PackageHostOutcome::Recovered {
            revision: self.bump_revision().await,
        })
    }

    pub(crate) async fn dispose_instance(
        &self,
        input: DisposePackage,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        let _gate = self.inner.mount_management.lock().await;
        let actor = self.actor(&input.instance).await?;
        let result = actor.dispose().await;
        self.revoke_mounts_for_instance(&input.instance).await;
        result.map_err(|failure| instance_error(failure, &input.instance))?;
        Ok(PackageHostOutcome::Disposed {
            revision: self.bump_revision().await,
        })
    }

    pub(crate) async fn shutdown_host(&self) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        {
            let mut data = self.inner.data.lock().await;
            data.state = PackageHostState::ShuttingDown;
        }
        self.bump_revision().await;
        {
            let mut data = self.inner.data.lock().await;
            data.mounts.clear();
            data.ui_routes.clear();
        }
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

    pub(crate) async fn actor(
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

    pub(crate) async fn bump_revision(&self) -> HostRevision {
        let mut data = self.inner.data.lock().await;
        data.revision.0 = data.revision.0.saturating_add(1);
        let revision = data.revision;
        self.inner.revisions.send_replace(revision);
        revision
    }
}

#[allow(clippy::result_large_err)]
pub(crate) async fn prepare_instance(
    config: &PackageHostConfig,
    core: &Arc<teamarness_core::CoreRuntime>,
    calls: &supervisor::CallEnvironment,
    store: &host_store::HostStoreHandle,
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
    let private = config.private_data_directory().to_owned();
    let installation = config.installation_public_identity().clone();
    let allocating_workspace = workspace_id.clone();
    let allocating_package = template.package_identity.clone();
    let grant = store
        .run(move |store| {
            data_directory::allocate(
                store,
                &private,
                &installation,
                &allocating_workspace,
                &allocating_package,
            )
        })
        .await
        .map_err(|failure| host_error(failure.code, PackageErrorStage::Storage))?
        .map_err(|failure| host_error(failure.code, PackageErrorStage::Storage))?;
    let spec = supervisor::ProcessSpec {
        template: template.clone(),
        workspace_id,
        workspace_name: workspace.name().to_owned(),
        installation: config.installation_public_identity().clone(),
        data_directory: grant.path,
        continuity_id: grant.continuity_id,
        core: core.clone(),
        calls: calls.clone(),
        store: store.clone(),
    };
    Ok((
        key.clone(),
        Arc::new(instance::InstanceActor::new(key, spec)),
    ))
}
