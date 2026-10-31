//! Opening the Host, and the Package Instance lifecycle commands.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use tokio::sync::{Mutex, watch};

use crate::errors::{
    PackageErrorCode, PackageErrorEnvelope, PackageErrorStage, activation_error, host_error,
    instance_error,
};
use crate::host::personal;
use crate::values::{
    HostRevision, PackageHostConfig, PackageHostState, PackageIdentity, PackageInstanceKey,
    PackageSourceKind, PackageTier, PackageTrustKind, WorkspaceId,
};
use crate::{
    DisposePackage, HostData, PackageHost, PackageHostInner, PackageHostOutcome,
    ReconcileWorkspaces, RecoverPackage, SuspendPackage, admission, broker, capability,
    data_directory, host_store, instance, protocol, supervisor, ui,
};

/// A Package Identity and Workspace pair whose orphan lease is unresolved, so no
/// Instance may be activated for it.
type BlockedInstances = BTreeSet<(String, String)>;

/// The admitted Packages of one admission report, divided by tier.
struct AdmittedTiers {
    team_templates: Vec<supervisor::ProcessTemplate>,
    team_ui: BTreeMap<PackageIdentity, Arc<ui::AdmittedUi>>,
    team_admitted: admission::Eligible,
    personal: Vec<personal::PersonalPackage>,
}

/// The immutable Team tier fields of a new Host.
struct Team {
    tier: personal::TeamTier,
    templates: Vec<supervisor::ProcessTemplate>,
    admitted_ui: BTreeMap<PackageIdentity, Arc<ui::AdmittedUi>>,
    authoring_catalog: Vec<crate::AuthoringCatalogEntry>,
    port_graph: BTreeMap<PackageIdentity, Vec<crate::PortAvailability>>,
    port_contracts: BTreeMap<
        PackageIdentity,
        Arc<teamarness_package_contract::port_contract::CompiledPortContracts>,
    >,
}

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
        let port_contracts: BTreeMap<
            PackageIdentity,
            Arc<teamarness_package_contract::port_contract::CompiledPortContracts>,
        > = report
            .admissions
            .iter()
            .filter_map(|admission| {
                admission
                    .manifest
                    .port_contract
                    .as_ref()
                    .map(|contract| (admission.manifest.identity.clone(), contract.clone()))
            })
            .collect();
        debug_assert_eq!(report.graph.len(), report.snapshot.packages.len());
        let consumed = report
            .admissions
            .iter()
            .map(|admission| {
                (
                    admission.manifest.identity.clone(),
                    admission.manifest.consumed_ports.clone(),
                )
            })
            .collect();
        let team_identities: BTreeSet<PackageIdentity> = report
            .tiers
            .iter()
            .filter(|(_, tier)| **tier == PackageTier::Team)
            .map(|(identity, _)| identity.clone())
            .collect();
        let port_broker = broker::PortBroker::new(
            broker::PortSnapshot::new(
                report.graph.clone(),
                consumed,
                port_contracts.clone(),
                report.tiers.clone(),
                report.team_providers.clone(),
                report.personal_providers.clone(),
            ),
            crate::port_access::PortAccess::new(store.clone()),
        );
        let tiers = Self::split_tiers(&config, &mut report, &team_identities)?;
        let templates: Vec<_> = tiers
            .team_templates
            .iter()
            .chain(tiers.personal.iter().map(|package| &package.template))
            .cloned()
            .collect();
        // One permit pool, one set of protocol bounds and one content view
        // table for every Package in this Host.
        let calls = supervisor::CallEnvironment {
            limiter: capability::CoreCallLimiter::new(),
            limits: protocol::ProtocolLimits {
                capability_call: config.capability_call_timeout,
            },
            content_views: Arc::default(),
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
            &port_broker,
        )
        .await?;
        let (team_catalog, personal_catalog): (Vec<_>, Vec<_>) =
            std::mem::take(&mut report.catalog)
                .into_iter()
                .partition(|entry| team_identities.contains(&entry.package.identity));
        let mut team_graph = std::mem::take(&mut report.graph);
        team_graph.retain(|identity, _| team_identities.contains(identity));
        let mut team_contracts = port_contracts;
        team_contracts.retain(|identity, _| team_identities.contains(identity));
        let personal = personal::PersonalTier::new(
            tiers.personal,
            std::mem::take(&mut report.personal_providers),
            personal_catalog,
        );
        let team = personal::TeamTier {
            admitted: tiers.team_admitted,
            providers: std::mem::take(&mut report.team_providers),
        };
        Ok(Self::assemble(
            config,
            core,
            store,
            calls,
            Team {
                tier: team,
                templates: tiers.team_templates,
                admitted_ui: tiers.team_ui,
                authoring_catalog: team_catalog,
                port_graph: team_graph,
                port_contracts: team_contracts,
            },
            personal,
            port_broker,
            report.snapshot.packages,
            report.snapshot.diagnostics,
            report.snapshot.registrations,
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

    pub(crate) async fn list_core_workspaces(
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
    /// from, and divides the admitted Packages between the immutable Team tier
    /// and the live Personal tier.
    fn split_tiers(
        config: &PackageHostConfig,
        report: &mut admission::AdmissionReport,
        team: &BTreeSet<PackageIdentity>,
    ) -> Result<AdmittedTiers, PackageErrorEnvelope> {
        let mut tiers = AdmittedTiers {
            team_templates: Vec::new(),
            team_ui: BTreeMap::new(),
            team_admitted: BTreeMap::new(),
            personal: Vec::new(),
        };
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
                })?
                .clone();
            let identity = admission.manifest.identity.clone();
            let template = process_template(config, &catalog, &admission);
            if team.contains(&identity) {
                tiers.team_templates.push(template);
                tiers.team_ui.insert(identity.clone(), admission.ui.clone());
                tiers.team_admitted.insert(identity, (catalog, admission));
            } else {
                let routes = report.graph.get(&identity).cloned().unwrap_or_default();
                tiers.personal.push(personal::PersonalPackage {
                    admitted: (catalog, admission),
                    template,
                    routes,
                });
            }
        }
        Ok(tiers)
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
        broker: &Arc<broker::PortBroker>,
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
                    prepare_instance(config, core, calls, store, template, workspace, broker, 0)
                        .await?;
                broker.register(key.clone(), &actor);
                instances.insert(key, actor);
            }
        }
        // Every actor and its immutable graph must be addressable before any eager Backend
        // is activated: activation itself may call an already-admitted Provider.
        for template in templates.iter().filter(|template| {
            template.activation == teamarness_package_contract::manifest::ActivationPolicy::Eager
        }) {
            for (key, actor) in &instances {
                if key.package_identity == template.package_identity
                    && let Err(failure) = actor.activate().await
                {
                    diagnostics.push(activation_error(&failure, key));
                }
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
        team: Team,
        personal: personal::PersonalTier,
        port_broker: Arc<broker::PortBroker>,
        packages: Vec<crate::PackageCatalogEntry>,
        diagnostics: Vec<PackageErrorEnvelope>,
        registrations: Vec<crate::InstallationRegistrationSnapshot>,
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
                    registrations,
                    instances,
                    mounts: BTreeMap::new(),
                    ui_routes: BTreeMap::new(),
                }),
                revisions,
                config,
                core,
                team: team.tier,
                templates: team.templates,
                admitted_ui: team.admitted_ui,
                authoring_catalog: team.authoring_catalog,
                port_graph: team.port_graph,
                port_contracts: team.port_contracts,
                personal,
                port_broker,
                reconciliation: Mutex::new(()),
                mount_management: Mutex::new(()),
            }),
        }
    }

    /// Activates every admitted Package for every Workspace Core now lists, and
    /// leaves every existing Instance alone.
    ///
    /// It holds the Personal tier lock, so it sees either all or none of a live
    /// Personal operation and never creates a second Instance for a key.
    pub(crate) async fn reconcile_workspaces(
        &self,
        _input: ReconcileWorkspaces,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        let tier = self.inner.personal.lock().await;
        let _gate = self.inner.reconciliation.lock().await;
        let workspaces = Self::list_core_workspaces(&self.inner.core).await?;
        let templates: Vec<_> = self
            .inner
            .templates
            .iter()
            .cloned()
            .chain(self.inner.personal.templates())
            .collect();
        let mut eager = Vec::new();
        for template in &templates {
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
                    &self.inner.port_broker,
                    tier.last_generation(&key),
                )
                .await?;
                self.inner.port_broker.register(key.clone(), &actor);
                if template.activation
                    == teamarness_package_contract::manifest::ActivationPolicy::Eager
                {
                    eager.push((key.clone(), actor.clone()));
                }
                self.inner.data.lock().await.instances.insert(key, actor);
            }
        }
        for (key, actor) in eager {
            if let Err(failure) = actor.activate().await {
                self.inner
                    .data
                    .lock()
                    .await
                    .diagnostics
                    .push(activation_error(&failure, &key));
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
        // A Personal operation or reconciliation that is creating Instances
        // finishes first, so every Instance it creates is shut down below.
        let _tier = self.inner.personal.lock().await;
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
            .iter()
            .map(|(key, actor)| (key.clone(), actor.clone()))
            .collect::<Vec<_>>();
        let mut blocked = None;
        for (key, actor) in actors {
            if !actor.shutdown().await {
                blocked = Some(key);
            }
        }
        if let Some(key) = blocked {
            return Err(instance_error(
                protocol::WireFailure::new(PackageErrorCode::OutcomeUncertain),
                &key,
            ));
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

/// The spawn template for one admitted Package.
pub(crate) fn process_template(
    config: &PackageHostConfig,
    catalog: &crate::PackageCatalogEntry,
    admission: &admission::Admission,
) -> supervisor::ProcessTemplate {
    let source_label = match catalog.source {
        PackageSourceKind::Bundled => "bundled",
        PackageSourceKind::Local => "local",
    };
    let trust_label = match catalog.trust {
        PackageTrustKind::ApplicationBundled => "application_bundled",
        PackageTrustKind::TrustedNativeCodeV1 => "trusted_native_code_v1",
    };
    supervisor::ProcessTemplate {
        package_identity: admission.manifest.identity.clone(),
        package_version: admission.manifest.version.to_string(),
        manifest_digest: admission.manifest.manifest_digest.clone(),
        backend_digest: admission.manifest.backend.digest.clone(),
        ui_contract_digest: admission.manifest.ui.contract.digest.clone(),
        backend_path: admission.backend_path.clone(),
        private_data_directory: config.private_data_directory().to_owned(),
        source_label: source_label.to_owned(),
        trust_label: trust_label.to_owned(),
        protocol: (
            admission.manifest.protocol.major,
            admission.manifest.protocol.max_minor,
        ),
        activation: admission.manifest.activation,
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
    }
}

#[allow(clippy::result_large_err)]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn prepare_instance(
    config: &PackageHostConfig,
    core: &Arc<teamarness_core::CoreRuntime>,
    calls: &supervisor::CallEnvironment,
    store: &host_store::HostStoreHandle,
    template: &supervisor::ProcessTemplate,
    workspace: &teamarness_core::WorkspaceSummary,
    broker: &Arc<broker::PortBroker>,
    initial_generation: u64,
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
        authority: broker.authority_generation(&key),
        template: template.clone(),
        workspace_id,
        workspace_name: workspace.name().to_owned(),
        installation: config.installation_public_identity().clone(),
        data_directory: grant.path,
        continuity_id: grant.continuity_id,
        core: core.clone(),
        calls: calls.clone(),
        store: store.clone(),
        broker: Arc::downgrade(broker),
    };
    Ok((
        key.clone(),
        Arc::new(instance::InstanceActor::new(key, spec, initial_generation)),
    ))
}
