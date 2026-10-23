use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use crate::catalog::{self, ApplicationRegistration, InstallationRegistration};
use crate::host_store::{HostStore, StoredAdmission};
use crate::manifest::{self, LoadedManifest, sha256};
use crate::ui::{self, AdmittedUi};
use crate::{
    DiagnosticId, HostRevision, PackageCatalogEntry, PackageErrorCode, PackageErrorEnvelope,
    PackageErrorStage, PackageHostConfig, PackageHostSnapshot, PackageHostState, PackageIdentity,
    PackageSourceKind, PackageTrustKind, RecoveryAction,
};

pub(crate) struct AdmissionReport {
    pub(crate) snapshot: PackageHostSnapshot,
    pub(crate) admissions: Vec<Admission>,
    pub(crate) graph: BTreeMap<PackageIdentity, Vec<PortAvailability>>,
    pub(crate) catalog: Vec<crate::AuthoringCatalogEntry>,
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

/// The installed graph's per-declaration availability, before generation grants exist.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortAvailability {
    pub contract: String,
    pub provider: Option<PackageIdentity>,
    pub minor: Option<u16>,
    pub unavailable: Option<PackageErrorCode>,
}

struct Candidate {
    root: PathBuf,
    manifest: LoadedManifest,
    source: PackageSourceKind,
    trust: PackageTrustKind,
}

struct LocalCandidate {
    registration: InstallationRegistration,
    result: Result<Candidate, PackageErrorCode>,
}

pub(crate) fn admit(
    config: &PackageHostConfig,
    store: &HostStore,
) -> Result<AdmissionReport, AdmissionFailure> {
    let mut diagnostics = Vec::new();
    let applications = match catalog::application_registrations(config) {
        Ok(registrations) => registrations,
        Err(error) => {
            diagnostics.push(diagnostic(error.code, PackageErrorStage::Discovery, None));
            Vec::new()
        }
    };
    let installations = match catalog::installation_registrations(config) {
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

    let mut local_by_expected: BTreeMap<String, Vec<LocalCandidate>> = BTreeMap::new();
    for registration in installations {
        let expected = registration.expected_identity.clone();
        let result = load_installation(&registration, config.target());
        local_by_expected
            .entry(expected)
            .or_default()
            .push(LocalCandidate {
                registration,
                result,
            });
    }

    let mut all_identities: BTreeSet<PackageIdentity> =
        application_by_identity.keys().cloned().collect();
    for expected in local_by_expected.keys() {
        match PackageIdentity::from_str(expected) {
            Ok(identity) => {
                all_identities.insert(identity);
            }
            Err(_) => diagnostics.push(diagnostic(
                PackageErrorCode::InvalidPackageIdentity,
                PackageErrorStage::Admission,
                None,
            )),
        }
    }

    let mut selected = Vec::new();
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
            continue;
        }
        let application = applications.pop();
        let local = locals.pop();
        match (application, local) {
            (Some(application), None) => selected.push(application),
            (None, Some(local)) => match local.result {
                Ok(candidate) => selected.push(candidate),
                Err(code) => diagnostics.push(diagnostic(
                    code,
                    PackageErrorStage::Admission,
                    Some(identity),
                )),
            },
            (Some(_), Some(local)) => {
                if !local.registration.override_application {
                    diagnostics.push(diagnostic(
                        PackageErrorCode::OverrideRequired,
                        PackageErrorStage::Admission,
                        Some(identity),
                    ));
                } else {
                    match local.result {
                        Ok(candidate) => selected.push(candidate),
                        Err(code) => diagnostics.push(diagnostic(
                            code,
                            PackageErrorStage::Admission,
                            Some(identity),
                        )),
                    }
                }
            }
            (None, None) => {}
        }
    }

    // Finalize local validity before considering any installation-wide Provider claim.
    let mut eligible = BTreeMap::new();
    for candidate in selected {
        match finalize(config, store, candidate) {
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
            )
        })
        .collect();
    let incumbent = store
        .port_providers()
        .map_err(|error| AdmissionFailure { code: error.code })?;
    let mut claims: BTreeMap<String, Vec<PackageIdentity>> = BTreeMap::new();
    for (identity, (_, admission)) in &eligible {
        if let Some(contracts) = &admission.manifest.port_contract {
            for contract in &contracts.contracts {
                claims
                    .entry(contract.identity.clone())
                    .or_default()
                    .push(identity.clone());
            }
        }
    }
    // Conflict selection is provisional until required dependencies have settled. A
    // graph-invalid winner must not permanently displace a viable rival on reopen.
    let mut candidates = eligible;
    let (mut eligible, providers, remaining) = loop {
        let mut eligible = candidates.clone();
        let mut trial_diagnostics = Vec::new();
        let mut providers = BTreeMap::new();
        // BTreeMap order is stable regardless of registration order. A still-valid
        // incumbent wins over a newly installed, lexically earlier contender.
        loop {
            providers.clear();
            let mut losers = BTreeSet::new();
            for (contract, all_candidates) in &claims {
                let mut contenders: Vec<_> = all_candidates
                    .iter()
                    .filter(|candidate| eligible.contains_key(*candidate))
                    .cloned()
                    .collect();
                if contenders.is_empty() {
                    continue;
                }
                contenders.sort_by_key(|id| {
                    let source = eligible[id].0.source;
                    (matches!(source, PackageSourceKind::Local), id.clone())
                });
                let winner = incumbent
                    .get(contract)
                    .and_then(|id| contenders.iter().find(|candidate| candidate.as_str() == id))
                    .cloned()
                    .unwrap_or_else(|| contenders[0].clone());
                losers.extend(
                    contenders
                        .into_iter()
                        .filter(|candidate| *candidate != winner),
                );
                providers.insert(contract.clone(), winner);
            }
            if losers.is_empty() {
                break;
            }
            for candidate in losers {
                eligible.remove(&candidate);
                trial_diagnostics.push(diagnostic(
                    PackageErrorCode::PortProviderConflict,
                    PackageErrorStage::Admission,
                    Some(candidate),
                ));
            }
        }

        let mut remaining: BTreeSet<_> = eligible.keys().cloned().collect();
        // Reject required cycles before propagating missing dependencies.
        for identity in required_cycle_members(&eligible, &providers) {
            remaining.remove(&identity);
            trial_diagnostics.push(diagnostic(
                PackageErrorCode::PortDependencyCycle,
                PackageErrorStage::Admission,
                Some(identity),
            ));
        }
        loop {
            let failures: Vec<_> = remaining
                .iter()
                .filter_map(|identity| {
                    let admission = &eligible[identity].1;
                    admission
                        .manifest
                        .consumed_ports
                        .iter()
                        .filter(|request| request.required)
                        .find_map(|request| {
                            resolve(request, &providers, &eligible, &remaining)
                                .unavailable
                                .map(|code| (identity.clone(), code))
                        })
                })
                .collect();
            if failures.is_empty() {
                break;
            }
            for (identity, code) in failures {
                remaining.remove(&identity);
                trial_diagnostics.push(diagnostic(
                    code,
                    PackageErrorStage::Admission,
                    Some(identity),
                ));
            }
        }
        let invalid_winners: BTreeSet<_> = providers
            .values()
            .filter(|identity| !remaining.contains(*identity))
            .cloned()
            .collect();
        if invalid_winners.is_empty() {
            diagnostics.extend(trial_diagnostics);
            break (eligible, providers, remaining);
        }
        // Replace one contested invalid winner first. Other winners may only be
        // invalid because they depend on it; a rival can make them viable on retry.
        let contested = providers.iter().find_map(|(contract, winner)| {
            (invalid_winners.contains(winner)
                && claims[contract]
                    .iter()
                    .any(|rival| rival != winner && candidates.contains_key(rival)))
            .then(|| winner.clone())
        });
        let evicted: Vec<_> = contested
            .map(|winner| vec![winner])
            .unwrap_or_else(|| invalid_winners.into_iter().collect());
        for identity in evicted {
            if let Some(reason) = trial_diagnostics
                .iter()
                .find(|error| error.affected_package.as_ref() == Some(&identity))
            {
                diagnostics.push(reason.clone());
            }
            candidates.remove(&identity);
        }
    };
    let graph = remaining
        .iter()
        .map(|identity| {
            let ports = eligible[identity]
                .1
                .manifest
                .consumed_ports
                .iter()
                .map(|request| resolve(request, &providers, &eligible, &remaining))
                .collect();
            (identity.clone(), ports)
        })
        .collect();
    for (contract, identity) in &providers {
        if remaining.contains(identity) {
            store
                .record_port_provider(contract, identity.as_str())
                .map_err(|error| AdmissionFailure { code: error.code })?;
        }
    }
    let catalog = catalog_candidates
        .into_iter()
        .map(|(package, document, consumed)| {
            let availability = consumed
                .iter()
                .map(|request| resolve(request, &providers, &eligible, &remaining))
                .collect();
            let diagnostics = diagnostics
                .iter()
                .filter(|error| error.affected_package.as_ref() == Some(&package.identity))
                .cloned()
                .collect();
            crate::AuthoringCatalogEntry {
                package,
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
    let mut packages = Vec::new();
    let mut admissions = Vec::new();
    for identity in remaining {
        let (catalog, admission) = eligible.remove(&identity).expect("selected identity");
        packages.push(catalog);
        admissions.push(admission);
    }
    packages.sort_by(|left, right| left.identity.cmp(&right.identity));
    admissions.sort_by(|left, right| left.manifest.identity.cmp(&right.manifest.identity));
    diagnostics.sort_by(|left, right| left.diagnostic_id.cmp(&right.diagnostic_id));
    for error in &diagnostics {
        store
            .record_diagnostic(error)
            .map_err(|error| AdmissionFailure { code: error.code })?;
    }
    Ok(AdmissionReport {
        snapshot: PackageHostSnapshot {
            revision: HostRevision::INITIAL,
            state: PackageHostState::Opening,
            packages,
            instances: Vec::new(),
            diagnostics,
        },
        admissions,
        graph,
        catalog,
    })
}

fn resolve(
    request: &teamarness_package_contract::manifest::ConsumedContract,
    providers: &BTreeMap<String, PackageIdentity>,
    eligible: &BTreeMap<PackageIdentity, (PackageCatalogEntry, Admission)>,
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
    eligible: &BTreeMap<PackageIdentity, (PackageCatalogEntry, Admission)>,
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

fn load_installation(
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

fn finalize(
    config: &PackageHostConfig,
    store: &HostStore,
    candidate: Candidate,
) -> Result<(PackageCatalogEntry, Admission), (PackageErrorCode, PackageIdentity)> {
    let identity = candidate.manifest.identity.clone();
    let ui_contract = ui::compile_contract(&candidate.manifest.ui.contract.bytes)
        .map_err(|error| (error.code, identity.clone()))?;
    let admission_digest = admission_digest(&candidate.manifest, candidate.source);
    let ui_cache = ui::populate_cache(
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
    let catalog = PackageCatalogEntry {
        identity,
        version: candidate.manifest.version.to_string(),
        display_name: candidate.manifest.display_name.clone(),
        source: candidate.source,
        trust: candidate.trust,
        ui_contract_digest: candidate.manifest.ui.contract.digest.clone(),
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

fn diagnostic(
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
        | PackageErrorCode::TrustEvidenceMissing => RecoveryAction::CorrectRegistration,
        PackageErrorCode::DataDirectoryPermission => RecoveryAction::RestorePermissions,
        _ => RecoveryAction::CorrectPackageSource,
    };
    let mut error = PackageErrorEnvelope::new(code, stage, false, recovery, id);
    error.affected_package = package;
    error
}
