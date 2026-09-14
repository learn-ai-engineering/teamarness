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
}

pub(crate) struct Admission {
    pub(crate) backend_path: PathBuf,
    pub(crate) ui: Arc<AdmittedUi>,
    pub(crate) manifest: LoadedManifest,
}

#[derive(Debug)]
pub(crate) struct AdmissionFailure {
    pub(crate) code: PackageErrorCode,
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

pub(crate) fn admit(config: &PackageHostConfig) -> Result<AdmissionReport, AdmissionFailure> {
    let store = HostStore::open(config.private_data_directory())
        .map_err(|error| AdmissionFailure { code: error.code })?;
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

    let mut packages = Vec::new();
    let mut admissions = Vec::new();
    for candidate in selected {
        match finalize(config, &store, candidate) {
            Ok((catalog, admission)) => {
                packages.push(catalog);
                admissions.push(admission);
            }
            Err((code, identity)) => diagnostics.push(diagnostic(
                code,
                PackageErrorStage::Admission,
                Some(identity),
            )),
        }
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
    })
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
