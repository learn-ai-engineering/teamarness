use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::manifest::strict_json;
use crate::{PackageErrorCode, PackageHostConfig};

#[derive(Debug)]
pub(crate) struct CatalogFailure {
    pub(crate) code: PackageErrorCode,
}

#[derive(Debug, Clone)]
pub(crate) struct ApplicationRegistration {
    pub(crate) directory: PathBuf,
    pub(crate) path_error: Option<PackageErrorCode>,
}

#[derive(Debug, Clone)]
pub(crate) struct InstallationRegistration {
    pub(crate) directory: PathBuf,
    pub(crate) expected_identity: String,
    pub(crate) trust_complete: bool,
    pub(crate) override_application: bool,
    pub(crate) path_error: Option<PackageErrorCode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApplicationIndex {
    schema_version: u16,
    packages: Vec<ApplicationEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplicationEntry {
    directory: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstallationIndex {
    schema_version: u16,
    registrations: Vec<InstallationEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstallationEntry {
    directory: String,
    expected_identity: String,
    trust: Option<TrustEvidence>,
    #[serde(default)]
    override_application: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TrustEvidence {
    kind: String,
    accepted_risks: Vec<String>,
}

pub(crate) fn application_registrations(
    config: &PackageHostConfig,
) -> Result<Vec<ApplicationRegistration>, CatalogFailure> {
    let bytes = fs::read(config.application_index()).map_err(|_| invalid_registration())?;
    let value = strict_json(&bytes).map_err(|_| invalid_registration())?;
    let index: ApplicationIndex =
        serde_json::from_value(value).map_err(|_| invalid_registration())?;
    if index.schema_version != 1 {
        return Err(invalid_registration());
    }
    index
        .packages
        .into_iter()
        .map(|entry| {
            let relative = normalized_relative(&entry.directory)?;
            let directory = config.application_root().join(relative);
            let path_error = match fs::symlink_metadata(&directory) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                    Some(PackageErrorCode::InvalidRegistration)
                }
                Ok(_) => None,
                Err(_) => Some(PackageErrorCode::SourceNotFound),
            };
            Ok(ApplicationRegistration {
                directory,
                path_error,
            })
        })
        .collect()
}

pub(crate) fn installation_registrations(
    config: &PackageHostConfig,
) -> Result<Vec<InstallationRegistration>, CatalogFailure> {
    let path = config
        .private_data_directory()
        .join("packages/registrations.v1.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let metadata = fs::symlink_metadata(&path).map_err(|_| invalid_registration())?;
    let expected_owner = fs::metadata(config.private_data_directory())
        .map_err(|_| invalid_registration())?
        .uid();
    let kind = if metadata.file_type().is_symlink() {
        RegistrationFileKind::Symlink
    } else if metadata.is_file() {
        RegistrationFileKind::Regular
    } else {
        RegistrationFileKind::Other
    };
    validate_registration_file_policy(metadata.mode(), metadata.uid(), expected_owner, kind)?;
    let bytes = fs::read(path).map_err(|_| invalid_registration())?;
    let value = strict_json(&bytes).map_err(|_| invalid_registration())?;
    let index: InstallationIndex =
        serde_json::from_value(value).map_err(|_| invalid_registration())?;
    if index.schema_version != 1 {
        return Err(invalid_registration());
    }
    Ok(index
        .registrations
        .into_iter()
        .map(|entry| {
            let directory = PathBuf::from(entry.directory);
            let path_error = if !directory.is_absolute() {
                Some(PackageErrorCode::InvalidRegistration)
            } else {
                match fs::symlink_metadata(&directory) {
                    Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                        Some(PackageErrorCode::InvalidRegistration)
                    }
                    Ok(_) => None,
                    Err(_) => Some(PackageErrorCode::SourceNotFound),
                }
            };
            let directory = if path_error.is_none() {
                fs::canonicalize(&directory).unwrap_or(directory)
            } else {
                directory
            };
            InstallationRegistration {
                directory,
                expected_identity: entry.expected_identity,
                trust_complete: entry.trust.is_some_and(valid_trust),
                override_application: entry.override_application,
                path_error,
            }
        })
        .collect())
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum RegistrationFileKind {
    Regular,
    Symlink,
    Other,
}

pub(crate) fn validate_registration_file_policy(
    mode: u32,
    owner: u32,
    expected_owner: u32,
    kind: RegistrationFileKind,
) -> Result<(), CatalogFailure> {
    if !matches!(kind, RegistrationFileKind::Regular)
        || mode & 0o077 != 0
        || owner != expected_owner
    {
        return Err(invalid_registration());
    }
    Ok(())
}

fn valid_trust(trust: TrustEvidence) -> bool {
    let risks: BTreeSet<_> = trust.accepted_risks.iter().map(String::as_str).collect();
    trust.kind == "trusted_native_code_v1"
        && trust.accepted_risks.len() == 3
        && risks == BTreeSet::from(["filesystem", "network", "process"])
}

fn normalized_relative(value: &str) -> Result<PathBuf, CatalogFailure> {
    if value.is_empty()
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains("//")
        || !value.is_ascii()
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(invalid_registration());
    }
    let path = PathBuf::from(value);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid_registration());
    }
    Ok(path)
}

fn reject_final_symlink(path: &Path) -> Result<(), CatalogFailure> {
    let metadata = fs::symlink_metadata(path).map_err(|_| invalid_registration())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(invalid_registration());
    }
    Ok(())
}

const fn invalid_registration() -> CatalogFailure {
    CatalogFailure {
        code: PackageErrorCode::InvalidRegistration,
    }
}
