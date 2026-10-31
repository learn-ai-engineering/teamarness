use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

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

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstallationIndex {
    schema_version: u16,
    registrations: Vec<InstallationEntry>,
}

/// One entry of `registrations.v1.json`, exactly as the file spells it.
///
/// The registration store reads and rewrites these, so an entry it does not
/// change keeps its meaning.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct InstallationEntry {
    pub(crate) directory: String,
    pub(crate) expected_identity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) trust: Option<TrustEvidence>,
    #[serde(default)]
    pub(crate) override_application: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TrustEvidence {
    pub(crate) kind: String,
    pub(crate) accepted_risks: Vec<String>,
}

/// The only Package Trust kind an installation registration may carry.
pub(crate) const TRUST_KIND: &str = "trusted_native_code_v1";
/// The exact risks `trusted_native_code_v1` evidence accepts.
pub(crate) const TRUST_RISKS: [&str; 3] = ["filesystem", "network", "process"];

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
    Ok(registrations_from_entries(
        read_installation_entries(config)?.unwrap_or_default(),
    ))
}

/// The installation registration file of this Host configuration.
pub(crate) fn installation_file(config: &PackageHostConfig) -> PathBuf {
    config
        .private_data_directory()
        .join("packages/registrations.v1.json")
}

/// Reads the installation registration file under its file policy.
///
/// `None` means the file does not exist. Any other unreadable, unsafe, or
/// malformed file is an error, never an empty list.
pub(crate) fn read_installation_entries(
    config: &PackageHostConfig,
) -> Result<Option<Vec<InstallationEntry>>, CatalogFailure> {
    let path = installation_file(config);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(invalid_registration()),
    };
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
    parse_installation_entries(&bytes).map(Some)
}

/// Parses `registrations.v1.json` bytes with the same rules the Host reads them with.
pub(crate) fn parse_installation_entries(
    bytes: &[u8],
) -> Result<Vec<InstallationEntry>, CatalogFailure> {
    let value = strict_json(bytes).map_err(|_| invalid_registration())?;
    let index: InstallationIndex =
        serde_json::from_value(value).map_err(|_| invalid_registration())?;
    if index.schema_version != 1 {
        return Err(invalid_registration());
    }
    Ok(index.registrations)
}

/// Serializes entries as a v1 installation registration file.
pub(crate) fn serialize_installation_entries(entries: &[InstallationEntry]) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(&InstallationIndex {
        schema_version: 1,
        registrations: entries.to_vec(),
    })
    .expect("installation entries serialize");
    bytes.push(b'\n');
    bytes
}

/// Resolves file entries into registrations, checking each directory.
pub(crate) fn registrations_from_entries(
    entries: Vec<InstallationEntry>,
) -> Vec<InstallationRegistration> {
    entries
        .into_iter()
        .map(|entry| {
            let directory = PathBuf::from(entry.directory);
            let path_error = directory_error(&directory);
            let directory = if path_error.is_none() {
                fs::canonicalize(&directory).unwrap_or(directory)
            } else {
                directory
            };
            InstallationRegistration {
                directory,
                expected_identity: entry.expected_identity,
                trust_complete: entry.trust.as_ref().is_some_and(valid_trust),
                override_application: entry.override_application,
                path_error,
            }
        })
        .collect()
}

/// The registration rule an installation Package Directory path breaks, if any.
pub(crate) fn directory_error(directory: &Path) -> Option<PackageErrorCode> {
    if !directory.is_absolute() {
        return Some(PackageErrorCode::InvalidRegistration);
    }
    match fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Some(PackageErrorCode::InvalidRegistration)
        }
        Ok(_) => None,
        Err(_) => Some(PackageErrorCode::SourceNotFound),
    }
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

fn valid_trust(trust: &TrustEvidence) -> bool {
    trust.kind == TRUST_KIND && exact_risks(&trust.accepted_risks)
}

/// Whether `risks` is exactly the `trusted_native_code_v1` risk set, each once.
pub(crate) fn exact_risks(risks: &[String]) -> bool {
    let set: BTreeSet<_> = risks.iter().map(String::as_str).collect();
    risks.len() == TRUST_RISKS.len() && set == BTreeSet::from(TRUST_RISKS)
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

const fn invalid_registration() -> CatalogFailure {
    CatalogFailure {
        code: PackageErrorCode::InvalidRegistration,
    }
}
