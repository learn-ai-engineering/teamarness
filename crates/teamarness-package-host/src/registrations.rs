//! The installation registration store: inspect a Package Directory, then add or
//! remove its entry in `registrations.v1.json` (ADR 0031).
//!
//! The store shares one definition of a valid file with the reader in
//! [`crate::catalog`]. It refuses to rewrite a file that breaks the file policy
//! or does not parse, so an entry a person wrote by hand is never lost. A write
//! is committed only after a preflight admission, which writes no Host state,
//! shows that the candidate would be admitted. The public functions change the
//! file only, and a running Host admits that change when it next opens. The
//! Host's live Personal commands use the same store under the Personal tier
//! lock (`crate::host::personal`).

// Every Host operation returns the full, payload-free error envelope, as the Host facade does.
#![allow(clippy::result_large_err)]

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use crate::admission::{self, AdmissionMode};
use crate::catalog::{self, InstallationEntry, TrustEvidence};
use crate::diag::fault;
use crate::errors::host_error;
use crate::host_store::HostStore;
use crate::manifest::{self, strict_json};
use crate::values::{
    InspectedCapability, InspectedPackage, InspectedPackageKind, InspectedPresentation,
    RegistrationRequest, RegistrationStatus,
};
use crate::{
    PackageErrorCode, PackageErrorEnvelope, PackageErrorStage, PackageHostConfig, PackageIdentity,
    RecoveryAction,
};

/// Reads a Package Directory for a preview without running any Package code.
///
/// The path must be absolute, exist, be a directory, and not be a symbolic link.
/// The manifest is loaded for the Host target with every artifact digest checked.
///
/// # Errors
///
/// Returns `invalid_registration` for a relative path, a symbolic link, or a
/// file; `source_not_found` for a missing path; the manifest's own code for an
/// invalid Package Directory; and `registration_file_invalid` when the existing
/// installation registration file cannot be read under its policy.
pub fn inspect_package_directory(
    config: &PackageHostConfig,
    path: &Path,
) -> Result<InspectedPackage, PackageErrorEnvelope> {
    let entries = read_entries(config)?;
    inspect(config, path, &entries)
}

/// Adds or replaces the registration for `request.expected_identity`.
///
/// Other entries are kept. Nothing is written unless the directory inspects as
/// that identity, the request accepts exactly the `trusted_native_code_v1`
/// risks, a bundled-identity collision is confirmed with
/// `override_application`, and a preflight admission of the resulting file
/// admits the candidate. The file is written atomically as `0600` under a
/// `0700` parent.
///
/// # Errors
///
/// Returns `registration_file_invalid` for an existing file it will not
/// rewrite, any inspect error, `identity_mismatch`, `trust_evidence_missing`,
/// `override_required`, the candidate's admission code, or
/// `data_directory_unavailable` when the file cannot be written.
pub fn write_registration(
    config: &PackageHostConfig,
    request: RegistrationRequest,
) -> Result<(), PackageErrorEnvelope> {
    let entries = read_entries(config)?;
    let proposed = propose(config, &request, entries)?;
    let store = HostStore::open(config.private_data_directory())
        .map_err(|failure| host_error(failure.code, PackageErrorStage::Storage))?;
    preflight(config, &store, &proposed, &request.expected_identity)?;
    write_entries(config, &proposed)
}

/// Removes the registration for `identity`, keeping every other entry.
///
/// The Package Data Directory is never touched.
///
/// # Errors
///
/// Returns `registration_file_invalid` for an existing file it will not
/// rewrite, `not_registered` when no entry has the identity, or
/// `data_directory_unavailable` when the file cannot be written.
pub fn remove_registration(
    config: &PackageHostConfig,
    identity: &PackageIdentity,
) -> Result<(), PackageErrorEnvelope> {
    let mut entries = read_entries(config)?;
    let before = entries.len();
    entries.retain(|entry| entry.expected_identity != identity.as_str());
    if entries.len() == before {
        return Err(refusal(
            PackageErrorCode::NotRegistered,
            PackageErrorStage::Discovery,
            Some(identity.clone()),
        ));
    }
    write_entries(config, &entries)
}

/// Checks `request` against the current entries and returns the file it would write.
pub(crate) fn propose(
    config: &PackageHostConfig,
    request: &RegistrationRequest,
    mut entries: Vec<InstallationEntry>,
) -> Result<Vec<InstallationEntry>, PackageErrorEnvelope> {
    let identity = Some(request.expected_identity.clone());
    let inspected = inspect(config, &request.directory, &entries)?;
    if inspected.identity != request.expected_identity {
        return Err(refusal(
            PackageErrorCode::IdentityMismatch,
            PackageErrorStage::Admission,
            identity,
        ));
    }
    if !catalog::exact_risks(&request.accepted_risks) {
        return Err(refusal(
            PackageErrorCode::TrustEvidenceMissing,
            PackageErrorStage::Admission,
            identity,
        ));
    }
    if inspected.kind == InspectedPackageKind::OverridesTeam && !request.override_application {
        return Err(refusal(
            PackageErrorCode::OverrideRequired,
            PackageErrorStage::Admission,
            identity,
        ));
    }
    let entry = InstallationEntry {
        directory: inspected.directory.to_string_lossy().into_owned(),
        expected_identity: request.expected_identity.to_string(),
        trust: Some(TrustEvidence {
            kind: catalog::TRUST_KIND.to_owned(),
            accepted_risks: catalog::TRUST_RISKS.map(str::to_owned).to_vec(),
        }),
        override_application: request.override_application,
    };
    // The new entry takes the place of the first entry for its identity, and any
    // duplicate entry for that identity is dropped.
    let mut placed = false;
    entries.retain_mut(|existing| {
        if existing.expected_identity != entry.expected_identity {
            return true;
        }
        if placed {
            return false;
        }
        placed = true;
        *existing = entry.clone();
        true
    });
    if !placed {
        entries.push(entry);
    }
    Ok(entries)
}

/// Admits `proposed` without writing any Host state and reports the candidate's verdict.
pub(crate) fn preflight(
    config: &PackageHostConfig,
    store: &HostStore,
    proposed: &[InstallationEntry],
    identity: &PackageIdentity,
) -> Result<(), PackageErrorEnvelope> {
    let report = admission::admit_installation(
        config,
        store,
        Ok(catalog::registrations_from_entries(proposed.to_vec())),
        AdmissionMode::Preflight,
    )
    .map_err(|failure| host_error(failure.code, PackageErrorStage::Admission))?;
    let status = report
        .snapshot
        .registrations
        .iter()
        .find(|registration| registration.expected_identity == identity.as_str())
        .map_or(
            RegistrationStatus::Refused {
                code: PackageErrorCode::InvalidRegistration,
            },
            |registration| registration.status,
        );
    match status {
        RegistrationStatus::Admitted => Ok(()),
        RegistrationStatus::Refused { code } => Err(refusal(
            code,
            PackageErrorStage::Admission,
            Some(identity.clone()),
        )),
        // A fresh admission never reports a pending change.
        RegistrationStatus::PendingRestart { .. } => Err(refusal(
            PackageErrorCode::InvalidRegistration,
            PackageErrorStage::Admission,
            Some(identity.clone()),
        )),
    }
}

fn inspect(
    config: &PackageHostConfig,
    path: &Path,
    entries: &[InstallationEntry],
) -> Result<InspectedPackage, PackageErrorEnvelope> {
    if let Some(code) = catalog::directory_error(path) {
        return Err(refusal(code, PackageErrorStage::Discovery, None));
    }
    let directory = fs::canonicalize(path).map_err(fault!(refusal(
        PackageErrorCode::SourceNotFound,
        PackageErrorStage::Discovery,
        None
    )))?;
    let loaded = manifest::load(&directory, config.target())
        .map_err(|failure| refusal(failure.code, PackageErrorStage::Admission, None))?;
    let identity = loaded.identity.clone();
    let registered = catalog::registrations_from_entries(entries.to_vec())
        .into_iter()
        .filter(|registration| registration.expected_identity == identity.as_str())
        .map(|registration| registration.directory)
        .collect::<Vec<_>>();
    let kind = if application_identities(config).contains(&identity) {
        InspectedPackageKind::OverridesTeam
    } else if registered.contains(&directory) {
        InspectedPackageKind::Reload
    } else if registered.is_empty() {
        InspectedPackageKind::New
    } else {
        InspectedPackageKind::ReplacesRegistration
    };
    Ok(InspectedPackage {
        directory,
        identity,
        version: loaded.version.to_string(),
        presentation: InspectedPresentation {
            display_name: loaded.display_name,
            description: loaded.description,
            author: loaded.author,
            homepage: loaded.homepage,
        },
        capabilities: loaded
            .capabilities
            .iter()
            .map(|requirement| InspectedCapability {
                identity: requirement.family.clone(),
                major: requirement.major,
                min_minor: requirement.min_minor,
                max_minor: requirement.max_minor,
            })
            .collect(),
        provided: loaded
            .port_contract
            .as_ref()
            .map_or_else(Vec::new, |document| document.contracts.clone()),
        consumed: loaded.consumed_ports,
        kind,
    })
}

/// The identities the application index registers, read from each manifest's
/// `identity` field only. A bundled Package that fails admission still owns its
/// identity, so an installation registration for it still needs an override.
pub(crate) fn application_identities(config: &PackageHostConfig) -> BTreeSet<PackageIdentity> {
    catalog::application_registrations(config)
        .unwrap_or_default()
        .into_iter()
        .filter(|registration| registration.path_error.is_none())
        .filter_map(|registration| {
            let bytes = fs::read(registration.directory.join("teamarness.package.json")).ok()?;
            strict_json(&bytes)
                .ok()?
                .get("identity")?
                .as_str()?
                .parse()
                .ok()
        })
        .collect()
}

/// The current entries. A missing file has none; a file the reader would refuse is an error.
pub(crate) fn read_entries(
    config: &PackageHostConfig,
) -> Result<Vec<InstallationEntry>, PackageErrorEnvelope> {
    catalog::read_installation_entries(config)
        .map(Option::unwrap_or_default)
        .map_err(|_| {
            refusal(
                PackageErrorCode::RegistrationFileInvalid,
                PackageErrorStage::Discovery,
                None,
            )
        })
}

/// Replaces the registration file atomically: a `0600` temporary file in the same
/// `0700` directory, synced, renamed into place, then the directory synced.
pub(crate) fn write_entries(
    config: &PackageHostConfig,
    entries: &[InstallationEntry],
) -> Result<(), PackageErrorEnvelope> {
    let unavailable = || {
        host_error(
            PackageErrorCode::DataDirectoryUnavailable,
            PackageErrorStage::Storage,
        )
    };
    let path = catalog::installation_file(config);
    let parent = path.parent().ok_or_else(unavailable)?;
    match fs::symlink_metadata(parent) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(unavailable());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)
                .map_err(fault!(unavailable()))?;
        }
        Err(error) => {
            crate::diag::record(concat!(file!(), ":", line!()), &error);
            return Err(unavailable());
        }
    }
    let bytes = catalog::serialize_installation_entries(entries);
    let mut random = [0_u8; 8];
    let _ = getrandom::fill(&mut random);
    let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let temporary = parent.join(format!(".registrations.v1.json.{suffix}.tmp"));
    let written = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, &path)?;
        fs::File::open(parent)?.sync_all()
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        crate::diag::record(concat!(file!(), ":", line!()), &error);
        return Err(unavailable());
    }
    Ok(())
}

/// A registration-store refusal, with the recovery a person can take.
pub(crate) fn refusal(
    code: PackageErrorCode,
    stage: PackageErrorStage,
    package: Option<PackageIdentity>,
) -> PackageErrorEnvelope {
    let mut error = host_error(code, stage);
    if matches!(
        code,
        PackageErrorCode::InvalidRegistration
            | PackageErrorCode::RegistrationFileInvalid
            | PackageErrorCode::NotRegistered
            | PackageErrorCode::DuplicateIdentity
            | PackageErrorCode::OverrideRequired
            | PackageErrorCode::TrustEvidenceMissing
            | PackageErrorCode::IdentityMismatch
    ) {
        error.recovery = RecoveryAction::CorrectRegistration;
    }
    error.affected_package = package;
    error
}
