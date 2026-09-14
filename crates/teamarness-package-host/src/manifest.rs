use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use crate::{PackageErrorCode, PackageIdentity};
use semver::Version;
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};

const MANIFEST_NAME: &str = "teamarness.package.json";
const MAX_MANIFEST_BYTES: usize = 256 * 1024;
const MAX_UI_ENTRYPOINT_BYTES: usize = 8 * 1024 * 1024;
const MAX_UI_ASSET_BYTES: usize = 8 * 1024 * 1024;
const MAX_UI_ASSETS: usize = 64;
const MAX_UI_CONTRACT_BYTES: usize = 1024 * 1024;
const MAX_UI_TOTAL_BYTES: usize = 32 * 1024 * 1024;
const PROTOCOL_MAJOR: u16 = 1;
const PROTOCOL_MINOR: u16 = 0;

#[derive(Debug)]
pub(crate) struct ManifestFailure {
    pub(crate) code: PackageErrorCode,
}

impl ManifestFailure {
    const fn new(code: PackageErrorCode) -> Self {
        Self { code }
    }
}

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

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct VersionRange {
    pub(crate) major: u16,
    pub(crate) min_minor: u16,
    pub(crate) max_minor: u16,
}

#[derive(Debug, Clone)]
pub(crate) struct VerifiedFile {
    pub(crate) relative_path: String,
    pub(crate) bytes: Vec<u8>,
    pub(crate) digest: String,
}

#[derive(Debug, Clone)]
pub(crate) struct LoadedUi {
    pub(crate) entrypoint: VerifiedFile,
    pub(crate) assets: Vec<LoadedAsset>,
    pub(crate) contract: VerifiedFile,
}

#[derive(Debug, Clone)]
pub(crate) struct LoadedAsset {
    pub(crate) file: VerifiedFile,
    pub(crate) media_type: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CapabilityRequirement {
    pub(crate) identity: String,
    pub(crate) major: u16,
    pub(crate) min_minor: u16,
    pub(crate) max_minor: u16,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManifestDocument {
    schema_version: u16,
    identity: String,
    version: String,
    presentation: Presentation,
    protocol: VersionRange,
    backends: Vec<FileArtifact>,
    ui: UiDocument,
    #[serde(default)]
    capabilities: Vec<CapabilityRequirement>,
    #[serde(default)]
    extensions: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Presentation {
    display_name: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileArtifact {
    target: String,
    path: String,
    size: u64,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UiDocument {
    entrypoint: SizedDigestFile,
    #[serde(default)]
    assets: Vec<AssetDocument>,
    contract: DigestFile,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SizedDigestFile {
    path: String,
    size: u64,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AssetDocument {
    path: String,
    media_type: String,
    size: u64,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DigestFile {
    path: String,
    sha256: String,
}

pub(crate) fn load(package_root: &Path, target: &str) -> Result<LoadedManifest, ManifestFailure> {
    let bytes = read_regular_file(
        package_root,
        MANIFEST_NAME,
        PackageErrorCode::ManifestMalformed,
    )?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ManifestFailure::new(PackageErrorCode::ManifestMalformed));
    }
    let value = strict_json(&bytes)
        .map_err(|_| ManifestFailure::new(PackageErrorCode::ManifestMalformed))?;
    let document: ManifestDocument = serde_json::from_value(value)
        .map_err(|_| ManifestFailure::new(PackageErrorCode::ManifestMalformed))?;
    if document.schema_version != 1 {
        return Err(ManifestFailure::new(PackageErrorCode::ManifestUnsupported));
    }
    let identity = PackageIdentity::from_str(&document.identity)
        .map_err(|_| ManifestFailure::new(PackageErrorCode::InvalidPackageIdentity))?;
    let version = Version::parse(&document.version)
        .map_err(|_| ManifestFailure::new(PackageErrorCode::ManifestUnsupported))?;
    if version.to_string() != document.version {
        return Err(ManifestFailure::new(PackageErrorCode::ManifestUnsupported));
    }
    validate_display_name(&document.presentation.display_name)?;
    validate_range(document.protocol)?;
    if document.protocol.major != PROTOCOL_MAJOR
        || !(document.protocol.min_minor..=document.protocol.max_minor).contains(&PROTOCOL_MINOR)
    {
        return Err(ManifestFailure::new(PackageErrorCode::ManifestUnsupported));
    }
    validate_extensions(&document.extensions)?;
    validate_capabilities(&document.capabilities)?;
    if document.ui.entrypoint.size > MAX_UI_ENTRYPOINT_BYTES as u64
        || document.ui.assets.len() > MAX_UI_ASSETS
        || document
            .ui
            .assets
            .iter()
            .any(|asset| asset.size > MAX_UI_ASSET_BYTES as u64)
    {
        return Err(ManifestFailure::new(PackageErrorCode::ManifestMalformed));
    }

    let matching: Vec<_> = document
        .backends
        .iter()
        .filter(|artifact| artifact.target == target)
        .collect();
    if matching.len() != 1 {
        return Err(ManifestFailure::new(PackageErrorCode::TargetUnsupported));
    }
    let mut seen_paths = BTreeSet::new();
    for artifact in &document.backends {
        if artifact.target.is_empty() || !seen_paths.insert(artifact.path.clone()) {
            return Err(ManifestFailure::new(PackageErrorCode::ManifestMalformed));
        }
        normalized_relative_path(&artifact.path)?;
        validate_digest(&artifact.sha256)?;
    }
    let backend = verify_sized_file(
        package_root,
        matching[0],
        PackageErrorCode::ArtifactMissing,
        usize::MAX,
    )?;
    let entrypoint = verify_sized_file(
        package_root,
        &FileArtifact {
            target: String::new(),
            path: document.ui.entrypoint.path.clone(),
            size: document.ui.entrypoint.size,
            sha256: document.ui.entrypoint.sha256.clone(),
        },
        PackageErrorCode::UiEntrypointMissing,
        MAX_UI_ENTRYPOINT_BYTES,
    )?;
    if entrypoint.bytes.len() > MAX_UI_ENTRYPOINT_BYTES
        || std::str::from_utf8(&entrypoint.bytes).is_err()
        || document.ui.assets.len() > MAX_UI_ASSETS
        || !seen_paths.insert(document.ui.entrypoint.path.clone())
        || !seen_paths.insert(document.ui.contract.path.clone())
    {
        return Err(ManifestFailure::new(PackageErrorCode::ManifestMalformed));
    }
    let mut ui_total = entrypoint.bytes.len();
    let mut assets = Vec::with_capacity(document.ui.assets.len());
    for asset in document.ui.assets {
        if !seen_paths.insert(asset.path.clone())
            || mime::Mime::from_str(&asset.media_type).is_err()
        {
            return Err(ManifestFailure::new(PackageErrorCode::ManifestMalformed));
        }
        let file = verify_sized_file(
            package_root,
            &FileArtifact {
                target: String::new(),
                path: asset.path,
                size: asset.size,
                sha256: asset.sha256,
            },
            PackageErrorCode::UiEntrypointMissing,
            MAX_UI_ASSET_BYTES,
        )?;
        if file.bytes.len() > MAX_UI_ASSET_BYTES {
            return Err(ManifestFailure::new(PackageErrorCode::ManifestMalformed));
        }
        ui_total = ui_total
            .checked_add(file.bytes.len())
            .ok_or_else(|| ManifestFailure::new(PackageErrorCode::ManifestMalformed))?;
        assets.push(LoadedAsset {
            file,
            media_type: asset.media_type,
        });
    }
    let contract = verify_digest_file(
        package_root,
        &document.ui.contract,
        PackageErrorCode::UiContractViolation,
    )?;
    ui_total = ui_total
        .checked_add(contract.bytes.len())
        .ok_or_else(|| ManifestFailure::new(PackageErrorCode::ManifestMalformed))?;
    if contract.bytes.len() > MAX_UI_CONTRACT_BYTES {
        return Err(ManifestFailure::new(PackageErrorCode::UiContractViolation));
    }
    if ui_total > MAX_UI_TOTAL_BYTES {
        return Err(ManifestFailure::new(PackageErrorCode::ManifestMalformed));
    }

    Ok(LoadedManifest {
        identity,
        version,
        display_name: document.presentation.display_name,
        protocol: document.protocol,
        backend,
        ui: LoadedUi {
            entrypoint,
            assets,
            contract,
        },
        capabilities: document.capabilities,
        manifest_digest: sha256(&bytes),
    })
}

fn validate_display_name(value: &str) -> Result<(), ManifestFailure> {
    if value.trim() != value
        || value.is_empty()
        || value.len() > 128
        || value.chars().any(char::is_control)
    {
        return Err(ManifestFailure::new(PackageErrorCode::ManifestMalformed));
    }
    Ok(())
}

fn validate_range(range: VersionRange) -> Result<(), ManifestFailure> {
    if range.major == 0 || range.min_minor > range.max_minor {
        return Err(ManifestFailure::new(PackageErrorCode::ManifestUnsupported));
    }
    Ok(())
}

fn validate_capabilities(values: &[CapabilityRequirement]) -> Result<(), ManifestFailure> {
    let mut identities = BTreeSet::new();
    for value in values {
        PackageIdentity::from_str(&value.identity)
            .map_err(|_| ManifestFailure::new(PackageErrorCode::ManifestMalformed))?;
        validate_range(VersionRange {
            major: value.major,
            min_minor: value.min_minor,
            max_minor: value.max_minor,
        })?;
        if !identities.insert(&value.identity) {
            return Err(ManifestFailure::new(PackageErrorCode::ManifestMalformed));
        }
    }
    Ok(())
}

fn validate_extensions(values: &BTreeMap<String, Value>) -> Result<(), ManifestFailure> {
    if values
        .keys()
        .any(|key| PackageIdentity::from_str(key).is_err())
    {
        return Err(ManifestFailure::new(PackageErrorCode::ManifestMalformed));
    }
    Ok(())
}

fn verify_sized_file(
    root: &Path,
    artifact: &FileArtifact,
    missing_code: PackageErrorCode,
    maximum_bytes: usize,
) -> Result<VerifiedFile, ManifestFailure> {
    validate_digest(&artifact.sha256)?;
    let bytes = read_regular_file_bounded(root, &artifact.path, missing_code, maximum_bytes)?;
    if u64::try_from(bytes.len()).ok() != Some(artifact.size) || sha256(&bytes) != artifact.sha256 {
        return Err(ManifestFailure::new(PackageErrorCode::ArtifactChanged));
    }
    Ok(VerifiedFile {
        relative_path: artifact.path.clone(),
        bytes,
        digest: artifact.sha256.clone(),
    })
}

fn verify_digest_file(
    root: &Path,
    artifact: &DigestFile,
    missing_code: PackageErrorCode,
) -> Result<VerifiedFile, ManifestFailure> {
    validate_digest(&artifact.sha256)?;
    let bytes =
        read_regular_file_bounded(root, &artifact.path, missing_code, MAX_UI_CONTRACT_BYTES)?;
    if sha256(&bytes) != artifact.sha256 {
        return Err(ManifestFailure::new(PackageErrorCode::ArtifactChanged));
    }
    Ok(VerifiedFile {
        relative_path: artifact.path.clone(),
        bytes,
        digest: artifact.sha256.clone(),
    })
}

fn validate_digest(value: &str) -> Result<(), ManifestFailure> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ManifestFailure::new(PackageErrorCode::ManifestMalformed));
    }
    Ok(())
}

fn read_regular_file(
    root: &Path,
    relative: &str,
    missing_code: PackageErrorCode,
) -> Result<Vec<u8>, ManifestFailure> {
    read_regular_file_bounded(root, relative, missing_code, usize::MAX)
}

fn read_regular_file_bounded(
    root: &Path,
    relative: &str,
    missing_code: PackageErrorCode,
    maximum_bytes: usize,
) -> Result<Vec<u8>, ManifestFailure> {
    let relative_path = normalized_relative_path(relative)?;
    let mut path = PathBuf::from(root);
    for component in relative_path.components() {
        let Component::Normal(component) = component else {
            return Err(ManifestFailure::new(PackageErrorCode::ManifestMalformed));
        };
        path.push(component);
        let metadata =
            fs::symlink_metadata(&path).map_err(|_| ManifestFailure::new(missing_code))?;
        if metadata.file_type().is_symlink() {
            return Err(ManifestFailure::new(missing_code));
        }
    }
    let metadata = fs::metadata(&path).map_err(|_| ManifestFailure::new(missing_code))?;
    if !metadata.is_file() {
        return Err(ManifestFailure::new(missing_code));
    }
    if metadata.len() > maximum_bytes as u64 {
        return Err(ManifestFailure::new(missing_code));
    }
    fs::read(path).map_err(|_| ManifestFailure::new(missing_code))
}

fn normalized_relative_path(value: &str) -> Result<PathBuf, ManifestFailure> {
    if value.is_empty()
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains("//")
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || !value.is_ascii()
    {
        return Err(ManifestFailure::new(PackageErrorCode::ManifestMalformed));
    }
    let path = PathBuf::from(value);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ManifestFailure::new(PackageErrorCode::ManifestMalformed));
    }
    Ok(path)
}

pub(crate) fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

pub(crate) fn strict_json(bytes: &[u8]) -> Result<Value, serde_json::Error> {
    serde_json::from_slice::<StrictValue>(bytes).map(|value| value.0)
}

struct StrictValue(Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictValueVisitor)
    }
}

struct StrictValueVisitor;

impl<'de> Visitor<'de> for StrictValueVisitor {
    type Value = StrictValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }
    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Bool(value)))
    }
    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(Number::from(value))))
    }
    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(Number::from(value))))
    }
    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .map(StrictValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }
    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::String(value.to_owned())))
    }
    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::String(value)))
    }
    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }
    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }
    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<StrictValue>()? {
            values.push(value.0);
        }
        Ok(StrictValue(Value::Array(values)))
    }
    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = BTreeMap::new();
        while let Some(key) = map.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(serde::de::Error::custom(format_args!(
                    "duplicate key `{key}`"
                )));
            }
            values.insert(key, map.next_value::<StrictValue>()?.0);
        }
        Ok(StrictValue(Value::Object(Map::from_iter(values))))
    }
}
