//! SPIKE (filesystem-docs prototype) — throwaway, never merge as-is.
//!
//! In-Backend folder sync: maps every non-archived Collection to `<root>/<collection name>/`
//! and each of its Documents to `<title>.md`. External edits enter through the same local write
//! path as the Person's `document.revise` ([`stage_local_write`] + [`drain_local_writes`]); new
//! files become `document.create` + a revise; a missing file archives its Document. Remote and
//! in-app changes are written back with the splice writer and the "refuse if the file changed
//! since we last wrote it" rule from [`crate::spike_folder_sync`].
//!
//! Configuration: `<data dir>/PROTOTYPE-folder-map.json` = `{"root": "/abs/path"}`.
//! State: `<data dir>/PROTOTYPE-folder-sync.json` (no migration of documents.sqlite3).
//! Log: `<data dir>/PROTOTYPE-folder-sync.log`, one line per action, for the system test.
//!
//! Simplifications (spike): every trigger runs a full reconcile of every mapped Document; sync
//! store and filesystem calls run on the async task rather than the blocking pool.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use teamarness_package_sdk::capabilities::record_log::RecordLog;
use teamarness_package_sdk::{BackendError, CancellationToken};
use tokio::sync::{broadcast, mpsc, watch};

use super::ports::{decode_version, encode_version};
use super::{
    BodyHub, LocalWrite, Session, drain_local_writes, error, import_edit_rows, stage_local_write,
    storage_failed,
};
use crate::domain::DomainRecord;
use crate::spike_folder_sync::{Hash, WriteMode, hash, render_text, write_back_text};
use crate::store::{DocumentStore, IngestResult};

const DEBOUNCE: Duration = Duration::from_millis(75);
const IDLE_TICK: Duration = Duration::from_secs(1);
const RETRY_TICK: Duration = Duration::from_millis(100);

#[derive(Clone)]
pub(super) struct FolderSync {
    pub(super) log: RecordLog,
    pub(super) hub: BodyHub,
    pub(super) changes: watch::Sender<u64>,
    pub(super) installation: String,
    pub(super) store: DocumentStore,
    pub(super) dir: PathBuf,
}

#[derive(Deserialize)]
struct Config {
    root: PathBuf,
}

#[derive(Serialize, Deserialize, Default)]
struct MapState {
    #[serde(default)]
    docs: BTreeMap<String, Entry>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Entry {
    path: PathBuf,
    /// Version token (ports.rs `encode_version`) the file's contents describe.
    base: String,
    /// Hex SHA-256 of the bytes last written or imported.
    written: String,
    /// Hash of file contents whose import was refused (not retried until the file changes).
    #[serde(default)]
    refused: Option<String>,
}

fn hex(h: &Hash) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Hash {
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(s.get(i * 2..i * 2 + 2).unwrap_or("00"), 16).unwrap_or(0);
    }
    out
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Filesystem-safe file or folder name.
fn sanitize(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    out = out.trim().to_owned();
    if out.starts_with('.') {
        out.replace_range(0..1, "_");
    }
    if out.is_empty() {
        out = "Untitled".into();
    }
    out
}

impl FolderSync {
    fn log_line(&self, line: &str) {
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("PROTOTYPE-folder-sync.log"))
        {
            let _ = writeln!(file, "{} {line}", now_ms());
        }
    }

    fn config(&self) -> Option<PathBuf> {
        let bytes = fs::read(self.dir.join("PROTOTYPE-folder-map.json")).ok()?;
        let config: Config = serde_json::from_slice(&bytes).ok()?;
        config.root.is_absolute().then_some(config.root)
    }

    fn load(&self) -> MapState {
        fs::read(self.dir.join("PROTOTYPE-folder-sync.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save(&self, state: &MapState) {
        let path = self.dir.join("PROTOTYPE-folder-sync.json");
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, serde_json::to_vec_pretty(state).unwrap_or_default()).is_ok() {
            let _ = fs::rename(tmp, path);
        }
    }

    /// Whether `document`'s body can be read and revised now (ports.rs `body_state`), after one
    /// attempt to open its unopened records (backend.rs `reopen_unopened_edits`).
    async fn available(&self, document: &str) -> bool {
        if self.hub.with_body(document, |body| body.unopened()) > 0
            && let Ok(rows) = self.store.body_records(Some(document))
            && let Ok(unopened) = import_edit_rows(&self.log, &self.hub, &rows).await
        {
            let changed = self.hub.with_body(document, |body| {
                let changed = body.unopened() != unopened;
                body.set_unopened(unopened);
                changed
            });
            if changed {
                self.hub.refresh_identity_index_complete();
                self.hub.publish_status(document);
            }
        }
        if self.store.identity_conflicted(document).unwrap_or(true)
            || self.store.import_failure(document).map_or(true, |f| f.is_some())
        {
            return false;
        }
        self.hub.with_body(document, |body| {
            body.editable() && !body.frontiers().is_empty() && !body.has_pending()
        })
    }

    /// backend.rs `append_as` without a request Context.
    async fn append(&self, record: DomainRecord) -> Result<String, BackendError> {
        let purpose = record.kind();
        let canonical = record.encode();
        let appended = self
            .log
            .append_with_principal(purpose, &canonical, None)
            .await?;
        let hub = self.hub.clone();
        let changes = self.changes.clone();
        let id = appended.record_id.as_str().to_owned();
        let ingested = tokio::task::spawn_blocking(move || {
            hub.ingest_record(
                appended.record_id.as_str(),
                appended.order_key.as_str(),
                &appended.author,
                purpose,
                &appended.signature,
                &canonical,
                &appended.sealed_record,
                None,
                &|| changes.send_modify(|revision| *revision = revision.saturating_add(1)),
            )
        })
        .await
        .map_err(|_| storage_failed())?
        .map_err(|_| storage_failed())?;
        match ingested.result {
            IngestResult::Applied | IngestResult::Duplicate => Ok(id),
            _ => Err(error("documents.rejected", false)),
        }
    }

    /// One external file revision through the Person's local write path. Returns whether a
    /// record was appended.
    async fn revise(&self, document: &str, base: &str, markdown: &str) -> Result<bool, BackendError> {
        let base = decode_version(base).ok_or_else(|| error("documents.base_unavailable", false))?;
        let request_id = format!(
            "{:016x}{:016x}",
            getrandom::u64().map_err(|_| error("documents.edit_failed", true))?,
            getrandom::u64().map_err(|_| error("documents.edit_failed", true))?
        );
        let staged = stage_local_write(
            &self.log,
            &self.hub,
            &self.installation,
            document,
            &request_id,
            LocalWrite::Revision {
                base: &base,
                markdown,
                append_first: false,
            },
        )
        .await?;
        if staged.changed {
            drain_local_writes(&self.log, &self.hub, &self.changes, &self.installation, document)
                .await?;
        }
        Ok(staged.changed)
    }

    fn body(&self, document: &str) -> (String, Option<String>) {
        self.hub
            .with_body(document, |body| (body.text().to_owned(), encode_version(&body.frontiers())))
    }

    /// Brings every mapped file and Document into agreement. Returns `true` when something
    /// must be retried soon (body pending, `NotReady`, file changed during write).
    async fn reconcile(&self, root: &Path, state: &mut MapState) -> bool {
        let started = Instant::now();
        let mut retry = false;
        let mut appended = 0usize;
        let (Ok(collections), Ok(documents)) = (self.store.collections(), self.store.documents())
        else {
            return true;
        };
        // Live Collections and their folders.
        let mut folders: HashMap<String, PathBuf> = HashMap::new();
        let mut folder_owner: HashMap<PathBuf, String> = HashMap::new();
        for collection in collections.iter().filter(|c| !c.archived) {
            let Some(name) = &collection.name else {
                retry = true;
                continue;
            };
            let mut folder = root.join(sanitize(name));
            if folder_owner.contains_key(&folder) {
                folder = root.join(format!("{} ({})", sanitize(name), &collection.id[..8]));
            }
            let _ = fs::create_dir_all(&folder);
            folder_owner.insert(folder.clone(), collection.id.clone());
            folders.insert(collection.id.clone(), folder);
        }
        // Where each live Document belongs.
        let mut desired: BTreeMap<String, PathBuf> = BTreeMap::new();
        let mut taken: BTreeSet<PathBuf> = BTreeSet::new();
        for document in documents.iter().filter(|d| !d.archived) {
            let Some(folder) = folders.get(&document.collection_id) else {
                continue;
            };
            let Some(title) = &document.title else {
                retry = true;
                continue;
            };
            let mut path = folder.join(format!("{}.md", sanitize(title)));
            // A mapped Document keeps the path it has when the name collides.
            if let Some(entry) = state.docs.get(&document.id)
                && entry.path.parent() == Some(folder.as_path())
                && taken.contains(&path)
            {
                path = entry.path.clone();
            }
            if taken.contains(&path) {
                path = folder.join(format!("{} ({}).md", sanitize(title), &document.id[..8]));
            }
            taken.insert(path.clone());
            desired.insert(document.id.clone(), path);
        }
        // Mapped Documents that are no longer live: drop the file if we wrote it last.
        let gone: Vec<String> = state
            .docs
            .keys()
            .filter(|id| !desired.contains_key(*id))
            .cloned()
            .collect();
        for id in gone {
            let entry = state.docs.remove(&id).unwrap();
            match fs::read(&entry.path) {
                Ok(bytes) if hex(&hash(&bytes)) == entry.written => {
                    let _ = fs::remove_file(&entry.path);
                    self.log_line(&format!("unmap-remove {id} {}", entry.path.display()));
                }
                Ok(_) => {
                    // Edited but no longer live: keep the file, keep it out of the creator.
                    self.log_line(&format!("unmap-keep-edited {id} {}", entry.path.display()));
                    state.docs.insert(id, entry);
                }
                Err(_) => self.log_line(&format!("unmap {id}")),
            }
        }
        // Files on disk in live folders.
        let mut on_disk: BTreeSet<PathBuf> = BTreeSet::new();
        for folder in folders.values() {
            if let Ok(entries) = fs::read_dir(folder) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().is_some_and(|x| x == "md") && path.is_file() {
                        on_disk.insert(path);
                    }
                }
            }
        }
        let mapped_paths: BTreeSet<PathBuf> = state.docs.values().map(|e| e.path.clone()).collect();
        let mut missing: Vec<String> = state
            .docs
            .iter()
            .filter(|(id, e)| desired.contains_key(*id) && !e.path.exists())
            .map(|(id, _)| id.clone())
            .collect();
        // Unknown files: a rename (same bytes we last wrote) or a new Document.
        let unmapped_desired: BTreeMap<PathBuf, String> = desired
            .iter()
            .filter(|(id, _)| !state.docs.contains_key(*id))
            .map(|(id, p)| (p.clone(), id.clone()))
            .collect();
        for path in on_disk.iter().filter(|p| !mapped_paths.contains(*p)) {
            if unmapped_desired.contains_key(path) {
                continue; // adopted below
            }
            let Ok(bytes) = fs::read(path) else { continue };
            let digest = hex(&hash(&bytes));
            if let Some(index) = missing
                .iter()
                .position(|id| state.docs[id].written == digest)
            {
                let id = missing.remove(index);
                let title = path.file_stem().unwrap().to_string_lossy().into_owned();
                match DomainRecord::document_rename(&self.installation, &id, &title) {
                    Ok(record) => match self.append(record).await {
                        Ok(_) => {
                            appended += 1;
                            self.log_line(&format!("append rename {id} {title:?}"));
                            let entry = state.docs.get_mut(&id).unwrap();
                            entry.path = path.clone();
                        }
                        Err(cause) => {
                            retry = true;
                            self.log_line(&format!("rename-failed {id} {}", cause.code()));
                        }
                    },
                    Err(_) => self.log_line(&format!("rename-invalid {id} {title:?}")),
                }
                continue;
            }
            let Some(collection) = path.parent().and_then(|f| folder_owner.get(f)).cloned() else {
                continue;
            };
            let title = path.file_stem().unwrap().to_string_lossy().into_owned();
            let genesis = crate::body::genesis();
            let Some(base) = encode_version(&crate::body::genesis_frontiers(&genesis)) else {
                continue;
            };
            let record = match DomainRecord::document_create(&self.installation, &collection, &title, genesis) {
                Ok(record) => record,
                Err(_) => {
                    self.log_line(&format!("create-invalid {}", path.display()));
                    continue;
                }
            };
            match self.append(record).await {
                Ok(id) => {
                    appended += 1;
                    self.log_line(&format!("append create {id} {}", path.display()));
                    // `written` = hash of nothing, so the file imports as a revision of genesis.
                    state.docs.insert(
                        id.clone(),
                        Entry { path: path.clone(), base, written: hex(&hash(b"")), refused: None },
                    );
                    desired.insert(id, path.clone());
                }
                Err(cause) => {
                    retry = true;
                    self.log_line(&format!("create-failed {} {}", path.display(), cause.code()));
                }
            }
        }
        // Missing files: archive.
        for id in missing {
            match DomainRecord::document_archive(&self.installation, &id) {
                Ok(record) => match self.append(record).await {
                    Ok(_) => {
                        appended += 1;
                        self.log_line(&format!("append archive {id}"));
                        state.docs.remove(&id);
                        desired.remove(&id);
                    }
                    Err(cause) => {
                        retry = true;
                        self.log_line(&format!("archive-failed {id} {}", cause.code()));
                    }
                },
                Err(_) => {}
            }
        }
        // Each live Document: write, move, import, write back.
        for (id, path) in &desired {
            if !self.available(id).await {
                retry = true;
                continue;
            }
            if !state.docs.contains_key(id) {
                let (text, version) = self.body(id);
                let Some(version) = version else { continue };
                if let Ok(existing) = fs::read_to_string(path) {
                    // A file is already where this Document belongs: adopt it as a revision of
                    // the current body (the file wins where it differs).
                    self.log_line(&format!("adopt {id} {}", path.display()));
                    let _ = existing;
                    state.docs.insert(
                        id.clone(),
                        Entry { path: path.clone(), base: version, written: hex(&hash(b"")), refused: None },
                    );
                } else {
                    let mut written = hash(b"\0never");
                    if write_back_text(path, &mut written, &text) {
                        self.log_line(&format!("write-initial {id} {}", path.display()));
                        state.docs.insert(
                            id.clone(),
                            Entry { path: path.clone(), base: version, written: hex(&written), refused: None },
                        );
                    }
                    continue;
                }
            }
            let entry = state.docs.get_mut(id).unwrap();
            // Title or Collection renamed in the app: move the file if we wrote it last.
            if entry.path != *path {
                let unchanged = fs::read(&entry.path).is_ok_and(|b| hex(&hash(&b)) == entry.written);
                if unchanged && !path.exists() && fs::rename(&entry.path, path).is_ok() {
                    self.log_line(&format!("move {id} {} -> {}", entry.path.display(), path.display()));
                    entry.path = path.clone();
                }
            }
            let Ok(file_text) = fs::read_to_string(&entry.path) else {
                continue;
            };
            let digest = hex(&hash(file_text.as_bytes()));
            if digest != entry.written {
                if entry.refused.as_deref() == Some(digest.as_str()) {
                    continue;
                }
                let t = Instant::now();
                match self.revise(id, &entry.base, &file_text).await {
                    Ok(changed) => {
                        if changed {
                            appended += 1;
                        }
                        self.log_line(&format!(
                            "import {id} changed={changed} bytes={} took_ms={}",
                            file_text.len(),
                            t.elapsed().as_millis()
                        ));
                        entry.written = digest;
                        entry.refused = None;
                    }
                    Err(cause) if cause.retryable() => {
                        retry = true;
                        self.log_line(&format!("import-retry {id} {}", cause.code()));
                        continue;
                    }
                    Err(cause) => {
                        self.log_line(&format!(
                            "import-refused {id} {} bytes={} path={}",
                            cause.code(),
                            file_text.len(),
                            entry.path.display()
                        ));
                        entry.refused = Some(digest);
                        continue;
                    }
                }
            }
            // Write back when the body moved past what the file describes.
            let (text, version) = self.body(id);
            let Some(version) = version else { continue };
            if version == entry.base && entry.written == hex(&hash(file_text.as_bytes())) {
                continue;
            }
            if crate::markdown::canonical(&file_text).ok().as_deref() == Some(text.as_str()) {
                entry.base = version;
                continue;
            }
            let out = render_text(&text, &file_text, WriteMode::Splice);
            let mut written = unhex(&entry.written);
            if write_back_text(&entry.path, &mut written, &out) {
                self.log_line(&format!("write-back {id} bytes={}", out.len()));
                entry.written = hex(&written);
                entry.base = version;
            } else {
                retry = true;
                self.log_line(&format!("write-back-refused {id} (file changed since last write)"));
            }
        }
        self.save(state);
        if appended > 0 {
            self.log_line(&format!(
                "reconcile appended={appended} took_ms={}",
                started.elapsed().as_millis()
            ));
        }
        retry
    }

    async fn run(self, token: CancellationToken) {
        self.log_line("start");
        let mut events = self.hub.events.subscribe();
        let mut metadata = self.changes.subscribe();
        let (fs_tx, mut fs_rx) = mpsc::unbounded_channel::<()>();
        let mut watcher: Option<(PathBuf, notify::RecommendedWatcher)> = None;
        let mut state = self.load();
        let mut was_qualified = false;
        loop {
            let root = self.config();
            if root.is_none() {
                watcher = None;
            }
            if let Some(root) = &root
                && watcher.as_ref().is_none_or(|(watched, _)| watched != root)
            {
                let _ = fs::create_dir_all(root);
                let tx = fs_tx.clone();
                let made = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                    if let Ok(event) = result
                        && event.paths.iter().any(|p| p.extension().is_some_and(|x| x == "md"))
                    {
                        let _ = tx.send(());
                    }
                });
                match made {
                    Ok(mut made) => match made.watch(root, RecursiveMode::Recursive) {
                        Ok(()) => {
                            self.log_line(&format!("watch {}", root.display()));
                            watcher = Some((root.clone(), made));
                        }
                        Err(cause) => self.log_line(&format!("watch-failed {cause}")),
                    },
                    Err(cause) => self.log_line(&format!("watcher-failed {cause}")),
                }
            }
            let qualified = self.hub.qualified.load(Ordering::Acquire);
            if qualified != was_qualified {
                self.log_line(&format!("qualified={qualified}"));
                was_qualified = qualified;
            }
            let mut retry = false;
            if let Some(root) = &root {
                if qualified {
                    retry = self.reconcile(root, &mut state).await;
                } else {
                    retry = true;
                }
            }
            let tick = if retry { RETRY_TICK } else { IDLE_TICK };
            tokio::select! {
                () = token.cancelled() => {
                    self.log_line("stop");
                    return;
                }
                Some(()) = fs_rx.recv() => {
                    self.log_line("fs-event");
                    tokio::time::sleep(DEBOUNCE).await;
                    while fs_rx.try_recv().is_ok() {}
                }
                event = events.recv() => {
                    match event {
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(n)) => self.log_line(&format!("lagged {n}")),
                        Err(broadcast::error::RecvError::Closed) => return,
                    }
                    loop {
                        match events.try_recv() {
                            Ok(_) | Err(broadcast::error::TryRecvError::Lagged(_)) => {}
                            Err(_) => break,
                        }
                    }
                }
                result = metadata.changed() => {
                    if result.is_err() {
                        return;
                    }
                    metadata.borrow_and_update();
                }
                () = tokio::time::sleep(tick) => {}
            }
        }
    }
}

pub(super) fn start(sync: FolderSync) -> Session {
    let cancellation = CancellationToken::new();
    let token = cancellation.clone();
    let task = tokio::spawn(sync.run(token));
    Session { cancellation, task }
}

pub(super) async fn stop(slot: &std::sync::Mutex<Option<Session>>) {
    let session = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    if let Some(session) = session {
        session.cancellation.cancel();
        let _ = session.task.await;
    }
}
