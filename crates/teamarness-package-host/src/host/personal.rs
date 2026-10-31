//! The live Personal tier: install, reload, and remove one Personal Package
//! while the Host runs (ADR 0036).
//!
//! The Team tier is admitted at open and never changes. The Personal tier is
//! every installation registration without `overrideApplication`. This module
//! owns its lock and its per-identity entries, and holds these invariants:
//!
//! - Only a Personal operation writes the Personal entries, and it holds the
//!   tier lock for the whole operation. Workspace reconciliation and Host
//!   shutdown take the same lock, so neither sees half of an operation, and no
//!   Instance key ever gets a second actor.
//! - An operation never touches a Team Instance, route, generation, or mount.
//!   Team Packages never route to a Personal Provider, and every Port snapshot
//!   the tier publishes copies the Team entries unchanged.
//! - Each install, reload, or remove publishes the broker's next Port snapshot
//!   once, with the target's entries and every re-resolved Personal consumer's
//!   routes in the same swap.
//! - A running Provider is never displaced: a candidate that claims a contract
//!   a running Package provides is refused with `port_provider_conflict`.
//! - An Instance key's process generation never decreases while the Host runs,
//!   even across remove and reinstall.
//! - Only the target Package and the Personal consumers whose selected route
//!   changed are restarted.
//!
//! Lock order: the tier lock, then `mount_management`, then `data`. The entry
//! lock is a short synchronous lock that is never held across an await.

// Every Host operation returns the full, payload-free error envelope, as the Host facade does.
#![allow(clippy::result_large_err)]

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use tokio::sync::{Mutex, MutexGuard};

use crate::admission::{
    self, Admission, AdmissionMode, Eligible, LiveAdmission, LiveVerdict, View,
};
use crate::broker::CallerPorts;
use crate::catalog::{self, InstallationEntry, InstallationRegistration};
use crate::errors::{activation_error, host_error, instance_error};
use crate::host::lifecycle::{prepare_instance, process_template};
use crate::registrations;
use crate::values::{
    InstallationRegistrationSnapshot, PackageCatalogEntry, PackageHostState, PackageIdentity,
    PackageInstanceKey, PackageSourceKind, PackageTier, PendingRegistrationChange,
    RegistrationRequest, RegistrationStatus, WorkspaceId,
};
use crate::{
    AuthoringCatalogEntry, InstallPersonal, PackageErrorCode, PackageErrorEnvelope,
    PackageErrorStage, PackageHost, PackageHostOutcome, PortAvailability, ReloadPersonal,
    RemoveOverride, RemovePersonal, RestartPersonal, WriteOverride, instance, supervisor, ui,
};

/// The Team tier as the Personal tier sees it: what it may consume, never change.
pub(crate) struct TeamTier {
    pub(crate) admitted: Eligible,
    pub(crate) providers: BTreeMap<String, PackageIdentity>,
}

/// One admitted, running Personal Package.
pub(crate) struct PersonalPackage {
    pub(crate) admitted: (PackageCatalogEntry, Admission),
    pub(crate) template: supervisor::ProcessTemplate,
    /// Its consumed-Port routes, as the last published Port snapshot has them.
    pub(crate) routes: Vec<PortAvailability>,
}

#[derive(Default)]
struct Entries {
    packages: BTreeMap<PackageIdentity, PersonalPackage>,
    /// Contract identity to the Personal Package that provides it.
    providers: BTreeMap<String, PackageIdentity>,
    /// Authoring catalog entries, including candidates a Port graph refused.
    catalog: BTreeMap<PackageIdentity, AuthoringCatalogEntry>,
}

/// What the tier lock guards besides the operation itself.
#[derive(Default)]
pub(crate) struct TierState {
    /// The last process generation of every Personal Instance key this Host disposed.
    generations: BTreeMap<PackageInstanceKey, u64>,
}

impl TierState {
    /// The generation a new actor for `key` starts from.
    pub(crate) fn last_generation(&self, key: &PackageInstanceKey) -> u64 {
        self.generations.get(key).copied().unwrap_or(0)
    }
}

/// The Personal tier lock and the mutable per-identity Personal entries.
pub(crate) struct PersonalTier {
    lock: Mutex<TierState>,
    entries: RwLock<Entries>,
    /// Holds the next new Instance between its preparation and its insertion.
    #[cfg(test)]
    instance_pause: std::sync::Mutex<Option<Arc<tests::Pause>>>,
}

impl PersonalTier {
    pub(crate) fn new(
        packages: Vec<PersonalPackage>,
        providers: BTreeMap<String, PackageIdentity>,
        catalog: Vec<AuthoringCatalogEntry>,
    ) -> Self {
        Self {
            lock: Mutex::new(TierState::default()),
            entries: RwLock::new(Entries {
                packages: packages
                    .into_iter()
                    .map(|package| (package.admitted.0.identity.clone(), package))
                    .collect(),
                providers,
                catalog: catalog
                    .into_iter()
                    .map(|entry| (entry.package.identity.clone(), entry))
                    .collect(),
            }),
            #[cfg(test)]
            instance_pause: std::sync::Mutex::new(None),
        }
    }

    /// Takes the tier lock.
    pub(crate) async fn lock(&self) -> MutexGuard<'_, TierState> {
        self.lock.lock().await
    }

    /// The spawn templates of every running Personal Package.
    pub(crate) fn templates(&self) -> Vec<supervisor::ProcessTemplate> {
        self.read()
            .packages
            .values()
            .map(|package| package.template.clone())
            .collect()
    }

    pub(crate) fn admitted_ui(&self, identity: &PackageIdentity) -> Option<Arc<ui::AdmittedUi>> {
        self.read()
            .packages
            .get(identity)
            .map(|package| package.admitted.1.ui.clone())
    }

    pub(crate) fn catalog(&self) -> Vec<AuthoringCatalogEntry> {
        self.read().catalog.values().cloned().collect()
    }

    #[cfg(test)]
    async fn pause_instance(&self) {
        let pause = self.instance_pause.lock().expect("pause poisoned").take();
        if let Some(pause) = pause {
            pause.reached.notify_one();
            pause.release.notified().await;
        }
    }

    fn read(&self) -> RwLockReadGuard<'_, Entries> {
        self.entries.read().expect("personal tier poisoned")
    }

    fn write(&self) -> RwLockWriteGuard<'_, Entries> {
        self.entries.write().expect("personal tier poisoned")
    }
}

type TierGuard<'a> = MutexGuard<'a, TierState>;

#[allow(clippy::result_large_err)]
impl PackageHost {
    /// The admitted UI for `identity`: Team first, then Personal.
    pub(crate) fn admitted_ui(&self, identity: &PackageIdentity) -> Option<Arc<ui::AdmittedUi>> {
        self.inner
            .admitted_ui
            .get(identity)
            .cloned()
            .or_else(|| self.inner.personal.admitted_ui(identity))
    }

    /// Registers and admits a Personal Package, then starts it in every Workspace.
    ///
    /// Nothing is written unless the registration file would admit the
    /// candidate at the next open and the running Host admits it now. If the
    /// admission still fails after the file was written, the file is restored.
    pub(crate) async fn install_personal(
        &self,
        input: InstallPersonal,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        let mut tier = self.inner.personal.lock().await;
        self.ensure_running().await?;
        let identity = input.expected_identity.clone();
        if self.inner.team.admitted.contains_key(&identity) {
            return Err(refusal(PackageErrorCode::OverrideRequired, &identity));
        }
        let request = RegistrationRequest {
            directory: input.directory,
            expected_identity: identity.clone(),
            accepted_risks: input.accepted_risks,
            override_application: false,
        };
        let config = self.inner.config.clone();
        let checking = identity.clone();
        let (previous, proposed) = self
            .inner
            .store
            .run(move |store| {
                let previous = registrations::read_entries(&config)?;
                if previous.iter().any(|entry| {
                    entry.expected_identity == checking.as_str() && entry.override_application
                }) {
                    return Err(refusal(PackageErrorCode::OverrideRequired, &checking));
                }
                let proposed = registrations::propose(&config, &request, previous.clone())?;
                registrations::preflight(&config, store, &proposed, &checking)?;
                Ok((previous, proposed))
            })
            .await
            .map_err(|failure| host_error(failure.code, PackageErrorStage::Storage))??;
        let entry = proposed
            .iter()
            .find(|entry| entry.expected_identity == identity.as_str())
            .cloned()
            .ok_or_else(|| refusal(PackageErrorCode::InvalidRegistration, &identity))?;
        let registration = registration_of(&entry);
        let checked = self
            .admit_live(&identity, registration.clone(), AdmissionMode::Preflight)
            .await?;
        if let Err(code) = checked.result {
            return Err(refusal(code, &identity));
        }

        let replaced = self.retire(&identity, &mut tier).await;
        if let Err(error) = self.write_registrations(proposed).await {
            return Err(self.fail_install(&identity, replaced, None, error).await);
        }
        let verdict = match self
            .admit_live(&identity, registration, AdmissionMode::Commit)
            .await
        {
            Ok(verdict) => verdict,
            Err(error) => {
                return Err(self
                    .fail_install(&identity, replaced, Some(previous), error)
                    .await);
            }
        };
        let changed = match verdict.result {
            Ok(admitted) => {
                let changed = self.enter(admitted, verdict.catalog, &mut tier).await;
                self.upsert_registration(snapshot_of(&entry, RegistrationStatus::Admitted))
                    .await;
                changed
            }
            Err(code) => {
                if replaced {
                    self.record(verdict.diagnostics).await;
                }
                return Err(self
                    .fail_install(
                        &identity,
                        replaced,
                        Some(previous),
                        refusal(code, &identity),
                    )
                    .await);
            }
        };
        self.restart_consumers(changed).await;
        Ok(PackageHostOutcome::PersonalInstalled {
            revision: self.bump_revision().await,
        })
    }

    /// Re-admits a registered Personal Package from its directory.
    ///
    /// Its Instances are disposed first, then recreated with process generations
    /// that continue from the old ones. If re-admission fails, the registration
    /// stays and reports `refused` with the code.
    pub(crate) async fn reload_personal(
        &self,
        input: ReloadPersonal,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        let mut tier = self.inner.personal.lock().await;
        self.ensure_running().await?;
        let identity = input.identity;
        let entry = self.personal_entry(&identity).await?;
        self.retire(&identity, &mut tier).await;
        let verdict = match self
            .admit_live(&identity, registration_of(&entry), AdmissionMode::Commit)
            .await
        {
            Ok(verdict) => verdict,
            Err(error) => return Err(self.refuse_registration(&identity, error).await),
        };
        let LiveVerdict {
            result,
            catalog,
            diagnostics,
        } = verdict;
        let changed = match result {
            Ok(admitted) => {
                let changed = self.enter(admitted, catalog, &mut tier).await;
                self.upsert_registration(snapshot_of(&entry, RegistrationStatus::Admitted))
                    .await;
                changed
            }
            Err(code) => {
                if let Some(catalog) = catalog {
                    self.inner
                        .personal
                        .write()
                        .catalog
                        .insert(identity.clone(), catalog);
                }
                self.record(diagnostics).await;
                return Err(self
                    .refuse_registration(&identity, refusal(code, &identity))
                    .await);
            }
        };
        self.restart_consumers(changed).await;
        Ok(PackageHostOutcome::PersonalReloaded {
            revision: self.bump_revision().await,
        })
    }

    /// Stops a Personal Package in every Workspace and removes its registration.
    /// Its Package Data Directories are kept.
    pub(crate) async fn remove_personal(
        &self,
        input: RemovePersonal,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        let mut tier = self.inner.personal.lock().await;
        self.ensure_running().await?;
        let identity = input.identity;
        self.personal_entry(&identity).await?;
        self.retire(&identity, &mut tier).await;
        let config = self.inner.config.clone();
        let removing = identity.clone();
        let removed = tokio::task::spawn_blocking(move || {
            registrations::remove_registration(&config, &removing)
        })
        .await
        .map_err(|_| unavailable())
        .and_then(|result| result);
        if let Err(error) = removed {
            return Err(self.refuse_registration(&identity, error).await);
        }
        self.inner
            .data
            .lock()
            .await
            .registrations
            .retain(|registration| registration.expected_identity != identity.as_str());
        self.reresolve_consumers().await;
        Ok(PackageHostOutcome::PersonalRemoved {
            revision: self.bump_revision().await,
        })
    }

    /// Replaces the process of one Personal Instance with a new generation, over
    /// its current Port routes. Its data and its registration are unchanged.
    ///
    /// A Team Instance is refused with `override_required`, as Reload and Remove
    /// refuse a Team identity: a Team Package changes only through an override.
    /// An Instance that is not active or failed (never started, paused, or ended)
    /// is refused with `invalid_lifecycle_state`, so a refusal never looks like a
    /// restart that happened.
    pub(crate) async fn restart_personal(
        &self,
        input: RestartPersonal,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        let _tier = self.inner.personal.lock().await;
        self.ensure_running().await?;
        let key = input.instance;
        let identity = &key.package_identity;
        if self.inner.team.admitted.contains_key(identity) {
            return Err(refusal(PackageErrorCode::OverrideRequired, identity));
        }
        if !self.inner.personal.read().packages.contains_key(identity) {
            return Err(refusal(PackageErrorCode::NotRegistered, identity));
        }
        let actor = self.actor(&key).await?;
        let restarted = actor
            .restart()
            .await
            .map_err(|failure| instance_error(failure, &key))?;
        if restarted == instance::RestartOutcome::NotRunning {
            return Err(instance_error(
                crate::protocol::WireFailure::new(PackageErrorCode::InvalidLifecycleState),
                &key,
            ));
        }
        Ok(PackageHostOutcome::PersonalRestarted {
            revision: self.bump_revision().await,
        })
    }

    /// Writes an override registration for a bundled identity. No running
    /// Instance changes; the override is admitted in the Team tier at the next
    /// open. The snapshot shows the entry as pending a restart.
    ///
    /// The write holds the tier lock, so it never interleaves with a live
    /// Personal write to the same file.
    pub(crate) async fn write_override(
        &self,
        input: WriteOverride,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        let _tier = self.inner.personal.lock().await;
        self.ensure_running().await?;
        let identity = input.expected_identity.clone();
        let request = RegistrationRequest {
            directory: input.directory,
            expected_identity: identity.clone(),
            accepted_risks: input.accepted_risks,
            override_application: true,
        };
        let config = self.inner.config.clone();
        let proposed = self
            .inner
            .store
            .run(move |store| {
                if !registrations::application_identities(&config).contains(&identity) {
                    // Only a bundled Package can be overridden; anything else is Personal.
                    return Err(refusal(PackageErrorCode::InvalidRegistration, &identity));
                }
                let entries = registrations::read_entries(&config)?;
                let proposed = registrations::propose(&config, &request, entries)?;
                registrations::preflight(&config, store, &proposed, &identity)?;
                Ok(proposed)
            })
            .await
            .map_err(|failure| host_error(failure.code, PackageErrorStage::Storage))??;
        let entry = proposed
            .iter()
            .find(|entry| entry.expected_identity == input.expected_identity.as_str())
            .cloned()
            .ok_or_else(|| {
                refusal(
                    PackageErrorCode::InvalidRegistration,
                    &input.expected_identity,
                )
            })?;
        self.write_registrations(proposed).await?;
        self.upsert_registration(snapshot_of(
            &entry,
            RegistrationStatus::PendingRestart {
                change: PendingRegistrationChange::Written,
            },
        ))
        .await;
        self.bump_revision().await;
        Ok(PackageHostOutcome::OverrideWritten)
    }

    /// Removes the file entry for a Team Package identity: an override, or an
    /// entry written by hand without one. No running Instance changes; the
    /// bundled Package returns at the next open. Data is kept.
    ///
    /// The snapshot shows the entry as pending a restart, or drops it when the
    /// bundled Package already runs, because then nothing is left to apply.
    pub(crate) async fn remove_override(
        &self,
        input: RemoveOverride,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        let _tier = self.inner.personal.lock().await;
        self.ensure_running().await?;
        let identity = input.identity;
        let config = self.inner.config.clone();
        let removing = identity.clone();
        tokio::task::spawn_blocking(move || {
            let mut entries = registrations::read_entries(&config)?;
            let team = registrations::application_identities(&config).contains(&removing);
            if !entries.iter().any(|entry| {
                entry.expected_identity == removing.as_str() && (entry.override_application || team)
            }) {
                return Err(refusal(PackageErrorCode::NotRegistered, &removing));
            }
            entries.retain(|entry| entry.expected_identity != removing.as_str());
            registrations::write_entries(&config, &entries)
        })
        .await
        .map_err(|_| unavailable())??;
        {
            let mut data = self.inner.data.lock().await;
            let bundled_runs = data.packages.iter().any(|package| {
                package.identity == identity && package.source == PackageSourceKind::Bundled
            });
            if bundled_runs {
                data.registrations
                    .retain(|registration| registration.expected_identity != identity.as_str());
            } else {
                for registration in &mut data.registrations {
                    if registration.expected_identity == identity.as_str() {
                        registration.status = RegistrationStatus::PendingRestart {
                            change: PendingRegistrationChange::Removed,
                        };
                    }
                }
            }
        }
        self.bump_revision().await;
        Ok(PackageHostOutcome::OverrideRemoved)
    }

    /// Refuses a Personal operation that started while the Host was stopping.
    async fn ensure_running(&self) -> Result<(), PackageErrorEnvelope> {
        if self.inner.data.lock().await.state == PackageHostState::Running {
            Ok(())
        } else {
            Err(host_error(
                PackageErrorCode::ShuttingDown,
                PackageErrorStage::Shutdown,
            ))
        }
    }

    /// The registration file entry for a Personal identity.
    ///
    /// An entry for a Team Package identity is refused with `override_required`,
    /// also one written by hand without `overrideApplication`: it changes only
    /// through [`RemoveOverride`] or an override, applied on restart.
    async fn personal_entry(
        &self,
        identity: &PackageIdentity,
    ) -> Result<InstallationEntry, PackageErrorEnvelope> {
        let config = self.inner.config.clone();
        let checking = identity.clone();
        let (entries, team) = tokio::task::spawn_blocking(move || {
            let team = registrations::application_identities(&config).contains(&checking);
            registrations::read_entries(&config).map(|entries| (entries, team))
        })
        .await
        .map_err(|_| unavailable())??;
        let entry = entries
            .into_iter()
            .find(|entry| entry.expected_identity == identity.as_str())
            .ok_or_else(|| refusal(PackageErrorCode::NotRegistered, identity))?;
        if entry.override_application || team || self.inner.team.admitted.contains_key(identity) {
            return Err(refusal(PackageErrorCode::OverrideRequired, identity));
        }
        Ok(entry)
    }

    /// Admits one registration against the Team tier and every other running
    /// Personal Package.
    async fn admit_live(
        &self,
        identity: &PackageIdentity,
        registration: InstallationRegistration,
        mode: AdmissionMode,
    ) -> Result<LiveVerdict, PackageErrorEnvelope> {
        let (running, providers) = {
            let entries = self.inner.personal.read();
            let mut running = self.inner.team.admitted.clone();
            running.extend(
                entries
                    .packages
                    .iter()
                    .filter(|(other, _)| *other != identity)
                    .map(|(other, package)| (other.clone(), package.admitted.clone())),
            );
            let mut providers = self.inner.team.providers.clone();
            providers.extend(
                entries
                    .providers
                    .iter()
                    .filter(|(_, provider)| *provider != identity)
                    .map(|(contract, provider)| (contract.clone(), provider.clone())),
            );
            (running, providers)
        };
        let config = self.inner.config.clone();
        self.inner
            .store
            .run(move |store| {
                admission::admit_live_personal(
                    &config,
                    store,
                    &running,
                    &providers,
                    &registration,
                    mode,
                )
            })
            .await
            .map_err(|failure| host_error(failure.code, PackageErrorStage::Storage))?
            .map_err(|failure| host_error(failure.code, PackageErrorStage::Admission))
    }

    /// Disposes every Instance of `identity`, which revokes its mounts, then
    /// removes its entries. Returns whether it was running.
    ///
    /// The published Port snapshot keeps its entries until the operation
    /// publishes the next one; its Instances are already unreachable, so a new
    /// call to it is refused with `port_unavailable` meanwhile.
    async fn retire(&self, identity: &PackageIdentity, tier: &mut TierGuard<'_>) -> bool {
        let actors = self
            .inner
            .data
            .lock()
            .await
            .instances
            .iter()
            .filter(|(key, _)| key.package_identity == *identity)
            .map(|(key, actor)| (key.clone(), actor.clone()))
            .collect::<Vec<_>>();
        let mut failures = Vec::new();
        for (key, actor) in actors {
            // New routes stop here; a call that already resolved this actor keeps it.
            self.inner.port_broker.unregister(&key);
            let _gate = self.inner.mount_management.lock().await;
            let disposed = actor.dispose().await;
            self.revoke_mounts_for_instance(&key).await;
            let generation = actor.generation().await;
            let last = tier.generations.entry(key.clone()).or_insert(0);
            *last = (*last).max(generation);
            self.inner.data.lock().await.instances.remove(&key);
            if let Err(failure) = disposed {
                failures.push(instance_error(failure, &key));
            }
        }
        let running = {
            let mut entries = self.inner.personal.write();
            entries.providers.retain(|_, provider| provider != identity);
            entries.catalog.remove(identity);
            entries.packages.remove(identity).is_some()
        };
        let mut data = self.inner.data.lock().await;
        data.packages
            .retain(|package| package.identity != *identity);
        // Its earlier admission and lifecycle diagnostics no longer apply. An
        // unresolved orphan lease from the previous run still does.
        data.diagnostics.retain(|error| {
            error.affected_package.as_ref() != Some(identity)
                || error.code == PackageErrorCode::OrphanUnresolved
        });
        data.diagnostics.extend(failures);
        running
    }

    /// Inserts one admitted Package's entries, publishes the next Port snapshot
    /// with them, then creates and activates its Instances in every Workspace.
    /// Returns the Personal consumers whose routes changed, for a restart.
    async fn enter(
        &self,
        admitted: LiveAdmission,
        catalog: Option<AuthoringCatalogEntry>,
        tier: &mut TierGuard<'_>,
    ) -> Vec<PackageIdentity> {
        let LiveAdmission {
            entry,
            admission,
            graph,
            providers,
        } = admitted;
        let identity = entry.identity.clone();
        let template = process_template(&self.inner.config, &entry, &admission);
        {
            let mut entries = self.inner.personal.write();
            entries.providers.extend(providers);
            if let Some(catalog) = catalog {
                entries.catalog.insert(identity.clone(), catalog);
            }
            entries.packages.insert(
                identity.clone(),
                PersonalPackage {
                    admitted: (entry.clone(), admission),
                    template: template.clone(),
                    routes: graph,
                },
            );
        }
        {
            let mut data = self.inner.data.lock().await;
            data.packages.retain(|package| package.identity != identity);
            data.packages.push(entry);
            data.packages
                .sort_by(|left, right| left.identity.cmp(&right.identity));
        }
        // Publish before the new Instances exist, so their first calls see their routes.
        let changed = self.publish_personal(Some(&identity)).await;
        self.create_instances(&template, tier).await;
        changed
    }

    /// Creates one Instance of `template` per Workspace, then activates the
    /// eager ones once every new actor is addressable.
    async fn create_instances(&self, template: &supervisor::ProcessTemplate, tier: &TierGuard<'_>) {
        let identity = &template.package_identity;
        let workspaces = match Self::list_core_workspaces(&self.inner.core).await {
            Ok(workspaces) => workspaces,
            Err(mut error) => {
                error.affected_package = Some(identity.clone());
                self.inner.data.lock().await.diagnostics.push(error);
                return;
            }
        };
        let mut created: Vec<(PackageInstanceKey, Arc<instance::InstanceActor>)> = Vec::new();
        for workspace in &workspaces {
            let Ok(workspace_id) = WorkspaceId::new(workspace.id().to_string()) else {
                continue;
            };
            let key = PackageInstanceKey::new(identity.clone(), workspace_id);
            // Under the tier lock nothing else creates a Personal Instance.
            if self.inner.data.lock().await.instances.contains_key(&key) {
                debug_assert!(false, "a retired Personal Package has no Instance");
                continue;
            }
            match prepare_instance(
                &self.inner.config,
                &self.inner.core,
                &self.inner.calls,
                &self.inner.store,
                template,
                workspace,
                &self.inner.port_broker,
                tier.last_generation(&key),
            )
            .await
            {
                Ok((key, actor)) => {
                    #[cfg(test)]
                    self.inner.personal.pause_instance().await;
                    self.inner.port_broker.register(key.clone(), &actor);
                    self.inner
                        .data
                        .lock()
                        .await
                        .instances
                        .insert(key.clone(), actor.clone());
                    created.push((key, actor));
                }
                Err(mut error) => {
                    error.affected_package = Some(identity.clone());
                    self.inner.data.lock().await.diagnostics.push(error);
                }
            }
        }
        if template.activation != teamarness_package_contract::manifest::ActivationPolicy::Eager {
            return;
        }
        for (key, actor) in created {
            if let Err(failure) = actor.activate().await {
                self.inner
                    .data
                    .lock()
                    .await
                    .diagnostics
                    .push(activation_error(&failure, &key));
            }
        }
    }

    /// Publishes the next Port snapshot after a remove or a refused
    /// registration, then restarts the Personal consumers whose routes changed.
    async fn reresolve_consumers(&self) {
        let changed = self.publish_personal(None).await;
        self.restart_consumers(changed).await;
    }

    /// Re-resolves every running Personal consumer except `except` against the
    /// current Providers, then publishes the next Port snapshot in one swap: the
    /// Team entries unchanged, and every Personal entry and Provider as the tier
    /// now has them. A consumer whose selected routes changed gets a diagnostic
    /// for each required Port that is now unavailable. Returns those consumers.
    ///
    /// The caller holds the tier lock, which serializes every publish.
    async fn publish_personal(&self, except: Option<&PackageIdentity>) -> Vec<PackageIdentity> {
        let changed: Vec<(PackageIdentity, Vec<PackageErrorCode>)> = {
            let mut entries = self.inner.personal.write();
            let healthy;
            let resolved: Vec<_> = {
                let mut running: View<'_> = self
                    .inner
                    .team
                    .admitted
                    .iter()
                    .map(|(identity, entry)| (identity.clone(), entry))
                    .collect();
                running.extend(
                    entries
                        .packages
                        .iter()
                        .map(|(identity, package)| (identity.clone(), &package.admitted)),
                );
                let mut providers = self.inner.team.providers.clone();
                providers.extend(entries.providers.clone());
                // The same required-Provider health filter as live admission (CR-002):
                // a Provider that lost a required Port serves no consumer, old or new.
                healthy = admission::healthy(&running, &providers);
                entries
                    .packages
                    .iter()
                    .filter(|(identity, _)| Some(*identity) != except)
                    .filter_map(|(identity, package)| {
                        let consumed = &package.admitted.1.manifest.consumed_ports;
                        let routes =
                            admission::resolve_routes(consumed, &providers, &running, &healthy);
                        (routes != package.routes).then(|| {
                            let missing = consumed
                                .iter()
                                .zip(&routes)
                                .filter(|(request, _)| request.required)
                                .filter_map(|(_, route)| route.unavailable)
                                .collect();
                            (identity.clone(), routes, missing)
                        })
                    })
                    .collect()
            };
            let mut changed = Vec::with_capacity(resolved.len());
            for (identity, routes, missing) in resolved {
                if let Some(catalog) = entries.catalog.get_mut(&identity) {
                    catalog.availability = routes.clone();
                }
                if let Some(package) = entries.packages.get_mut(&identity) {
                    package.routes = routes;
                }
                changed.push((identity, missing));
            }
            let personal = entries
                .packages
                .iter()
                .map(|(identity, package)| {
                    let manifest = &package.admitted.1.manifest;
                    (
                        identity.clone(),
                        CallerPorts {
                            routes: package.routes.clone(),
                            consumed: manifest.consumed_ports.clone(),
                            contracts: manifest.port_contract.clone(),
                        },
                    )
                })
                .collect();
            let broker = &self.inner.port_broker;
            let offered = entries
                .providers
                .iter()
                .filter(|(_, provider)| healthy.contains(*provider))
                .map(|(contract, provider)| (contract.clone(), provider.clone()))
                .collect();
            broker.publish(broker.snapshot().with_personal(personal, offered));
            changed
        };
        let mut data = self.inner.data.lock().await;
        for (identity, missing) in &changed {
            data.diagnostics.retain(|error| {
                error.affected_package.as_ref() != Some(identity) || !route_code(error.code)
            });
            for code in missing {
                let error = admission::diagnostic(
                    *code,
                    PackageErrorStage::Admission,
                    Some(identity.clone()),
                );
                if !data
                    .diagnostics
                    .iter()
                    .any(|existing| existing.diagnostic_id == error.diagnostic_id)
                {
                    data.diagnostics.push(error);
                }
            }
        }
        changed.into_iter().map(|(identity, _)| identity).collect()
    }

    /// Restarts every Instance of each changed consumer over its new routes.
    /// No other Instance is touched.
    async fn restart_consumers(&self, changed: Vec<PackageIdentity>) {
        for identity in changed {
            let actors = self
                .inner
                .data
                .lock()
                .await
                .instances
                .iter()
                .filter(|(key, _)| key.package_identity == identity)
                .map(|(key, actor)| (key.clone(), actor.clone()))
                .collect::<Vec<_>>();
            for (key, actor) in actors {
                match actor.restart().await {
                    // A consumer that is not running reads its new routes when it next starts.
                    Ok(
                        instance::RestartOutcome::Restarted | instance::RestartOutcome::NotRunning,
                    ) => {}
                    Err(failure) => self
                        .inner
                        .data
                        .lock()
                        .await
                        .diagnostics
                        .push(activation_error(&failure, &key)),
                }
            }
        }
    }

    /// Ends an install that failed after a running registration was retired or
    /// the file was written. The previous file is restored when one is given.
    async fn fail_install(
        &self,
        identity: &PackageIdentity,
        replaced: bool,
        previous: Option<Vec<InstallationEntry>>,
        error: PackageErrorEnvelope,
    ) -> PackageErrorEnvelope {
        if let Some(previous) = previous {
            // The file is restored best-effort; a failure to restore it is the
            // same storage fault that the next open reports.
            let _ = self.write_registrations(previous).await;
        }
        if replaced {
            // The previous registration is still in the file but no longer runs.
            return self.refuse_registration(identity, error).await;
        }
        error
    }

    /// Marks a registration `refused` after its Package stopped running, then
    /// re-resolves the consumers it served and publishes the change.
    async fn refuse_registration(
        &self,
        identity: &PackageIdentity,
        error: PackageErrorEnvelope,
    ) -> PackageErrorEnvelope {
        {
            let mut data = self.inner.data.lock().await;
            for registration in &mut data.registrations {
                if registration.expected_identity == identity.as_str() {
                    registration.status = RegistrationStatus::Refused { code: error.code };
                }
            }
            if !data
                .diagnostics
                .iter()
                .any(|existing| existing.affected_package.as_ref() == Some(identity))
            {
                data.diagnostics.push(error.clone());
            }
        }
        self.reresolve_consumers().await;
        self.bump_revision().await;
        error
    }

    /// Adds admission diagnostics, once each.
    async fn record(&self, diagnostics: Vec<PackageErrorEnvelope>) {
        let mut data = self.inner.data.lock().await;
        for error in diagnostics {
            if !data
                .diagnostics
                .iter()
                .any(|existing| existing.diagnostic_id == error.diagnostic_id)
            {
                data.diagnostics.push(error);
            }
        }
    }

    /// Replaces the snapshot entry for one identity as the store replaces its file entry.
    async fn upsert_registration(&self, snapshot: InstallationRegistrationSnapshot) {
        let mut data = self.inner.data.lock().await;
        let mut placed = false;
        data.registrations.retain_mut(|existing| {
            if existing.expected_identity != snapshot.expected_identity {
                return true;
            }
            if placed {
                return false;
            }
            placed = true;
            *existing = snapshot.clone();
            true
        });
        if !placed {
            data.registrations.push(snapshot);
        }
    }

    async fn write_registrations(
        &self,
        entries: Vec<InstallationEntry>,
    ) -> Result<(), PackageErrorEnvelope> {
        let config = self.inner.config.clone();
        tokio::task::spawn_blocking(move || registrations::write_entries(&config, &entries))
            .await
            .map_err(|_| unavailable())?
    }
}

fn registration_of(entry: &InstallationEntry) -> InstallationRegistration {
    catalog::registrations_from_entries(vec![entry.clone()])
        .pop()
        .expect("one entry yields one registration")
}

fn snapshot_of(
    entry: &InstallationEntry,
    status: RegistrationStatus,
) -> InstallationRegistrationSnapshot {
    let registration = registration_of(entry);
    InstallationRegistrationSnapshot {
        directory: registration.directory.to_string_lossy().into_owned(),
        expected_identity: entry.expected_identity.clone(),
        override_application: entry.override_application,
        tier: if entry.override_application {
            PackageTier::Team
        } else {
            PackageTier::Personal
        },
        status,
    }
}

/// The codes a consumed-Port route reports when it has no usable Provider.
fn route_code(code: PackageErrorCode) -> bool {
    matches!(
        code,
        PackageErrorCode::PortContractMissing
            | PackageErrorCode::PortMissing
            | PackageErrorCode::PortVersionMismatch
    )
}

fn refusal(code: PackageErrorCode, identity: &PackageIdentity) -> PackageErrorEnvelope {
    registrations::refusal(code, PackageErrorStage::Admission, Some(identity.clone()))
}

fn unavailable() -> PackageErrorEnvelope {
    host_error(
        PackageErrorCode::DataDirectoryUnavailable,
        PackageErrorStage::Storage,
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{PackageHostCommand, ReconcileWorkspaces, ShutdownHost, host_fixture};

    #[derive(Default)]
    pub(crate) struct Pause {
        pub(crate) reached: tokio::sync::Notify,
        pub(crate) release: tokio::sync::Notify,
    }

    /// Verifies: TEAM.SYS.PKG-R09
    #[tokio::test]
    async fn reconciliation_waits_for_an_install_that_is_creating_instances() {
        let fixture = host_fixture::Fixture::node(&[("team", "com.example.team")]).await;
        let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
            .await
            .unwrap();
        let path = fixture._root.path().join("sources/racer");
        host_fixture::write_node_package(&path, "com.example.racer");
        fixture.core.create_workspace("Second Workspace").unwrap();
        let pause = Arc::new(Pause::default());
        *host.inner.personal.instance_pause.lock().unwrap() = Some(pause.clone());

        let installing = tokio::spawn({
            let host = host.clone();
            async move {
                host.execute(PackageHostCommand::InstallPersonal(InstallPersonal {
                    directory: path,
                    expected_identity: "com.example.racer".parse().unwrap(),
                    accepted_risks: vec!["filesystem".into(), "network".into(), "process".into()],
                }))
                .await
            }
        });
        // The racer's template is visible and its first Instance is prepared
        // but not yet inserted: the window a second creator would use.
        pause.reached.notified().await;
        let mut reconciling = tokio::spawn({
            let host = host.clone();
            async move {
                host.execute(PackageHostCommand::ReconcileWorkspaces(
                    ReconcileWorkspaces {},
                ))
                .await
            }
        });
        // negative-wait: proves reconciliation does NOT run while the install
        // holds the tier lock; there is no event for a task that is waiting.
        let early = tokio::time::timeout(Duration::from_millis(300), &mut reconciling).await;
        assert!(early.is_err(), "reconciliation waited for the tier lock");
        pause.release.notify_one();
        installing.await.unwrap().unwrap();
        reconciling.await.unwrap().unwrap();

        let data = host.inner.data.lock().await;
        for identity in ["com.example.team", "com.example.racer"] {
            let keys: Vec<_> = data
                .instances
                .keys()
                .filter(|key| key.package_identity.as_str() == identity)
                .collect();
            assert_eq!(keys.len(), 2, "{identity} has one Instance per Workspace");
        }
        for (key, actor) in &data.instances {
            assert_eq!(
                actor.generation().await,
                1,
                "{key:?} started exactly once and was never replaced"
            );
        }
        drop(data);
        host.execute(PackageHostCommand::Shutdown(ShutdownHost))
            .await
            .unwrap();
    }

    /// The restart result seam (SM-011, SF-102): the actor itself says whether it started a
    /// new generation, decided under its lifecycle gate. A result that reported success for an
    /// Instance it left alone would let `RestartPersonal` claim a restart that never happened.
    ///
    /// Verifies: TEAM.SYS.PKG-R09
    #[tokio::test]
    async fn actor_restart_reports_not_running_for_a_suspended_instance() {
        let fixture = host_fixture::Fixture::node(&[("team", "com.example.team")]).await;
        let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
            .await
            .unwrap();
        let key = host.snapshot().await.instances[0].key.clone();
        let actor = host.actor(&key).await.unwrap();
        assert_eq!(actor.generation().await, 1);

        assert_eq!(
            actor.restart().await.unwrap(),
            instance::RestartOutcome::Restarted
        );
        assert_eq!(actor.generation().await, 2, "a running Instance restarts");

        host.execute(PackageHostCommand::Suspend(crate::SuspendPackage {
            instance: key.clone(),
        }))
        .await
        .unwrap();
        assert_eq!(
            actor.restart().await.unwrap(),
            instance::RestartOutcome::NotRunning,
            "a suspended Instance is left alone and says so"
        );
        assert_eq!(actor.generation().await, 2);
        assert_eq!(
            actor.snapshot().await.lifecycle,
            crate::LifecycleState::Suspended
        );
        host.execute(PackageHostCommand::Shutdown(ShutdownHost))
            .await
            .unwrap();
    }

    /// Verifies: TEAM.SYS.PKG-R09
    #[tokio::test]
    async fn an_override_write_waits_for_a_live_install_and_keeps_both_entries() {
        let fixture = host_fixture::Fixture::node(&[("team", "com.example.team")]).await;
        let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
            .await
            .unwrap();
        let racer = fixture._root.path().join("sources/racer");
        host_fixture::write_node_package(&racer, "com.example.racer");
        let build = fixture._root.path().join("sources/team-build");
        host_fixture::write_node_package(&build, "com.example.team");
        let risks = || vec!["filesystem".into(), "network".into(), "process".into()];
        let pause = Arc::new(Pause::default());
        *host.inner.personal.instance_pause.lock().unwrap() = Some(pause.clone());

        let installing = tokio::spawn({
            let host = host.clone();
            async move {
                host.execute(PackageHostCommand::InstallPersonal(InstallPersonal {
                    directory: racer,
                    expected_identity: "com.example.racer".parse().unwrap(),
                    accepted_risks: risks(),
                }))
                .await
            }
        });
        // The install has written its entry and holds the tier lock.
        pause.reached.notified().await;
        let mut overriding = tokio::spawn({
            let host = host.clone();
            async move {
                host.execute(PackageHostCommand::WriteOverride(WriteOverride {
                    directory: build,
                    expected_identity: "com.example.team".parse().unwrap(),
                    accepted_risks: risks(),
                }))
                .await
            }
        });
        // negative-wait: proves the override write does NOT run while the
        // install holds the tier lock; there is no event for a waiting task.
        let early = tokio::time::timeout(Duration::from_millis(300), &mut overriding).await;
        assert!(
            early.is_err(),
            "the override write waited for the tier lock"
        );
        pause.release.notify_one();
        installing.await.unwrap().unwrap();
        assert_eq!(
            overriding.await.unwrap().unwrap(),
            PackageHostOutcome::OverrideWritten
        );

        let entries = registrations::read_entries(&fixture.config).unwrap();
        let mut identities: Vec<_> = entries
            .iter()
            .map(|entry| (entry.expected_identity.as_str(), entry.override_application))
            .collect();
        identities.sort_unstable();
        assert_eq!(
            identities,
            vec![("com.example.racer", false), ("com.example.team", true)],
            "neither write lost the other's entry"
        );
        // The override is file-only: the Team Package is still the bundled one.
        let snapshot = host.snapshot().await;
        let team = snapshot
            .packages
            .iter()
            .find(|package| package.identity.as_str() == "com.example.team")
            .unwrap();
        assert_eq!(team.source, crate::PackageSourceKind::Bundled);
        host.execute(PackageHostCommand::Shutdown(ShutdownHost))
            .await
            .unwrap();
    }
}
