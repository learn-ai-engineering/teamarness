//! Host adapter over the shared UI contract profile, plus the admitted UI resource cache.
//!
//! The profile rules live in `teamarness-package-contract` so the author validator applies the
//! same code. This module keeps the Host's cache, resource resolution and error mapping.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use teamarness_package_contract::ui_contract as contract;

use crate::manifest::{LoadedManifest, VerifiedFile, sha256};
use crate::{PackageErrorCode, PackageValue};

// The protocol layer bounds UI payloads with the shared constant; a `#[path]`-included test
// module may not use it.
#[allow(unused_imports)]
pub(crate) use teamarness_package_contract::ui_contract::MAX_UI_PAYLOAD_BYTES;

/// The event streams one Package UI Mount may hold open at once.
///
/// The Shell enforces the same bound before it asks (`MAX_OPEN_SUBSCRIPTIONS`
/// in `package-frame/protocol.ts`); `check-protocol-bounds.mjs` reads both so
/// neither can move alone. A Shell that allowed more would have its extra
/// subscriptions refused here as `overloaded`.
pub const MAX_UI_SUBSCRIPTIONS_PER_MOUNT: usize = 8;

#[derive(Debug)]
pub(crate) struct UiContractFailure {
    pub(crate) code: PackageErrorCode,
}

pub(crate) struct AdmittedUi {
    pub(crate) cache: UiCacheRecord,
    pub(crate) contract: CompiledUiContract,
}

/// An admitted UI contract. The Host reports only the stable code; the shared crate's `rule` and
/// `pointer` are author diagnostics and stop here.
pub(crate) struct CompiledUiContract(contract::CompiledUiContract);

#[derive(Debug, Clone)]
pub(crate) struct UiCacheRecord {
    pub(crate) admitted_root: PathBuf,
    pub(crate) entrypoint: String,
    pub(crate) resources: BTreeMap<String, UiResourceRecord>,
    pub(crate) contract_digest: String,
}

#[derive(Debug, Clone)]
pub(crate) struct UiResourceRecord {
    pub(crate) relative_path: String,
    pub(crate) media_type: String,
    pub(crate) size: u64,
    pub(crate) digest: String,
}

pub(crate) struct ResolvedUiResource {
    pub(crate) bytes: Vec<u8>,
    pub(crate) media_type: String,
}

impl CompiledUiContract {
    pub(crate) fn validate_request(
        &self,
        operation: &str,
        payload: &PackageValue,
    ) -> Result<(), UiContractFailure> {
        self.validate(payload, |value| self.0.validate_request(operation, value))
    }

    pub(crate) fn validate_response(
        &self,
        operation: &str,
        payload: &PackageValue,
    ) -> Result<(), UiContractFailure> {
        self.validate(payload, |value| self.0.validate_response(operation, value))
    }

    pub(crate) fn validate_subscription(
        &self,
        operation: &str,
        payload: &PackageValue,
    ) -> Result<(), UiContractFailure> {
        self.validate(payload, |value| {
            self.0.validate_subscription(operation, value)
        })
    }

    pub(crate) fn validate_event(
        &self,
        operation: &str,
        payload: &PackageValue,
    ) -> Result<(), UiContractFailure> {
        self.validate(payload, |value| self.0.validate_event(operation, value))
    }

    fn validate(
        &self,
        payload: &PackageValue,
        check: impl FnOnce(&serde_json::Value) -> Result<(), teamarness_package_contract::ContractError>,
    ) -> Result<(), UiContractFailure> {
        let value = serde_json::to_value(payload).map_err(|_| failure())?;
        check(&value).map_err(|_| failure())
    }
}

pub(crate) fn compile_contract(bytes: &[u8]) -> Result<CompiledUiContract, UiContractFailure> {
    contract::compile_ui_contract(bytes)
        .map(CompiledUiContract)
        .map_err(|_| failure())
}

pub(crate) fn populate_cache(
    private_data_directory: &Path,
    admission_digest: &str,
    manifest: &LoadedManifest,
) -> Result<UiCacheRecord, UiContractFailure> {
    let cache_parent = private_data_directory.join("packages/ui-cache/v1");
    fs::create_dir_all(&cache_parent).map_err(|_| failure())?;
    set_mode(&cache_parent, 0o700)?;
    let admitted_root = cache_parent.join(admission_digest);
    if admitted_root.exists() {
        verify_cache_root(&admitted_root)?;
        verify_cached_file(&admitted_root, &manifest.ui.entrypoint)?;
        verify_cached_file(&admitted_root, &manifest.ui.contract)?;
        for asset in &manifest.ui.assets {
            verify_cached_file(&admitted_root, &asset.file)?;
        }
    } else {
        let temporary =
            cache_parent.join(format!(".{admission_digest}.tmp-{}", std::process::id()));
        if temporary.exists() {
            fs::remove_dir_all(&temporary).map_err(|_| failure())?;
        }
        fs::create_dir(&temporary).map_err(|_| failure())?;
        copy_file(&temporary, &manifest.ui.entrypoint)?;
        copy_file(&temporary, &manifest.ui.contract)?;
        for asset in &manifest.ui.assets {
            copy_file(&temporary, &asset.file)?;
        }
        make_tree_read_only(&temporary)?;
        fs::rename(&temporary, &admitted_root).map_err(|_| failure())?;
    }
    Ok(cache_record(admitted_root, manifest))
}

/// The cache record an admission would serve, without writing the cache.
///
/// A preflight admission uses it so that checking a candidate leaves no UI
/// cache entry behind.
pub(crate) fn planned_cache(
    private_data_directory: &Path,
    admission_digest: &str,
    manifest: &LoadedManifest,
) -> UiCacheRecord {
    let admitted_root = private_data_directory
        .join("packages/ui-cache/v1")
        .join(admission_digest);
    cache_record(admitted_root, manifest)
}

fn cache_record(admitted_root: PathBuf, manifest: &LoadedManifest) -> UiCacheRecord {
    let entrypoint = UiResourceRecord {
        relative_path: manifest.ui.entrypoint.relative_path.clone(),
        media_type: "text/javascript".to_owned(),
        size: manifest.ui.entrypoint.bytes.len() as u64,
        digest: manifest.ui.entrypoint.digest.clone(),
    };
    let mut resources = BTreeMap::from([(entrypoint.relative_path.clone(), entrypoint)]);
    resources.extend(manifest.ui.assets.iter().map(|asset| {
        let record = UiResourceRecord {
            relative_path: asset.file.relative_path.clone(),
            media_type: asset.media_type.clone(),
            size: asset.file.bytes.len() as u64,
            digest: asset.file.digest.clone(),
        };
        (record.relative_path.clone(), record)
    }));
    UiCacheRecord {
        admitted_root,
        entrypoint: manifest.ui.entrypoint.relative_path.clone(),
        resources,
        contract_digest: manifest.ui.contract.digest.clone(),
    }
}

impl UiCacheRecord {
    pub(crate) fn resolve(
        &self,
        relative_path: &str,
    ) -> Result<ResolvedUiResource, UiContractFailure> {
        validate_resource_path(relative_path)?;
        let resource = self.resources.get(relative_path).ok_or_else(failure)?;
        let path = self.admitted_root.join(&resource.relative_path);
        let metadata = fs::symlink_metadata(&path).map_err(|_| failure())?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.permissions().mode() & 0o777 != 0o400
            || metadata.len() != resource.size
        {
            return Err(failure());
        }
        let bytes = fs::read(path).map_err(|_| failure())?;
        if sha256(&bytes) != resource.digest {
            return Err(failure());
        }
        Ok(ResolvedUiResource {
            bytes,
            media_type: resource.media_type.clone(),
        })
    }
}

fn validate_resource_path(value: &str) -> Result<(), UiContractFailure> {
    // The segment rules are shared with content-view paths.
    if !teamarness_package_contract::media_types::is_normalized_relative_path(value)
        || Path::new(value)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(failure());
    }
    Ok(())
}

fn copy_file(root: &Path, file: &VerifiedFile) -> Result<(), UiContractFailure> {
    let target = root.join(&file.relative_path);
    let parent = target.parent().ok_or_else(failure)?;
    fs::create_dir_all(parent).map_err(|_| failure())?;
    fs::write(target, &file.bytes).map_err(|_| failure())
}

fn verify_cache_root(root: &Path) -> Result<(), UiContractFailure> {
    let metadata = fs::symlink_metadata(root).map_err(|_| failure())?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.permissions().mode() & 0o777 != 0o500
    {
        return Err(failure());
    }
    Ok(())
}

fn verify_cached_file(root: &Path, file: &VerifiedFile) -> Result<(), UiContractFailure> {
    let mut path = root.to_owned();
    let components: Vec<_> = Path::new(&file.relative_path).components().collect();
    for (index, component) in components.iter().enumerate() {
        path.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&path).map_err(|_| failure())?;
        let is_file = index + 1 == components.len();
        if metadata.file_type().is_symlink()
            || (is_file && !metadata.is_file())
            || (!is_file && !metadata.is_dir())
            || metadata.permissions().mode() & 0o777 != if is_file { 0o400 } else { 0o500 }
        {
            return Err(failure());
        }
    }
    let metadata = fs::metadata(&path).map_err(|_| failure())?;
    if metadata.len() != file.bytes.len() as u64 {
        return Err(failure());
    }
    let bytes = fs::read(path).map_err(|_| failure())?;
    if sha256(&bytes) != file.digest {
        return Err(failure());
    }
    Ok(())
}

fn make_tree_read_only(root: &Path) -> Result<(), UiContractFailure> {
    for entry in walk_files(root)? {
        set_mode(&entry, if entry.is_dir() { 0o500 } else { 0o400 })?;
    }
    set_mode(root, 0o500)
}

fn walk_files(root: &Path) -> Result<Vec<PathBuf>, UiContractFailure> {
    let mut pending = vec![root.to_owned()];
    let mut entries = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).map_err(|_| failure())? {
            let path = entry.map_err(|_| failure())?.path();
            if path.is_dir() {
                pending.push(path.clone());
            }
            entries.push(path);
        }
    }
    entries.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    Ok(entries)
}

fn set_mode(path: &Path, mode: u32) -> Result<(), UiContractFailure> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|_| failure())
}

fn failure() -> UiContractFailure {
    UiContractFailure {
        code: PackageErrorCode::UiContractViolation,
    }
}
