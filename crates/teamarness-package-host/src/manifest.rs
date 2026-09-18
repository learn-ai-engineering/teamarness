//! Host adapter over the shared admission rules in `teamarness-package-contract`.
//!
//! The rules themselves live in the contract crate so the author validator applies exactly the
//! same code. The Host keeps only the mapping from a [`ContractErrorCode`] onto its own
//! [`PackageErrorCode`]; the contract crate's `rule` and `pointer` are for the command line and
//! never reach Host diagnostics, which stay payload-free.

use std::path::Path;
use std::str::FromStr;

use semver::Version;
use teamarness_package_contract::error::ContractErrorCode;
use teamarness_package_contract::manifest as contract;

// Re-exported for the Host modules that parse JSON documents; a `#[path]`-included test module
// may not use every one of these.
#[allow(unused_imports)]
pub(crate) use teamarness_package_contract::json::strict_json;
#[allow(unused_imports)]
pub(crate) use teamarness_package_contract::manifest::{
    CapabilityRequirement, LoadedUi, VerifiedFile, VersionRange, sha256,
};

use crate::{PackageErrorCode, PackageIdentity};

#[derive(Debug)]
pub(crate) struct ManifestFailure {
    pub(crate) code: PackageErrorCode,
}

impl ManifestFailure {
    const fn new(code: PackageErrorCode) -> Self {
        Self { code }
    }
}

/// A Package Directory admitted by the shared rules, with a validated Host identity.
#[derive(Debug, Clone)]
pub(crate) struct LoadedManifest {
    pub(crate) identity: PackageIdentity,
    pub(crate) version: Version,
    pub(crate) display_name: String,
    pub(crate) protocol: VersionRange,
    pub(crate) backend: VerifiedFile,
    pub(crate) ui: LoadedUi,
    pub(crate) capabilities: Vec<CapabilityRequirement>,
    pub(crate) manifest_digest: String,
}

pub(crate) fn load(package_root: &Path, target: &str) -> Result<LoadedManifest, ManifestFailure> {
    let loaded = contract::load_package_directory(package_root, target)
        .map_err(|error| ManifestFailure::new(host_code(error.code)))?;
    let identity = PackageIdentity::from_str(&loaded.identity)
        .map_err(|_| ManifestFailure::new(PackageErrorCode::InvalidPackageIdentity))?;
    Ok(LoadedManifest {
        identity,
        version: loaded.version,
        display_name: loaded.display_name,
        protocol: loaded.protocol,
        backend: loaded.backend,
        ui: loaded.ui,
        capabilities: loaded.capabilities,
        manifest_digest: loaded.manifest_digest,
    })
}

/// Maps a shared admission code onto the Host's stable error vocabulary.
pub(crate) const fn host_code(code: ContractErrorCode) -> PackageErrorCode {
    match code {
        ContractErrorCode::ManifestMalformed => PackageErrorCode::ManifestMalformed,
        ContractErrorCode::ManifestUnsupported => PackageErrorCode::ManifestUnsupported,
        ContractErrorCode::InvalidPackageIdentity => PackageErrorCode::InvalidPackageIdentity,
        ContractErrorCode::TargetUnsupported => PackageErrorCode::TargetUnsupported,
        ContractErrorCode::ArtifactMissing => PackageErrorCode::ArtifactMissing,
        ContractErrorCode::ArtifactChanged => PackageErrorCode::ArtifactChanged,
        ContractErrorCode::UiEntrypointMissing => PackageErrorCode::UiEntrypointMissing,
        // `SmokeFailed` belongs to the author validator and never reaches admission; the Host
        // treats it, like a contract violation, as a UI contract failure.
        ContractErrorCode::UiContractViolation | ContractErrorCode::SmokeFailed => {
            PackageErrorCode::UiContractViolation
        }
    }
}
