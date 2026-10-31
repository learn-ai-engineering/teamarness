use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use crate::catalog::{self, ApplicationRegistration, InstallationRegistration};
use crate::host_store::{HostStore, StoredAdmission};
use crate::manifest::{self, LoadedManifest, sha256};
use crate::ui::{self, AdmittedUi};
use crate::{
    DiagnosticId, HostRevision, InstallationRegistrationSnapshot, PackageCatalogEntry,
    PackageErrorCode, PackageErrorEnvelope, PackageErrorStage, PackageHostConfig,
    PackageHostSnapshot, PackageHostState, PackageIdentity, PackageSourceKind, PackageTier,
    PackageTrustKind, PackageUiView, RecoveryAction, RegistrationStatus,
};

pub(crate) struct AdmissionReport {
    pub(crate) snapshot: PackageHostSnapshot,
    pub(crate) admissions: Vec<Admission>,
    pub(crate) graph: BTreeMap<PackageIdentity, Vec<PortAvailability>>,
    pub(crate) catalog: Vec<crate::AuthoringCatalogEntry>,
    /// The tier of every admitted Package and of every authoring catalog entry.
    pub(crate) tiers: BTreeMap<PackageIdentity, PackageTier>,
    /// Contract identity to the Provider the Team tier selected.
    pub(crate) team_providers: BTreeMap<String, PackageIdentity>,
    /// Contract identity to the Provider the Personal tier selected.
    pub(crate) personal_providers: BTreeMap<String, PackageIdentity>,
}

#[derive(Clone)]
pub(crate) struct Admission {
    pub(crate) backend_path: PathBuf,
    pub(crate) ui: Arc<AdmittedUi>,
    pub(crate) manifest: LoadedManifest,
}

#[derive(Debug)]
pub(crate) struct AdmissionFailure {
    pub(crate) code: PackageErrorCode,
}

/// Whether an admission pass may change Host state.
///
/// `Commit` populates the UI cache and records admissions, Port providers, and
/// diagnostics in the Host store. `Preflight` compiles and selects exactly as
/// `Commit` does but writes nothing, so a candidate can be checked before its
/// registration exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdmissionMode {
    Commit,
    Preflight,
}

/// The installed graph's per-declaration availability, before generation grants exist.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortAvailability {
    pub contract: String,
    pub provider: Option<PackageIdentity>,
    pub minor: Option<u16>,
    pub unavailable: Option<PackageErrorCode>,
}

pub(crate) struct Candidate {
    root: PathBuf,
    manifest: LoadedManifest,
    source: PackageSourceKind,
    trust: PackageTrustKind,
}

struct LocalCandidate {
    index: usize,
    registration: InstallationRegistration,
    result: Result<Candidate, PackageErrorCode>,
}

/// One admitted tier: its Packages, its selected Providers, and what it reports.
pub(crate) struct TierOutcome {
    /// Admitted Packages of this tier only.
    pub(crate) admitted: Eligible,
    /// Contract identity to the admitted Provider this tier selected.
    pub(crate) providers: BTreeMap<String, PackageIdentity>,
    pub(crate) graph: BTreeMap<PackageIdentity, Vec<PortAvailability>>,
    pub(crate) catalog: Vec<crate::AuthoringCatalogEntry>,
}

/// The admitted Packages a later tier is selected against, which it cannot change.
#[derive(Default)]
pub(crate) struct FixedGraph<'a> {
    /// Admitted Packages whose provided contracts a candidate may consume.
    pub(crate) admitted: View<'a>,
    /// Contract identity to its fixed Provider.
    pub(crate) providers: BTreeMap<String, PackageIdentity>,
    /// Contracts a candidate may not claim. A claim is refused with `port_provider_conflict`.
    pub(crate) reserved: BTreeSet<String>,
}

impl<'a> FixedGraph<'a> {
    /// The Team tier as the Personal tier sees it: every Team Provider is fixed and
    /// every contract the Team tier provides is reserved (ADR 0036).
    pub(crate) fn team(team: &'a TierOutcome) -> Self {
        Self {
            admitted: team
                .admitted
                .iter()
                .map(|(identity, entry)| (identity.clone(), entry))
                .collect(),
            providers: team.providers.clone(),
            reserved: team
                .admitted
                .values()
                .flat_map(|(_, admission)| provided_contracts(admission))
                .collect(),
        }
    }
}

fn provided_contracts(admission: &Admission) -> Vec<String> {
    admission
        .manifest
        .port_contract
        .iter()
        .flat_map(|document| document.contracts.iter().map(|item| item.identity.clone()))
        .collect()
}

/// What happened to one installation registration before tier selection.
enum Disposition {
    Refused(PackageErrorCode),
    Candidate(PackageIdentity),
}

/// Admits the registered Package Sources at Host open.
pub(crate) fn admit(
    config: &PackageHostConfig,
    store: &HostStore,
) -> Result<AdmissionReport, AdmissionFailure> {
    admit_installation(
        config,
        store,
        catalog::installation_registrations(config),
        AdmissionMode::Commit,
    )
}

/// Admits the application registrations plus `installations` in two tiers.
///
/// Identity collisions are resolved across every registration first. The Team
/// tier (bundled Packages and override registrations) is then admitted over Team
/// Providers only. The Personal tier (every other installation registration) is
/// admitted next against the fixed Team graph (ADR 0036).
pub(crate) fn admit_installation(
    config: &PackageHostConfig,
    store: &HostStore,
    installations: Result<Vec<InstallationRegistration>, catalog::CatalogFailure>,
    mode: AdmissionMode,
) -> Result<AdmissionReport, AdmissionFailure> {
    let mut diagnostics = Vec::new();
    let applications = match catalog::application_registrations(config) {
        Ok(registrations) => registrations,
        Err(error) => {
            diagnostics.push(diagnostic(error.code, PackageErrorStage::Discovery, None));
            Vec::new()
        }
    };
    let installations = match installations {
        Ok(registrations) => registrations,
        Err(error) => {
            diagnostics.push(diagnostic(error.code, PackageErrorStage::Discovery, None));
            Vec::new()
        }
    };

    let mut application_by_identity: BTreeMap<PackageIdentity, Vec<Candidate>> = BTreeMap::new();
    for registration in applications {
        match load_application(registration, config.target()) {
            Ok(candidate) => application_by_identity
                .entry(candidate.manifest.identity.clone())
                .or_default()
                .push(candidate),
            Err(code) => diagnostics.push(diagnostic(code, PackageErrorStage::Admission, None)),
        }
    }

    let mut dispositions: Vec<Option<Disposition>> = Vec::new();
    let mut registrations = Vec::new();
    let mut local_by_expected: BTreeMap<String, Vec<LocalCandidate>> = BTreeMap::new();
    for (index, registration) in installations.into_iter().enumerate() {
        let expected = registration.expected_identity.clone();
        let result = load_installation(&registration, config.target());
        registrations.push(registration.clone());
        dispositions.push(None);
        local_by_expected
            .entry(expected)
            .or_default()
            .push(LocalCandidate {
                index,
                registration,
                result,
            });
    }

    let mut all_identities: BTreeSet<PackageIdentity> =
        application_by_identity.keys().cloned().collect();
    for (expected, locals) in &local_by_expected {
        match PackageIdentity::from_str(expected) {
            Ok(identity) => {
                all_identities.insert(identity);
            }
            Err(_) => {
                diagnostics.push(diagnostic(
                    PackageErrorCode::InvalidPackageIdentity,
                    PackageErrorStage::Admission,
                    None,
                ));
                for local in locals {
                    dispositions[local.index] = Some(Disposition::Refused(
                        PackageErrorCode::InvalidPackageIdentity,
                    ));
                }
            }
        }
    }

    let mut team = Vec::new();
    let mut personal = Vec::new();
    for identity in all_identities {
        let mut applications = application_by_identity
            .remove(&identity)
            .unwrap_or_default();
        let mut locals = local_by_expected
            .remove(identity.as_str())
            .unwrap_or_default();
        if applications.len() > 1 || locals.len() > 1 {
            diagnostics.push(diagnostic(
                PackageErrorCode::DuplicateIdentity,
                PackageErrorStage::Admission,
                Some(identity),
            ));
            for local in &locals {
                dispositions[local.index] =
                    Some(Disposition::Refused(PackageErrorCode::DuplicateIdentity));
            }
            continue;
        }
        let application = applications.pop();
        let local = locals.pop();
        match (application, local) {
            (Some(application), None) => team.push(application),
            (application, Some(local)) => {
                let code = if application.is_some() && !local.registration.override_application {
                    Err(PackageErrorCode::OverrideRequired)
                } else {
                    local.result
                };
                match code {
                    Ok(candidate) => {
                        dispositions[local.index] = Some(Disposition::Candidate(identity.clone()));
                        if local.registration.override_application {
                            team.push(candidate);
                        } else {
                            personal.push(candidate);
                        }
                    }
                    Err(code) => {
                        dispositions[local.index] = Some(Disposition::Refused(code));
                        diagnostics.push(diagnostic(
                            code,
                            PackageErrorStage::Admission,
                            Some(identity),
                        ));
                    }
                }
            }
            (None, None) => {}
        }
    }

    let team = admit_team(config, store, team, mode, &mut diagnostics)?;
    let personal = admit_personal(
        config,
        store,
        &FixedGraph::team(&team),
        personal,
        mode,
        &mut diagnostics,
    )?;

    let mut tiers: BTreeMap<PackageIdentity, PackageTier> = BTreeMap::new();
    for entry in &personal.catalog {
        tiers.insert(entry.package.identity.clone(), PackageTier::Personal);
    }
    for entry in &team.catalog {
        tiers.insert(entry.package.identity.clone(), PackageTier::Team);
    }
    let team_providers = team.providers.clone();
    let personal_providers = personal.providers.clone();
    let mut graph = team.graph;
    graph.extend(personal.graph);
    let mut catalog = team.catalog;
    catalog.extend(personal.catalog);
    catalog.sort_by(|left, right| left.package.identity.cmp(&right.package.identity));
    let mut packages = Vec::new();
    let mut admissions = Vec::new();
    for (_, (entry, admission)) in team.admitted.into_iter().chain(personal.admitted) {
        packages.push(entry);
        admissions.push(admission);
    }
    packages.sort_by(|left, right| left.identity.cmp(&right.identity));
    admissions.sort_by(|left, right| left.manifest.identity.cmp(&right.manifest.identity));
    let registrations = registrations
        .into_iter()
        .zip(dispositions)
        .map(|(registration, disposition)| {
            let status = match disposition {
                Some(Disposition::Refused(code)) => RegistrationStatus::Refused { code },
                Some(Disposition::Candidate(identity)) => {
                    if packages.iter().any(|package| {
                        package.identity == identity && package.source == PackageSourceKind::Local
                    }) {
                        RegistrationStatus::Admitted
                    } else {
                        RegistrationStatus::Refused {
                            code: diagnostics
                                .iter()
                                .rev()
                                .find(|error| error.affected_package.as_ref() == Some(&identity))
                                .map_or(PackageErrorCode::InvalidRegistration, |error| error.code),
                        }
                    }
                }
                // Every registration is reached by the identity pass above.
                None => RegistrationStatus::Refused {
                    code: PackageErrorCode::InvalidRegistration,
                },
            };
            InstallationRegistrationSnapshot {
                directory: registration.directory.to_string_lossy().into_owned(),
                expected_identity: registration.expected_identity,
                override_application: registration.override_application,
                tier: if registration.override_application {
                    PackageTier::Team
                } else {
                    PackageTier::Personal
                },
                status,
            }
        })
        .collect();
    diagnostics.sort_by(|left, right| left.diagnostic_id.cmp(&right.diagnostic_id));
    if mode == AdmissionMode::Commit {
        for error in &diagnostics {
            store
                .record_diagnostic(error)
                .map_err(|error| AdmissionFailure { code: error.code })?;
        }
    }
    Ok(AdmissionReport {
        snapshot: PackageHostSnapshot {
            revision: HostRevision::INITIAL,
            state: PackageHostState::Opening,
            packages,
            instances: Vec::new(),
            diagnostics,
            registrations,
        },
        admissions,
        graph,
        catalog,
        tiers,
        team_providers,
        personal_providers,
    })
}

/// One Personal Package admitted against the running tiers.
pub(crate) struct LiveAdmission {
    pub(crate) entry: PackageCatalogEntry,
    pub(crate) admission: Admission,
    /// The Package's own consumed-Port routes.
    pub(crate) graph: Vec<PortAvailability>,
    /// The contracts it now provides.
    pub(crate) providers: BTreeMap<String, PackageIdentity>,
}

/// The verdict of admitting one Personal registration while the Host runs.
pub(crate) struct LiveVerdict {
    /// The admitted Package, or the code that refused it.
    pub(crate) result: Result<LiveAdmission, PackageErrorCode>,
    /// Its authoring catalog entry, which a graph refusal keeps.
    pub(crate) catalog: Option<crate::AuthoringCatalogEntry>,
    /// Every diagnostic about the candidate.
    pub(crate) diagnostics: Vec<PackageErrorEnvelope>,
}

/// Admits one Personal registration against the running Host (ADR 0036).
///
/// `running` holds every admitted Package except the candidate's own identity:
/// the Team tier and the other running Personal Packages, with `providers`
/// their selected Provider map. Every contract a running Package provides is
/// reserved, so a candidate that claims one is refused with
/// `port_provider_conflict`: a live operation never displaces a running
/// Provider. A running Personal Package whose required Port has since become
/// unavailable stays reserved but is not offered as a Provider.
pub(crate) fn admit_live_personal(
    config: &PackageHostConfig,
    store: &HostStore,
    running: &Eligible,
    providers: &BTreeMap<String, PackageIdentity>,
    registration: &InstallationRegistration,
    mode: AdmissionMode,
) -> Result<LiveVerdict, AdmissionFailure> {
    let expected = PackageIdentity::from_str(&registration.expected_identity).ok();
    let mut diagnostics = Vec::new();
    let candidate = match load_installation(registration, config.target()) {
        Ok(candidate) => candidate,
        Err(code) => {
            diagnostics.push(diagnostic(code, PackageErrorStage::Admission, expected));
            return Ok(LiveVerdict {
                result: Err(code),
                catalog: None,
                diagnostics,
            });
        }
    };
    let identity = candidate.manifest.identity.clone();
    let view: View<'_> = running
        .iter()
        .map(|(identity, entry)| (identity.clone(), entry))
        .collect();
    let healthy = healthy(&view, providers);
    let graph = FixedGraph {
        admitted: view
            .into_iter()
            .filter(|(identity, _)| healthy.contains(identity))
            .collect(),
        providers: providers
            .iter()
            .filter(|(_, provider)| healthy.contains(*provider))
            .map(|(contract, provider)| (contract.clone(), provider.clone()))
            .collect(),
        reserved: running
            .values()
            .flat_map(|(_, admission)| provided_contracts(admission))
            .collect(),
    };
    let mut outcome = admit_personal(
        config,
        store,
        &graph,
        vec![candidate],
        mode,
        &mut diagnostics,
    )?;
    let catalog = outcome.catalog.pop();
    let result = match (
        outcome.admitted.remove(&identity),
        outcome.graph.remove(&identity),
    ) {
        (Some((entry, admission)), Some(graph)) => Ok(LiveAdmission {
            entry,
            admission,
            graph,
            providers: outcome.providers,
        }),
        _ => Err(diagnostics
            .iter()
            .rev()
            .find(|error| error.affected_package.as_ref() == Some(&identity))
            .map_or(PackageErrorCode::InvalidRegistration, |error| error.code)),
    };
    Ok(LiveVerdict {
        result,
        catalog,
        diagnostics,
    })
}

/// The running Packages whose every required Port resolves, transitively.
///
/// A Package whose required Port has become unavailable keeps running but is
/// not offered as a Provider, exactly as [`admit_live_personal`] treats it.
pub(crate) fn healthy(
    running: &View<'_>,
    providers: &BTreeMap<String, PackageIdentity>,
) -> BTreeSet<PackageIdentity> {
    let mut healthy: BTreeSet<_> = running.keys().cloned().collect();
    settle_required(running, providers, &mut healthy);
    healthy
}

/// Resolves `consumed` against `providers`, offering only the `healthy`
/// Packages of `running` as Providers.
pub(crate) fn resolve_routes(
    consumed: &[teamarness_package_contract::manifest::ConsumedContract],
    providers: &BTreeMap<String, PackageIdentity>,
    running: &View<'_>,
    healthy: &BTreeSet<PackageIdentity>,
) -> Vec<PortAvailability> {
    consumed
        .iter()
        .map(|request| resolve(request, providers, running, healthy))
        .collect()
}

/// Admits the Team tier, selecting Providers among Team Packages only.
pub(crate) fn admit_team(
    config: &PackageHostConfig,
    store: &HostStore,
    candidates: Vec<Candidate>,
    mode: AdmissionMode,
    diagnostics: &mut Vec<PackageErrorEnvelope>,
) -> Result<TierOutcome, AdmissionFailure> {
    admit_tier(
        config,
        store,
        &FixedGraph::default(),
        candidates,
        mode,
        diagnostics,
    )
}

/// Admits Personal candidates against `fixed`, which they can consume but never change.
///
/// A candidate whose identity is already admitted in `fixed` is refused with
/// `override_required`, and one that claims a reserved contract with
/// `port_provider_conflict`. Fixed Packages never route to a candidate.
pub(crate) fn admit_personal(
    config: &PackageHostConfig,
    store: &HostStore,
    fixed: &FixedGraph<'_>,
    candidates: Vec<Candidate>,
    mode: AdmissionMode,
    diagnostics: &mut Vec<PackageErrorEnvelope>,
) -> Result<TierOutcome, AdmissionFailure> {
    let mut accepted = Vec::new();
    for candidate in candidates {
        if fixed.admitted.contains_key(&candidate.manifest.identity) {
            diagnostics.push(diagnostic(
                PackageErrorCode::OverrideRequired,
                PackageErrorStage::Admission,
                Some(candidate.manifest.identity.clone()),
            ));
        } else {
            accepted.push(candidate);
        }
    }
    admit_tier(config, store, fixed, accepted, mode, diagnostics)
}

fn admit_tier(
    config: &PackageHostConfig,
    store: &HostStore,
    fixed: &FixedGraph<'_>,
    candidates: Vec<Candidate>,
    mode: AdmissionMode,
    diagnostics: &mut Vec<PackageErrorEnvelope>,
) -> Result<TierOutcome, AdmissionFailure> {
    // Finalize local validity before considering any installation-wide Provider claim.
    let mut eligible = BTreeMap::new();
    for candidate in candidates {
        match finalize(config, store, candidate, mode) {
            Ok((catalog, admission)) => {
                eligible.insert(catalog.identity.clone(), (catalog, admission));
            }
            Err((code, identity)) => diagnostics.push(diagnostic(
                code,
                PackageErrorStage::Admission,
                Some(identity),
            )),
        }
    }
    let catalog_candidates: Vec<_> = eligible
        .values()
        .map(|(package, admission)| {
            (
                package.clone(),
                admission.manifest.port_contract.clone(),
                admission.manifest.consumed_ports.clone(),
                admission
                    .manifest
                    .capabilities
                    .iter()
                    .map(|requirement| crate::InspectedCapability {
                        identity: requirement.family.clone(),
                        major: requirement.major,
                        min_minor: requirement.min_minor,
                        max_minor: requirement.max_minor,
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    // A fixed Provider is never displaced: a claim on a reserved contract is refused.
    let conflicting: Vec<_> = eligible
        .iter()
        .filter(|(_, (_, admission))| {
            provided_contracts(admission)
                .iter()
                .any(|contract| fixed.reserved.contains(contract))
        })
        .map(|(identity, _)| identity.clone())
        .collect();
    for identity in conflicting {
        eligible.remove(&identity);
        diagnostics.push(diagnostic(
            PackageErrorCode::PortProviderConflict,
            PackageErrorStage::Admission,
            Some(identity),
        ));
    }
    let tier: BTreeSet<PackageIdentity> = eligible.keys().cloned().collect();
    let mut view: View<'_> = fixed.admitted.clone();
    view.extend(
        eligible
            .iter()
            .map(|(identity, entry)| (identity.clone(), entry)),
    );
    let incumbent = store
        .port_providers()
        .map_err(|error| AdmissionFailure { code: error.code })?;
    let mut claims: BTreeMap<String, Vec<PackageIdentity>> = BTreeMap::new();
    for (identity, (_, admission)) in &eligible {
        for contract in provided_contracts(admission) {
            claims.entry(contract).or_default().push(identity.clone());
        }
    }
    let (selected_providers, limited) =
        select_provider_graph(&view, &claims, &incumbent, &fixed.providers);
    let selected: BTreeSet<_> = selected_providers.values().cloned().collect();
    let mut providers = fixed.providers.clone();
    providers.extend(selected_providers.clone());
    let claimants: BTreeSet<_> = claims.values().flatten().cloned().collect();
    let mut remaining: BTreeSet<_> = view.keys().cloned().collect();
    remaining.retain(|id| !claimants.contains(id) || selected.contains(id));
    // Preserve the original cycle verdict when every candidate in a required cycle
    // must be discarded; the final empty graph alone cannot reveal that cause.
    let mut preferred = fixed.providers.clone();
    preferred.extend(preferred_providers(&view, &claims, &incumbent));
    let preferred_cycles = required_cycle_members(&view, &preferred);
    for identity in required_cycle_members(&view, &providers) {
        remaining.remove(&identity);
    }
    settle_required(&view, &providers, &mut remaining);
    debug_assert!(
        fixed
            .admitted
            .keys()
            .all(|identity| remaining.contains(identity)),
        "a later tier never removes a fixed Package"
    );
    for identity in &claimants {
        if selected.contains(identity) {
            continue;
        }
        let code = if limited.contains(identity) {
            PackageErrorCode::PortGraphSearchLimit
        } else {
            let missing = view[identity]
                .1
                .manifest
                .consumed_ports
                .iter()
                .filter(|request| request.required)
                .find_map(|request| resolve(request, &providers, &view, &remaining).unavailable);
            let has_winner = claims.iter().any(|(contract, candidates)| {
                candidates.contains(identity) && selected_providers.contains_key(contract)
            });
            if preferred_cycles.contains(identity) && !has_winner {
                PackageErrorCode::PortDependencyCycle
            } else {
                missing.unwrap_or(PackageErrorCode::PortProviderConflict)
            }
        };
        diagnostics.push(diagnostic(
            code,
            PackageErrorStage::Admission,
            Some(identity.clone()),
        ));
    }
    for identity in tier
        .iter()
        .filter(|identity| !claimants.contains(*identity))
    {
        if !remaining.contains(identity) {
            let code = view[identity]
                .1
                .manifest
                .consumed_ports
                .iter()
                .filter(|request| request.required)
                .find_map(|request| resolve(request, &providers, &view, &remaining).unavailable)
                .unwrap_or(PackageErrorCode::PortDependencyCycle);
            diagnostics.push(diagnostic(
                code,
                PackageErrorStage::Admission,
                Some(identity.clone()),
            ));
        }
    }
    let graph = remaining
        .iter()
        .filter(|identity| tier.contains(*identity))
        .map(|identity| {
            let ports = view[identity]
                .1
                .manifest
                .consumed_ports
                .iter()
                .map(|request| resolve(request, &providers, &view, &remaining))
                .collect();
            (identity.clone(), ports)
        })
        .collect();
    let tier_providers: BTreeMap<String, PackageIdentity> = selected_providers
        .into_iter()
        .filter(|(_, identity)| remaining.contains(identity))
        .collect();
    if mode == AdmissionMode::Commit {
        for (contract, identity) in &tier_providers {
            store
                .record_port_provider(contract, identity.as_str())
                .map_err(|error| AdmissionFailure { code: error.code })?;
        }
    }
    let catalog = catalog_candidates
        .into_iter()
        .map(|(package, document, consumed, capabilities)| {
            let availability = consumed
                .iter()
                .map(|request| resolve(request, &providers, &view, &remaining))
                .collect();
            let diagnostics = diagnostics
                .iter()
                .filter(|error| error.affected_package.as_ref() == Some(&package.identity))
                .cloned()
                .collect();
            crate::AuthoringCatalogEntry {
                package,
                capabilities,
                provided: document
                    .as_ref()
                    .map_or_else(Vec::new, |document| document.contracts.clone()),
                definitions: document
                    .as_ref()
                    .map_or_else(serde_json::Map::new, |document| {
                        document.definitions.clone()
                    }),
                consumed,
                availability,
                diagnostics,
            }
        })
        .collect();
    drop(view);
    eligible.retain(|identity, _| remaining.contains(identity));
    Ok(TierOutcome {
        admitted: eligible,
        providers: tier_providers,
        graph,
        catalog,
    })
}

pub(crate) type Eligible = BTreeMap<PackageIdentity, (PackageCatalogEntry, Admission)>;
/// Borrowed admitted entries: a tier's candidates plus the fixed Packages they may consume.
pub(crate) type View<'a> = BTreeMap<PackageIdentity, &'a (PackageCatalogEntry, Admission)>;
const MAX_GRAPH_STATES: usize = 4_096;

fn preferred_providers(
    eligible: &View<'_>,
    claims: &BTreeMap<String, Vec<PackageIdentity>>,
    incumbent: &BTreeMap<String, String>,
) -> BTreeMap<String, PackageIdentity> {
    claims
        .iter()
        .map(|(contract, candidates)| {
            let mut candidates = candidates.clone();
            candidates.sort_by_key(|id| {
                (
                    incumbent
                        .get(contract)
                        .is_none_or(|choice| choice != id.as_str()),
                    matches!(eligible[id].0.source, PackageSourceKind::Local),
                    id.clone(),
                )
            });
            (contract.clone(), candidates[0].clone())
        })
        .collect()
}

fn settle_required(
    eligible: &View<'_>,
    providers: &BTreeMap<String, PackageIdentity>,
    remaining: &mut BTreeSet<PackageIdentity>,
) {
    loop {
        let failures: Vec<_> = remaining
            .iter()
            .filter(|identity| {
                eligible[*identity]
                    .1
                    .manifest
                    .consumed_ports
                    .iter()
                    .filter(|request| request.required)
                    .any(|request| {
                        resolve(request, providers, eligible, remaining)
                            .unavailable
                            .is_some()
                    })
            })
            .cloned()
            .collect();
        if failures.is_empty() {
            break;
        }
        for identity in failures {
            remaining.remove(&identity);
        }
    }
}

fn viable_selection(
    eligible: &View<'_>,
    claims: &BTreeMap<String, Vec<PackageIdentity>>,
    component: &[String],
    providers: &BTreeMap<String, PackageIdentity>,
    fixed: &BTreeMap<String, PackageIdentity>,
) -> bool {
    let selected: BTreeSet<_> = providers.values().cloned().collect();
    // A Package cannot provide one of its contracts after losing another claim.
    if component.iter().any(|contract| {
        claims[contract]
            .iter()
            .any(|id| selected.contains(id) && providers.get(contract) != Some(id))
    }) {
        return false;
    }
    let claimants: BTreeSet<_> = component
        .iter()
        .flat_map(|contract| claims[contract].iter().cloned())
        .collect();
    let mut remaining: BTreeSet<_> = eligible.keys().cloned().collect();
    remaining.retain(|id| !claimants.contains(id) || selected.contains(id));
    let providers = with_fixed(fixed, providers);
    for identity in required_cycle_members(eligible, &providers) {
        remaining.remove(&identity);
    }
    settle_required(eligible, &providers, &mut remaining);
    selected.is_subset(&remaining)
}

struct GraphSearch<'a> {
    eligible: &'a View<'a>,
    claims: &'a BTreeMap<String, Vec<PackageIdentity>>,
    options: Vec<(String, Vec<Option<PackageIdentity>>)>,
    incumbent: &'a BTreeMap<String, String>,
    fixed: &'a BTreeMap<String, PackageIdentity>,
    incumbent_total: usize,
    states: usize,
    exhausted: bool,
    best: Option<BTreeMap<String, PackageIdentity>>,
    best_count: usize,
}

impl GraphSearch<'_> {
    fn visit(
        &mut self,
        index: usize,
        retained: usize,
        chosen: &mut BTreeMap<String, PackageIdentity>,
    ) {
        if self.exhausted || self.best.is_some() && self.best_count == self.incumbent_total {
            return;
        }
        if self.states == MAX_GRAPH_STATES {
            self.exhausted = true;
            return;
        }
        self.states += 1;
        if self.best.is_some()
            && retained
                + self.options[index..]
                    .iter()
                    .filter(|(contract, _)| self.incumbent.contains_key(contract))
                    .count()
                <= self.best_count
        {
            return;
        }
        if index == self.options.len() {
            let component: Vec<_> = self
                .options
                .iter()
                .map(|(contract, _)| contract.clone())
                .collect();
            if viable_selection(self.eligible, self.claims, &component, chosen, self.fixed) {
                self.best_count = retained;
                self.best = Some(chosen.clone());
            }
            return;
        }
        let (contract, candidates) = self.options[index].clone();
        for candidate in candidates {
            if let Some(id) = &candidate {
                chosen.insert(contract.clone(), id.clone());
            }
            let next = retained
                + usize::from(candidate.as_ref().is_some_and(|id| {
                    self.incumbent
                        .get(&contract)
                        .is_some_and(|old| old == id.as_str())
                }));
            self.visit(index + 1, next, chosen);
            chosen.remove(&contract);
            if self.exhausted || self.best.is_some() && self.best_count == self.incumbent_total {
                return;
            }
        }
    }
}

/// Selects one Provider per claimed contract. `fixed` Providers are already settled:
/// claimants may depend on them, but they are never chosen, replaced, or returned.
fn select_provider_graph(
    eligible: &View<'_>,
    claims: &BTreeMap<String, Vec<PackageIdentity>>,
    incumbent: &BTreeMap<String, String>,
    fixed: &BTreeMap<String, PackageIdentity>,
) -> (BTreeMap<String, PackageIdentity>, BTreeSet<PackageIdentity>) {
    // Dependency edges are undirected for search isolation: the Package claiming Y
    // may require X even when no claimant for X names Y.
    let mut adjacent: BTreeMap<String, BTreeSet<String>> = claims
        .keys()
        .map(|contract| (contract.clone(), BTreeSet::new()))
        .collect();
    for (_, admission) in eligible.values() {
        let provided: Vec<_> = admission
            .manifest
            .port_contract
            .iter()
            .flat_map(|document| document.contracts.iter().map(|item| item.identity.clone()))
            .filter(|contract| claims.contains_key(contract))
            .collect();
        let related: Vec<_> = provided
            .iter()
            .cloned()
            .chain(
                admission
                    .manifest
                    .consumed_ports
                    .iter()
                    .filter(|item| item.required && claims.contains_key(&item.identity))
                    .map(|item| item.identity.clone()),
            )
            .collect();
        for contract in &provided {
            for other in &related {
                adjacent
                    .get_mut(contract)
                    .expect("provided contract has a claim")
                    .insert(other.clone());
                adjacent
                    .get_mut(other)
                    .expect("required contract has a claim")
                    .insert(contract.clone());
            }
        }
    }
    let mut unvisited: BTreeSet<_> = claims.keys().cloned().collect();
    let mut providers = BTreeMap::new();
    let mut limited = BTreeSet::new();
    while let Some(start) = unvisited.pop_first() {
        let mut stack = vec![start];
        let mut component = BTreeSet::new();
        while let Some(contract) = stack.pop() {
            for other in &adjacent[&contract] {
                if unvisited.remove(other) {
                    stack.push(other.clone());
                }
            }
            component.insert(contract);
        }
        let component: Vec<_> = component.into_iter().collect();
        if component.iter().all(|contract| claims[contract].len() == 1) {
            let chosen: BTreeMap<_, _> = component
                .iter()
                .map(|contract| (contract.clone(), claims[contract][0].clone()))
                .collect();
            let mut remaining: BTreeSet<_> = eligible.keys().cloned().collect();
            let available = with_fixed(fixed, &chosen);
            for identity in required_cycle_members(eligible, &available) {
                remaining.remove(&identity);
            }
            settle_required(eligible, &available, &mut remaining);
            providers.extend(
                chosen
                    .into_iter()
                    .filter(|(_, identity)| remaining.contains(identity)),
            );
            continue;
        }
        let mut options = Vec::new();
        for contract in &component {
            let mut candidates = claims[contract].clone();
            candidates.sort_by_key(|id| {
                (
                    incumbent.get(contract).is_none_or(|old| old != id.as_str()),
                    matches!(eligible[id].0.source, PackageSourceKind::Local),
                    id.clone(),
                )
            });
            let mut choices: Vec<_> = candidates.into_iter().map(Some).collect();
            choices.push(None);
            options.push((contract.clone(), choices));
        }
        let incumbent_total = options
            .iter()
            .filter(|(contract, choices)| {
                incumbent
                    .get(contract)
                    .is_some_and(|old| choices.iter().flatten().any(|id| id.as_str() == old))
            })
            .count();
        let mut search = GraphSearch {
            eligible,
            claims,
            options,
            incumbent,
            fixed,
            incumbent_total,
            states: 0,
            exhausted: false,
            best: None,
            best_count: 0,
        };
        search.visit(0, 0, &mut BTreeMap::new());
        if search.exhausted {
            for contract in &component {
                limited.extend(claims[contract].iter().cloned());
            }
        } else if let Some(winners) = search.best {
            providers.extend(winners);
        }
    }
    (providers, limited)
}

/// `chosen` Providers over the `fixed` ones a tier is selected against.
fn with_fixed(
    fixed: &BTreeMap<String, PackageIdentity>,
    chosen: &BTreeMap<String, PackageIdentity>,
) -> BTreeMap<String, PackageIdentity> {
    let mut providers = fixed.clone();
    providers.extend(
        chosen
            .iter()
            .map(|(contract, id)| (contract.clone(), id.clone())),
    );
    providers
}

fn resolve(
    request: &teamarness_package_contract::manifest::ConsumedContract,
    providers: &BTreeMap<String, PackageIdentity>,
    eligible: &View<'_>,
    remaining: &BTreeSet<PackageIdentity>,
) -> PortAvailability {
    let mut result = PortAvailability {
        contract: request.identity.clone(),
        provider: None,
        minor: None,
        unavailable: None,
    };
    let descriptor = providers
        .get(&request.identity)
        .filter(|identity| remaining.contains(*identity))
        .and_then(|identity| {
            eligible[identity]
                .1
                .manifest
                .port_contract
                .as_ref()?
                .contracts
                .iter()
                .find(|contract| contract.identity == request.identity)
                .map(|contract| (identity, contract))
        });
    result.unavailable = match descriptor {
        None => Some(PackageErrorCode::PortContractMissing),
        Some((identity, contract)) => {
            if contract.major != request.major || contract.minor < request.min_minor {
                Some(PackageErrorCode::PortVersionMismatch)
            } else if request.ports.iter().any(|requested| {
                !contract
                    .ports
                    .iter()
                    .any(|port| port.name == requested.name && port.kind == requested.kind)
            }) {
                Some(PackageErrorCode::PortMissing)
            } else {
                result.provider = Some(identity.clone());
                result.minor = Some(contract.minor.min(request.max_minor));
                None
            }
        }
    };
    result
}

fn required_cycle_members(
    eligible: &View<'_>,
    providers: &BTreeMap<String, PackageIdentity>,
) -> BTreeSet<PackageIdentity> {
    let all: BTreeSet<_> = eligible.keys().cloned().collect();
    let edges: BTreeMap<_, Vec<_>> = eligible
        .iter()
        .map(|(id, (_, admission))| {
            (
                id.clone(),
                admission
                    .manifest
                    .consumed_ports
                    .iter()
                    .filter(|request| request.required)
                    .filter(|request| {
                        resolve(request, providers, eligible, &all)
                            .unavailable
                            .is_none()
                    })
                    .filter_map(|request| providers.get(&request.identity))
                    .cloned()
                    .collect(),
            )
        })
        .collect();
    // A node belongs to a cycle exactly when it can reach itself through one or more
    // required edges. Nodes leading into a cycle are rejected by the later fixed point.
    edges
        .keys()
        .filter(|origin| {
            let mut visited = BTreeSet::new();
            let mut stack = edges[*origin].clone();
            while let Some(node) = stack.pop() {
                if &node == *origin {
                    return true;
                }
                if visited.insert(node.clone()) {
                    stack.extend(edges.get(&node).into_iter().flatten().cloned());
                }
            }
            false
        })
        .cloned()
        .collect()
}

fn load_application(
    registration: ApplicationRegistration,
    target: &str,
) -> Result<Candidate, PackageErrorCode> {
    if let Some(code) = registration.path_error {
        return Err(code);
    }
    let manifest = manifest::load(&registration.directory, target).map_err(|error| error.code)?;
    Ok(Candidate {
        root: registration.directory,
        manifest,
        source: PackageSourceKind::Bundled,
        trust: PackageTrustKind::ApplicationBundled,
    })
}

pub(crate) fn load_installation(
    registration: &InstallationRegistration,
    target: &str,
) -> Result<Candidate, PackageErrorCode> {
    if let Some(code) = registration.path_error {
        return Err(code);
    }
    if !registration.trust_complete {
        return Err(PackageErrorCode::TrustEvidenceMissing);
    }
    let expected = PackageIdentity::from_str(&registration.expected_identity)
        .map_err(|_| PackageErrorCode::InvalidPackageIdentity)?;
    let manifest = manifest::load(&registration.directory, target).map_err(|error| error.code)?;
    if manifest.identity != expected {
        return Err(PackageErrorCode::IdentityMismatch);
    }
    Ok(Candidate {
        root: registration.directory.clone(),
        manifest,
        source: PackageSourceKind::Local,
        trust: PackageTrustKind::TrustedNativeCodeV1,
    })
}

/// Compiles one candidate. Only `Commit` writes its UI cache and admissions row.
fn finalize(
    config: &PackageHostConfig,
    store: &HostStore,
    candidate: Candidate,
    mode: AdmissionMode,
) -> Result<(PackageCatalogEntry, Admission), (PackageErrorCode, PackageIdentity)> {
    let identity = candidate.manifest.identity.clone();
    let ui_contract = ui::compile_contract(&candidate.manifest.ui.contract.bytes)
        .map_err(|error| (error.code, identity.clone()))?;
    let admission_digest = admission_digest(&candidate.manifest, candidate.source);
    let ui_cache = match mode {
        AdmissionMode::Commit => {
            let cache = ui::populate_cache(
                config.private_data_directory(),
                &admission_digest,
                &candidate.manifest,
            )
            .map_err(|error| (error.code, identity.clone()))?;
            store
                .record_admission(StoredAdmission {
                    admission_digest: &admission_digest,
                    package_identity: identity.as_str(),
                    package_version: &candidate.manifest.version.to_string(),
                    source: candidate.source,
                    manifest_digest: &candidate.manifest.manifest_digest,
                    backend_digest: &candidate.manifest.backend.digest,
                    ui_contract_digest: &candidate.manifest.ui.contract.digest,
                })
                .map_err(|error| (error.code, identity.clone()))?;
            cache
        }
        AdmissionMode::Preflight => ui::planned_cache(
            config.private_data_directory(),
            &admission_digest,
            &candidate.manifest,
        ),
    };
    let catalog = PackageCatalogEntry {
        identity,
        version: candidate.manifest.version.to_string(),
        display_name: candidate.manifest.display_name.clone(),
        description: candidate.manifest.description.clone(),
        author: candidate.manifest.author.clone(),
        homepage: candidate.manifest.homepage.clone(),
        source: candidate.source,
        trust: candidate.trust,
        ui_contract_digest: candidate.manifest.ui.contract.digest.clone(),
        views: candidate
            .manifest
            .ui
            .views
            .iter()
            .map(|view| match view {
                teamarness_package_contract::manifest::UiView::Settings => PackageUiView::Settings,
            })
            .collect(),
    };
    let backend_path = candidate
        .root
        .join(&candidate.manifest.backend.relative_path);
    Ok((
        catalog,
        Admission {
            backend_path,
            ui: Arc::new(AdmittedUi {
                cache: ui_cache,
                contract: ui_contract,
            }),
            manifest: candidate.manifest,
        },
    ))
}

fn admission_digest(manifest: &LoadedManifest, source: PackageSourceKind) -> String {
    let source = match source {
        PackageSourceKind::Bundled => "bundled",
        PackageSourceKind::Local => "local",
    };
    let mut content = format!(
        "{source}\0{}\0{}\0{}\0{}\0",
        manifest.manifest_digest,
        manifest.backend.digest,
        manifest.ui.entrypoint.digest,
        manifest.ui.contract.digest,
    );
    if let Some(digest) = &manifest.port_contract_digest {
        content.push_str(digest);
        content.push('\0');
    }
    for asset in &manifest.ui.assets {
        content.push_str(&asset.file.relative_path);
        content.push('\0');
        content.push_str(&asset.file.digest);
        content.push('\0');
    }
    sha256(content.as_bytes())
}

pub(crate) fn diagnostic(
    code: PackageErrorCode,
    stage: PackageErrorStage,
    package: Option<PackageIdentity>,
) -> PackageErrorEnvelope {
    let key = format!(
        "{stage:?}\0{code:?}\0{}",
        package.as_ref().map_or("", PackageIdentity::as_str)
    );
    let id = DiagnosticId::new(format!("diag-{}", &sha256(key.as_bytes())[..24]))
        .expect("digest-based diagnostic ID is valid");
    let recovery = match code {
        PackageErrorCode::InvalidRegistration
        | PackageErrorCode::DuplicateIdentity
        | PackageErrorCode::OverrideRequired
        | PackageErrorCode::TrustEvidenceMissing
        | PackageErrorCode::PortGraphSearchLimit => RecoveryAction::CorrectRegistration,
        PackageErrorCode::DataDirectoryPermission => RecoveryAction::RestorePermissions,
        _ => RecoveryAction::CorrectPackageSource,
    };
    let mut error = PackageErrorEnvelope::new(code, stage, false, recovery, id);
    error.affected_package = package;
    error
}
