use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use jsonschema::{Draft, Validator};
use serde_json::Value;

use crate::PackageErrorCode;
use crate::manifest::{LoadedManifest, VerifiedFile, sha256, strict_json};

const MAX_SCHEMA_DEPTH: usize = 64;
const MAX_SCHEMA_NODES: usize = 16_384;

#[derive(Debug)]
pub(crate) struct UiContractFailure {
    pub(crate) code: PackageErrorCode,
}

#[derive(Debug, Clone)]
pub(crate) struct UiCacheRecord {
    pub(crate) admitted_root: PathBuf,
    pub(crate) entrypoint: String,
    pub(crate) assets: Vec<UiAssetRecord>,
    pub(crate) contract_digest: String,
    pub(crate) admission_digest: String,
}

#[derive(Debug, Clone)]
pub(crate) struct UiAssetRecord {
    pub(crate) relative_path: String,
    pub(crate) media_type: String,
    pub(crate) digest: String,
}

pub(crate) fn compile_contract(bytes: &[u8]) -> Result<Validator, UiContractFailure> {
    let schema: Value = strict_json(bytes).map_err(|_| failure())?;
    let mut local_references = BTreeMap::<String, Vec<String>>::new();
    let mut node_count = 0;
    walk(&schema, "#", 0, &mut node_count, &mut local_references)?;
    reject_reference_cycles(&local_references)?;
    jsonschema::options()
        .with_draft(Draft::Draft202012)
        .build(&schema)
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
        let metadata = fs::symlink_metadata(&admitted_root).map_err(|_| failure())?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || metadata.permissions().mode() & 0o777 != 0o555
        {
            return Err(failure());
        }
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
    Ok(UiCacheRecord {
        admitted_root,
        entrypoint: manifest.ui.entrypoint.relative_path.clone(),
        assets: manifest
            .ui
            .assets
            .iter()
            .map(|asset| UiAssetRecord {
                relative_path: asset.file.relative_path.clone(),
                media_type: asset.media_type.clone(),
                digest: asset.file.digest.clone(),
            })
            .collect(),
        contract_digest: manifest.ui.contract.digest.clone(),
        admission_digest: admission_digest.to_owned(),
    })
}

fn copy_file(root: &Path, file: &VerifiedFile) -> Result<(), UiContractFailure> {
    let target = root.join(&file.relative_path);
    let parent = target.parent().ok_or_else(failure)?;
    fs::create_dir_all(parent).map_err(|_| failure())?;
    fs::write(target, &file.bytes).map_err(|_| failure())
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
            || metadata.permissions().mode() & 0o777 != if is_file { 0o444 } else { 0o555 }
        {
            return Err(failure());
        }
    }
    let bytes = fs::read(path).map_err(|_| failure())?;
    if sha256(&bytes) != file.digest {
        return Err(failure());
    }
    Ok(())
}

fn make_tree_read_only(root: &Path) -> Result<(), UiContractFailure> {
    for entry in walk_files(root)? {
        set_mode(&entry, if entry.is_dir() { 0o555 } else { 0o444 })?;
    }
    set_mode(root, 0o555)
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

fn walk(
    value: &Value,
    pointer: &str,
    depth: usize,
    node_count: &mut usize,
    references: &mut BTreeMap<String, Vec<String>>,
) -> Result<(), UiContractFailure> {
    *node_count += 1;
    if depth > MAX_SCHEMA_DEPTH || *node_count > MAX_SCHEMA_NODES {
        return Err(failure());
    }
    match value {
        Value::Object(object) => {
            for forbidden in [
                "$dynamicRef",
                "$dynamicAnchor",
                "$recursiveRef",
                "$recursiveAnchor",
            ] {
                if object.contains_key(forbidden) {
                    return Err(failure());
                }
            }
            if let Some(reference) = object.get("$ref") {
                let reference = reference.as_str().ok_or_else(failure)?;
                if !reference.starts_with("#/") {
                    return Err(failure());
                }
                references
                    .entry(pointer.to_owned())
                    .or_default()
                    .push(reference.to_owned());
            }
            for (key, child) in object {
                let escaped = key.replace('~', "~0").replace('/', "~1");
                walk(
                    child,
                    &format!("{pointer}/{escaped}"),
                    depth + 1,
                    node_count,
                    references,
                )?;
            }
        }
        Value::Array(values) => {
            for (index, child) in values.iter().enumerate() {
                walk(
                    child,
                    &format!("{pointer}/{index}"),
                    depth + 1,
                    node_count,
                    references,
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn reject_reference_cycles(
    references: &BTreeMap<String, Vec<String>>,
) -> Result<(), UiContractFailure> {
    fn visit(
        node: &str,
        references: &BTreeMap<String, Vec<String>>,
        active: &mut BTreeSet<String>,
        complete: &mut BTreeSet<String>,
    ) -> bool {
        if active.contains(node) {
            return false;
        }
        if complete.contains(node) {
            return true;
        }
        active.insert(node.to_owned());
        if let Some(targets) = references.get(node) {
            for target in targets {
                if !visit(target, references, active, complete) {
                    return false;
                }
            }
        }
        active.remove(node);
        complete.insert(node.to_owned());
        true
    }
    let mut active = BTreeSet::new();
    let mut complete = BTreeSet::new();
    for node in references.keys() {
        if !visit(node, references, &mut active, &mut complete) {
            return Err(failure());
        }
    }
    Ok(())
}

fn failure() -> UiContractFailure {
    UiContractFailure {
        code: PackageErrorCode::UiContractViolation,
    }
}
