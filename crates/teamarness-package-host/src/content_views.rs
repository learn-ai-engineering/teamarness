//! `teamarness.content-views` 1.0: the Host-wide table of open content views, and the
//! `ReadContent` command that serves their files.
//!
//! A Package states *what* to serve: an entry path and a list of files, each with a content
//! path, a SHA-256, a size and a source inside its own Package Data Directory. The Host owns
//! everything else — path and extension rules, source resolution, verification, media types,
//! token lifetime and bounds — so a Package cannot make the Host serve a byte it did not list,
//! from a place outside its directory, or under a media type the allowlist does not give.
//!
//! A view belongs to one Package Instance generation. At most
//! [`MAX_VIEWS_PER_GENERATION`] are open per generation: opening one more closes the least
//! recently opened. Every view of an Instance closes when its generation ends, and a read also
//! refuses a view whose generation is no longer the Instance's live one.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use teamarness_package_contract::media_types::{
    MAX_CONTENT_FILE_BYTES, MAX_CONTENT_FILES, MAX_CONTENT_TOTAL_BYTES,
    is_normalized_relative_path, is_valid_content_path, media_type_for_path,
};

use crate::errors::{PackageErrorEnvelope, PackageErrorStage, host_error};
use crate::manifest::sha256;
use crate::{
    ContentViewToken, PackageErrorCode, PackageHost, PackageHostOutcome, PackageInstanceKey,
    ReadContent,
};

/// The capability family whose declaration also opts a Package's UI mounts into nested content.
pub(crate) const FAMILY: &str = "teamarness.content-views";
/// The most views one Package Instance generation holds open at once.
pub(crate) const MAX_VIEWS_PER_GENERATION: usize = 32;
/// The longest `source.path`, in bytes. A Package names its own storage layout, so this only
/// keeps one `open` request well inside a process frame: 1,000 files at every bound is about
/// 540 KB of JSON.
pub(crate) const MAX_SOURCE_PATH_BYTES: usize = 256;
/// The scheme and host every view URL starts with.
const URL_PREFIX: &str = "teamarness-content://localhost/";

/// The Package Instance generation a view belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ViewOwner {
    pub(crate) instance: PackageInstanceKey,
    pub(crate) generation: u64,
}

/// One file of an `open` request, after its shape, path, extension and size were checked.
#[derive(Clone, Debug)]
pub(crate) struct RequestedFile {
    path: String,
    sha256: String,
    size: u64,
    source: String,
    media_type: &'static str,
}

/// A validated `open` request.
#[derive(Debug)]
pub(crate) struct OpenRequest {
    entry: String,
    files: Vec<RequestedFile>,
}

#[derive(Clone)]
struct ListedFile {
    source: String,
    sha256: String,
    size: u64,
    media_type: &'static str,
    /// The cached verification verdict, set by the first read.
    verified: Option<bool>,
}

struct View {
    owner: ViewOwner,
    /// The open order across the table; the smallest of an owner's views is evicted first.
    opened: u64,
    root: PathBuf,
    files: BTreeMap<String, ListedFile>,
}

#[derive(Default)]
struct Table {
    views: BTreeMap<ContentViewToken, View>,
    next_opened: u64,
}

/// The Host-wide table of open content views.
#[derive(Default)]
pub(crate) struct ContentViews {
    table: Mutex<Table>,
}

/// Everything one read needs, copied out of the table so no lock is held across file work.
pub(crate) struct ReadTarget {
    owner: ViewOwner,
    root: PathBuf,
    file: ListedFile,
}

impl ContentViews {
    /// Resolves every source under `root` and opens a view for `owner`.
    ///
    /// Blocking: it inspects the file system.
    pub(crate) fn open(
        &self,
        owner: ViewOwner,
        root: &Path,
        request: OpenRequest,
    ) -> Result<Value, PackageErrorCode> {
        for file in &request.files {
            resolve_source(root, &file.source)?;
        }
        let files = request
            .files
            .into_iter()
            .map(|file| {
                (
                    file.path,
                    ListedFile {
                        source: file.source,
                        sha256: file.sha256,
                        size: file.size,
                        media_type: file.media_type,
                        verified: None,
                    },
                )
            })
            .collect();
        let mut table = self.table.lock().unwrap();
        let token = loop {
            let token = random_token()?;
            if !table.views.contains_key(&token) {
                break token;
            }
        };
        let owned = table
            .views
            .values()
            .filter(|view| view.owner == owner)
            .count();
        if owned >= MAX_VIEWS_PER_GENERATION {
            let oldest = table
                .views
                .iter()
                .filter(|(_, view)| view.owner == owner)
                .min_by_key(|(_, view)| view.opened)
                .map(|(token, _)| token.clone());
            if let Some(oldest) = oldest {
                table.views.remove(&oldest);
            }
        }
        let opened = table.next_opened;
        table.next_opened += 1;
        table.views.insert(
            token.clone(),
            View {
                owner,
                opened,
                root: root.to_owned(),
                files,
            },
        );
        Ok(json!({
            "view": token.as_str(),
            "url": content_url(&token, &request.entry),
        }))
    }

    /// Closes one of `owner`'s views. Returns whether it was open.
    pub(crate) fn close(&self, owner: &ViewOwner, token: &ContentViewToken) -> bool {
        let mut table = self.table.lock().unwrap();
        if table
            .views
            .get(token)
            .is_some_and(|view| view.owner == *owner)
        {
            table.views.remove(token);
            true
        } else {
            false
        }
    }

    /// Closes every view of one Package Instance: its generation has ended.
    pub(crate) fn close_instance(&self, instance: &PackageInstanceKey) {
        self.table
            .lock()
            .unwrap()
            .views
            .retain(|_, view| view.owner.instance != *instance);
    }

    /// Closes every view of one ended generation.
    fn close_owner(&self, owner: &ViewOwner) {
        self.table
            .lock()
            .unwrap()
            .views
            .retain(|_, view| view.owner != *owner);
    }

    fn target(&self, token: &ContentViewToken, path: &str) -> Option<ReadTarget> {
        let table = self.table.lock().unwrap();
        let view = table.views.get(token)?;
        let file = view.files.get(path)?;
        Some(ReadTarget {
            owner: view.owner.clone(),
            root: view.root.clone(),
            file: file.clone(),
        })
    }

    fn record_verdict(&self, token: &ContentViewToken, path: &str, verdict: bool) {
        if let Some(file) = self
            .table
            .lock()
            .unwrap()
            .views
            .get_mut(token)
            .and_then(|view| view.files.get_mut(path))
        {
            file.verified = Some(verdict);
        }
    }

    #[cfg(test)]
    fn open_count(&self, owner: &ViewOwner) -> usize {
        self.table
            .lock()
            .unwrap()
            .views
            .values()
            .filter(|view| view.owner == *owner)
            .count()
    }
}

impl ReadTarget {
    /// Reads the file, checking its type and size on every read and its SHA-256 on the first.
    ///
    /// Blocking. `None` is every failure: the caller answers not-found without detail.
    fn read(&self) -> Option<Vec<u8>> {
        if self.file.verified == Some(false) {
            return None;
        }
        let path = resolve_source(&self.root, &self.file.source).ok()?;
        if fs::symlink_metadata(&path).ok()?.len() != self.file.size {
            return None;
        }
        let bytes = fs::read(&path).ok()?;
        if bytes.len() as u64 != self.file.size {
            return None;
        }
        if self.file.verified.is_none() && sha256(&bytes) != self.file.sha256 {
            return None;
        }
        Some(bytes)
    }
}

/// Validates one `open` request without touching the file system.
pub(crate) fn parse_open(object: &Map<String, Value>) -> Result<OpenRequest, PackageErrorCode> {
    exact_fields(object, &["method", "entry", "files"])?;
    let entry = object
        .get("entry")
        .and_then(Value::as_str)
        .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
    let values = object
        .get("files")
        .and_then(Value::as_array)
        .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
    if values.is_empty() {
        return Err(PackageErrorCode::CapabilityRequestInvalid);
    }
    if values.len() > MAX_CONTENT_FILES {
        return Err(PackageErrorCode::ContentTooLarge);
    }
    let mut files = Vec::with_capacity(values.len());
    let mut paths = std::collections::BTreeSet::new();
    let mut total = 0_u64;
    for value in values {
        let file = parse_file(value)?;
        if !paths.insert(file.path.clone()) {
            return Err(PackageErrorCode::InvalidContentPath);
        }
        total += file.size;
        if total > MAX_CONTENT_TOTAL_BYTES {
            return Err(PackageErrorCode::ContentTooLarge);
        }
        files.push(file);
    }
    if !paths.contains(entry) {
        return Err(PackageErrorCode::InvalidContentPath);
    }
    Ok(OpenRequest {
        entry: entry.to_owned(),
        files,
    })
}

fn parse_file(value: &Value) -> Result<RequestedFile, PackageErrorCode> {
    let object = value
        .as_object()
        .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
    exact_fields(object, &["path", "sha256", "size", "source"])?;
    let text = |name: &str| {
        object
            .get(name)
            .and_then(Value::as_str)
            .ok_or(PackageErrorCode::CapabilityRequestInvalid)
    };
    let path = text("path")?;
    let digest = text("sha256")?;
    let size = object
        .get("size")
        .and_then(Value::as_u64)
        .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
    let source = object
        .get("source")
        .and_then(Value::as_object)
        .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
    exact_fields(source, &["kind", "path"])?;
    if source.get("kind").and_then(Value::as_str) != Some("file") {
        return Err(PackageErrorCode::CapabilityRequestInvalid);
    }
    let source = source
        .get("path")
        .and_then(Value::as_str)
        .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(PackageErrorCode::CapabilityRequestInvalid);
    }
    if !is_valid_content_path(path) {
        return Err(PackageErrorCode::InvalidContentPath);
    }
    let media_type = media_type_for_path(path).ok_or(PackageErrorCode::UnsupportedMediaType)?;
    if size > MAX_CONTENT_FILE_BYTES {
        return Err(PackageErrorCode::ContentTooLarge);
    }
    if source.len() > MAX_SOURCE_PATH_BYTES
        || !is_normalized_relative_path(source)
        || source.chars().any(char::is_control)
    {
        return Err(PackageErrorCode::InvalidContentPath);
    }
    Ok(RequestedFile {
        path: path.to_owned(),
        sha256: digest.to_owned(),
        size,
        source: source.to_owned(),
        media_type,
    })
}

/// Resolves a validated source path under `root`, refusing a symlink anywhere on the way, a
/// missing component, and a last component that is not a regular file.
fn resolve_source(root: &Path, source: &str) -> Result<PathBuf, PackageErrorCode> {
    let refused = PackageErrorCode::InvalidContentPath;
    let segments: Vec<&str> = source.split('/').collect();
    let mut path = root.to_owned();
    for (index, segment) in segments.iter().enumerate() {
        path.push(segment);
        let metadata = fs::symlink_metadata(&path).map_err(|_| refused)?;
        let last = index + 1 == segments.len();
        if metadata.file_type().is_symlink()
            || (last && !metadata.is_file())
            || (!last && !metadata.is_dir())
        {
            return Err(refused);
        }
    }
    Ok(path)
}

fn exact_fields(object: &Map<String, Value>, expected: &[&str]) -> Result<(), PackageErrorCode> {
    if object.len() == expected.len() && object.keys().all(|key| expected.contains(&key.as_str())) {
        Ok(())
    } else {
        Err(PackageErrorCode::CapabilityRequestInvalid)
    }
}

fn random_token() -> Result<ContentViewToken, PackageErrorCode> {
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).map_err(|_| PackageErrorCode::CapabilityUnavailable)?;
    let text = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    ContentViewToken::new(text).map_err(|_| PackageErrorCode::CapabilityUnavailable)
}

/// `teamarness-content://localhost/<token>/<entry>`, with every entry byte outside the URL
/// unreserved set and `/` percent-encoded. The Shell decodes the path before `ReadContent`.
fn content_url(token: &ContentViewToken, entry: &str) -> String {
    let mut url = format!("{URL_PREFIX}{}/", token.as_str());
    for byte in entry.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            url.push(char::from(byte));
        } else {
            url.push_str(&format!("%{byte:02X}"));
        }
    }
    url
}

#[allow(clippy::result_large_err)]
impl PackageHost {
    /// Serves one listed file of a live view with its verified bytes and allowlisted media type.
    pub(crate) async fn read_content(
        &self,
        input: ReadContent,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        let not_found = || host_error(PackageErrorCode::ContentNotFound, PackageErrorStage::Ui);
        let views = Arc::clone(&self.inner.calls.content_views);
        let target = views
            .target(&input.token, &input.path)
            .ok_or_else(not_found)?;
        let actor = self
            .inner
            .data
            .lock()
            .await
            .instances
            .get(&target.owner.instance)
            .cloned();
        let live = match actor {
            Some(actor) => actor.live_generation().await == Some(target.owner.generation),
            None => false,
        };
        if !live {
            views.close_owner(&target.owner);
            return Err(not_found());
        }
        let media_type = target.file.media_type;
        let first_read = target.file.verified.is_none();
        // Reading and hashing a content file is blocking file work.
        let bytes = tokio::task::spawn_blocking(move || target.read())
            .await
            .map_err(|_| not_found())?;
        if first_read || bytes.is_none() {
            views.record_verdict(&input.token, &input.path, bytes.is_some());
        }
        Ok(PackageHostOutcome::Content {
            bytes: bytes.ok_or_else(not_found)?,
            media_type: media_type.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(generation: u64) -> ViewOwner {
        ViewOwner {
            instance: PackageInstanceKey::new(
                "dev.teamarness.fixture".parse().unwrap(),
                crate::WorkspaceId::new("workspace").unwrap(),
            ),
            generation,
        }
    }

    fn request(entry: &str, files: &[(&str, &str)]) -> Map<String, Value> {
        let files = files
            .iter()
            .map(|(path, source)| {
                json!({"path":path,"sha256":"0".repeat(64),"size":1,"source":{"kind":"file","path":source}})
            })
            .collect::<Vec<_>>();
        json!({"method":"open","entry":entry,"files":files})
            .as_object()
            .unwrap()
            .clone()
    }

    #[test]
    fn view_urls_percent_encode_the_entry() {
        let token = ContentViewToken::new("0123456789abcdef0123456789abcdef").unwrap();
        assert_eq!(
            content_url(&token, "pages/index.html"),
            "teamarness-content://localhost/0123456789abcdef0123456789abcdef/pages/index.html"
        );
        assert_eq!(
            content_url(&token, "a b/é.html"),
            "teamarness-content://localhost/0123456789abcdef0123456789abcdef/a%20b/%C3%A9.html"
        );
    }

    #[test]
    fn open_requests_are_validated_before_the_file_system() {
        assert!(parse_open(&request("index.html", &[("index.html", "a/b")])).is_ok());
        assert_eq!(
            parse_open(&request("missing.html", &[("index.html", "a")])).unwrap_err(),
            PackageErrorCode::InvalidContentPath
        );
        assert_eq!(
            parse_open(&request(
                "index.html",
                &[("index.html", "a"), ("index.html", "b")]
            ))
            .unwrap_err(),
            PackageErrorCode::InvalidContentPath
        );
        assert_eq!(
            parse_open(&request("index.html", &[("index.html", "../escape")])).unwrap_err(),
            PackageErrorCode::InvalidContentPath
        );
        assert_eq!(
            parse_open(&request("index.html", &[("index.html", "/absolute")])).unwrap_err(),
            PackageErrorCode::InvalidContentPath
        );
        assert_eq!(
            parse_open(&request("index.exe", &[("index.exe", "a")])).unwrap_err(),
            PackageErrorCode::UnsupportedMediaType
        );
        let mut extra = request("index.html", &[("index.html", "a")]);
        extra.insert("package".into(), json!("dev.teamarness.other"));
        assert_eq!(
            parse_open(&extra).unwrap_err(),
            PackageErrorCode::CapabilityRequestInvalid
        );
        let empty = json!({"method":"open","entry":"index.html","files":[]});
        assert_eq!(
            parse_open(empty.as_object().unwrap()).unwrap_err(),
            PackageErrorCode::CapabilityRequestInvalid
        );
        let long_source = "s".repeat(MAX_SOURCE_PATH_BYTES + 1);
        assert_eq!(
            parse_open(&request("index.html", &[("index.html", &long_source)])).unwrap_err(),
            PackageErrorCode::InvalidContentPath
        );
    }

    #[test]
    fn the_least_recently_opened_view_of_a_generation_is_evicted() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("a"), b"a").unwrap();
        let views = ContentViews::default();
        let open = |owner: ViewOwner| {
            let value = views
                .open(
                    owner,
                    root.path(),
                    parse_open(&request("index.html", &[("index.html", "a")])).unwrap(),
                )
                .unwrap();
            ContentViewToken::new(value["view"].as_str().unwrap()).unwrap()
        };
        let first = open(owner(1));
        let others: Vec<_> = (1..MAX_VIEWS_PER_GENERATION)
            .map(|_| open(owner(1)))
            .collect();
        let foreign = open(owner(2));
        assert_eq!(views.open_count(&owner(1)), MAX_VIEWS_PER_GENERATION);
        let newest = open(owner(1));
        assert_eq!(views.open_count(&owner(1)), MAX_VIEWS_PER_GENERATION);
        assert!(views.target(&first, "index.html").is_none());
        assert!(views.target(&others[0], "index.html").is_some());
        assert!(views.target(&newest, "index.html").is_some());
        assert!(views.target(&foreign, "index.html").is_some());
        assert!(
            !views.close(&owner(2), &newest),
            "another generation's view"
        );
        assert!(views.close(&owner(1), &newest));
        assert!(!views.close(&owner(1), &newest), "already closed");
        views.close_instance(&owner(1).instance);
        assert_eq!(views.open_count(&owner(1)), 0);
        assert_eq!(views.open_count(&owner(2)), 0);
    }
}
