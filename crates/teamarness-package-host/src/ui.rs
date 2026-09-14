use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use jsonschema::{Draft, Validator};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::manifest::{LoadedManifest, VerifiedFile, sha256, strict_json};
use crate::{PackageErrorCode, PackageValue};

pub(crate) const MAX_UI_PAYLOAD_BYTES: usize = 256 * 1024;
const MAX_CONTRACT_BYTES: usize = 1024 * 1024;
const MAX_SCHEMA_DEPTH: usize = 64;
const MAX_SCHEMA_NODES: usize = 16_384;
const MAX_OPERATIONS_PER_DIRECTION: usize = 128;
const MAX_OPERATIONS: usize = 256;

#[derive(Debug)]
pub(crate) struct UiContractFailure {
    pub(crate) code: PackageErrorCode,
}

pub(crate) struct AdmittedUi {
    pub(crate) cache: UiCacheRecord,
    pub(crate) contract: CompiledUiContract,
}

pub(crate) struct CompiledUiContract {
    requests: BTreeMap<String, DirectionalRequest>,
    subscriptions: BTreeMap<String, DirectionalSubscription>,
}

struct DirectionalRequest {
    request: Validator,
    response: Validator,
}

struct DirectionalSubscription {
    request: Validator,
    event: Validator,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ContractDocument {
    schema_version: u16,
    #[serde(rename = "$defs")]
    definitions: Map<String, Value>,
    requests: BTreeMap<String, RequestDescriptor>,
    subscriptions: BTreeMap<String, SubscriptionDescriptor>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestDescriptor {
    request: Value,
    response: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubscriptionDescriptor {
    request: Value,
    event: Value,
}

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
        self.requests
            .get(operation)
            .ok_or_else(failure)
            .and_then(|entry| validate_payload(&entry.request, payload))
    }

    pub(crate) fn validate_response(
        &self,
        operation: &str,
        payload: &PackageValue,
    ) -> Result<(), UiContractFailure> {
        self.requests
            .get(operation)
            .ok_or_else(failure)
            .and_then(|entry| validate_payload(&entry.response, payload))
    }

    pub(crate) fn validate_subscription(
        &self,
        operation: &str,
        payload: &PackageValue,
    ) -> Result<(), UiContractFailure> {
        self.subscriptions
            .get(operation)
            .ok_or_else(failure)
            .and_then(|entry| validate_payload(&entry.request, payload))
    }

    pub(crate) fn validate_event(
        &self,
        operation: &str,
        payload: &PackageValue,
    ) -> Result<(), UiContractFailure> {
        self.subscriptions
            .get(operation)
            .ok_or_else(failure)
            .and_then(|entry| validate_payload(&entry.event, payload))
    }
}

pub(crate) fn compile_contract(bytes: &[u8]) -> Result<CompiledUiContract, UiContractFailure> {
    if bytes.len() > MAX_CONTRACT_BYTES {
        return Err(failure());
    }
    let value = strict_json(bytes).map_err(|_| failure())?;
    let document: ContractDocument = serde_json::from_value(value).map_err(|_| failure())?;
    if document.schema_version != 1
        || document.requests.len() > MAX_OPERATIONS_PER_DIRECTION
        || document.subscriptions.len() > MAX_OPERATIONS_PER_DIRECTION
        || document.requests.len() + document.subscriptions.len() > MAX_OPERATIONS
    {
        return Err(failure());
    }
    if document
        .requests
        .keys()
        .chain(document.subscriptions.keys())
        .any(|name| !valid_operation_name(name))
    {
        return Err(failure());
    }

    let mut node_count = 0;
    let definition_names: BTreeSet<_> = document.definitions.keys().cloned().collect();
    let mut definition_edges = BTreeMap::new();
    for (name, schema) in &document.definitions {
        let mut references = BTreeSet::new();
        walk_schema(
            schema,
            0,
            &mut node_count,
            &definition_names,
            &mut references,
        )?;
        definition_edges.insert(name.clone(), references);
    }
    for descriptor in document.requests.values() {
        walk_schema(
            &descriptor.request,
            0,
            &mut node_count,
            &definition_names,
            &mut BTreeSet::new(),
        )?;
        walk_schema(
            &descriptor.response,
            0,
            &mut node_count,
            &definition_names,
            &mut BTreeSet::new(),
        )?;
    }
    for descriptor in document.subscriptions.values() {
        walk_schema(
            &descriptor.request,
            0,
            &mut node_count,
            &definition_names,
            &mut BTreeSet::new(),
        )?;
        walk_schema(
            &descriptor.event,
            0,
            &mut node_count,
            &definition_names,
            &mut BTreeSet::new(),
        )?;
    }
    reject_reference_cycles(&definition_edges)?;

    let definitions = Value::Object(document.definitions);
    let requests = document
        .requests
        .into_iter()
        .map(|(name, descriptor)| {
            Ok((
                name,
                DirectionalRequest {
                    request: compile_schema(&definitions, descriptor.request)?,
                    response: compile_schema(&definitions, descriptor.response)?,
                },
            ))
        })
        .collect::<Result<_, UiContractFailure>>()?;
    let subscriptions = document
        .subscriptions
        .into_iter()
        .map(|(name, descriptor)| {
            Ok((
                name,
                DirectionalSubscription {
                    request: compile_schema(&definitions, descriptor.request)?,
                    event: compile_schema(&definitions, descriptor.event)?,
                },
            ))
        })
        .collect::<Result<_, UiContractFailure>>()?;
    Ok(CompiledUiContract {
        requests,
        subscriptions,
    })
}

fn compile_schema(definitions: &Value, schema: Value) -> Result<Validator, UiContractFailure> {
    let wrapped = json!({"$defs": definitions, "allOf": [schema]});
    jsonschema::options()
        .with_draft(Draft::Draft202012)
        .build(&wrapped)
        .map_err(|_| failure())
}

fn valid_operation_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 128 || !name.is_ascii() {
        return false;
    }
    name.split('.').all(|segment| {
        !segment.is_empty()
            && segment.as_bytes()[0].is_ascii_lowercase()
            && segment.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            })
    })
}

fn walk_schema(
    value: &Value,
    depth: usize,
    node_count: &mut usize,
    definitions: &BTreeSet<String>,
    references: &mut BTreeSet<String>,
) -> Result<(), UiContractFailure> {
    *node_count += 1;
    if *node_count > MAX_SCHEMA_NODES
        || (matches!(value, Value::Object(_) | Value::Array(_)) && depth > MAX_SCHEMA_DEPTH)
    {
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
                let target = definition_reference(reference.as_str().ok_or_else(failure)?)?;
                if !definitions.contains(&target) {
                    return Err(failure());
                }
                references.insert(target);
            }
            for child in object.values() {
                walk_schema(child, depth + 1, node_count, definitions, references)?;
            }
        }
        Value::Array(values) => {
            for child in values {
                walk_schema(child, depth + 1, node_count, definitions, references)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn definition_reference(reference: &str) -> Result<String, UiContractFailure> {
    let encoded = reference.strip_prefix("#/$defs/").ok_or_else(failure)?;
    if encoded.is_empty() || encoded.contains('/') {
        return Err(failure());
    }
    let mut decoded = String::new();
    let mut chars = encoded.chars();
    while let Some(character) = chars.next() {
        if character != '~' {
            decoded.push(character);
            continue;
        }
        match chars.next() {
            Some('0') => decoded.push('~'),
            Some('1') => decoded.push('/'),
            _ => return Err(failure()),
        }
    }
    Ok(decoded)
}

fn reject_reference_cycles(
    references: &BTreeMap<String, BTreeSet<String>>,
) -> Result<(), UiContractFailure> {
    fn visit(
        node: &str,
        references: &BTreeMap<String, BTreeSet<String>>,
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
        if references.get(node).is_some_and(|targets| {
            targets
                .iter()
                .any(|target| !visit(target, references, active, complete))
        }) {
            return false;
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

fn validate_payload(
    validator: &Validator,
    payload: &PackageValue,
) -> Result<(), UiContractFailure> {
    let value = serde_json::to_value(payload).map_err(|_| failure())?;
    let bytes = serde_json::to_vec(&value).map_err(|_| failure())?;
    if bytes.len() > MAX_UI_PAYLOAD_BYTES || value_depth(&value, 0)? > MAX_SCHEMA_DEPTH {
        return Err(failure());
    }
    validator.validate(&value).map_err(|_| failure())
}

fn value_depth(value: &Value, depth: usize) -> Result<usize, UiContractFailure> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(failure());
    }
    let child_depth = match value {
        Value::Array(values) => values
            .iter()
            .map(|child| value_depth(child, depth + 1))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .max(),
        Value::Object(values) => values
            .values()
            .map(|child| value_depth(child, depth + 1))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .max(),
        _ => None,
    };
    Ok(child_depth.unwrap_or(depth))
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
    Ok(UiCacheRecord {
        admitted_root,
        entrypoint: manifest.ui.entrypoint.relative_path.clone(),
        resources,
        contract_digest: manifest.ui.contract.digest.clone(),
    })
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
    if value.is_empty()
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains("//")
        || value.contains('\\')
        || value.contains('\0')
        || value.contains('%')
        || value.contains('?')
        || value.contains('#')
        || value
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
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
