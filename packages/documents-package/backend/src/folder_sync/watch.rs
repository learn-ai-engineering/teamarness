//! The folder sync session: one task per activation that drives the reconciler.
//!
//! It waits until the record log replay is qualified (a new request ID is refused before then),
//! reconciles every stored mapping in full, and then reacts to three sources:
//!
//! - **Files.** Each mapped root is watched in its stored flat or recursive mode. An event is only a hint: it marks
//!   its mapping dirty, and the mapping is reconciled in full once a [`DEBOUNCE`] window has
//!   passed. A rescan flag, a watcher error or a new watch also reconciles the mapping in full.
//! - **Bodies.** A body update or status change writes that Document back; a lagged or resynced
//!   body stream reconciles every mapping in full.
//! - **The catalog.** A Collection, title, archive or mapping change found in the store writes
//!   the changed Documents back, and reconciles a Collection whose archive state or mapping
//!   changed in full.
//!
//! Every file and SQLite step runs on the blocking pool, raced against the session's
//! cancellation. A reconcile that finds a body not available yet, or fails, is retried after
//! [`RETRY`]. A mapping whose root cannot be watched (for example, an unmounted drive) is
//! retried on the same tick and reconciled in full once its watch is in place.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::event::{AccessKind, AccessMode, EventKind, MetadataKind, ModifyKind};
use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use teamarness_package_sdk::{BackendError, CancellationToken};
use tokio::sync::{Notify, broadcast, watch};
use tokio::time::Instant;

use super::super::{BodyEvent, BodyHub, recover, storage_failed};
use super::reconcile::{FolderSync, Outcome};
use crate::store::DocumentStore;

/// How long file events gather before their mapping is reconciled.
pub(crate) const DEBOUNCE: Duration = Duration::from_millis(100);
/// How often a reconcile that is not finished yet, or a root that is not watched, is retried.
pub(crate) const RETRY: Duration = Duration::from_secs(1);
/// How often the session checks whether the record log replay is qualified yet.
const READINESS_POLL: Duration = Duration::from_millis(50);

/// Mappings marked by the watchers since the session last looked. `true` asks for an immediate
/// full reconcile (a rescan flag or a watcher error) rather than a debounced one.
#[derive(Default)]
struct Marks {
    dirty: Mutex<HashMap<String, bool>>,
    wake: Notify,
}

impl Marks {
    fn mark(&self, collection: &str, urgent: bool) {
        let mut dirty = recover(&self.dirty);
        let entry = dirty.entry(collection.to_owned()).or_insert(false);
        *entry |= urgent;
        drop(dirty);
        self.wake.notify_one();
    }

    fn take(&self) -> HashMap<String, bool> {
        std::mem::take(&mut *recover(&self.dirty))
    }
}

/// Whether a file system event can mean that a mapped file changed. Opening or reading a file
/// (which the reconciler's own scan does) never does; without this filter, inotify's open events
/// would make every scan schedule the next one.
fn relevant(kind: &EventKind) -> bool {
    match kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_)
        | EventKind::Modify(ModifyKind::Metadata(MetadataKind::AccessTime)) => false,
        _ => true,
    }
}

/// One watched root.
struct Watch {
    root: String,
    recursive: bool,
    /// Dropping the watcher stops it.
    _watcher: RecommendedWatcher,
}

/// Watches `root` in its Installation-local mode; every relevant event marks `collection`.
fn watch_root(
    root: &str,
    recursive: bool,
    collection: &str,
    marks: &Arc<Mutex<Option<Arc<Marks>>>>,
) -> notify::Result<RecommendedWatcher> {
    let collection = collection.to_owned();
    let marks = marks.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        // The session clears the slot when it ends, so a late event goes nowhere.
        let Some(marks) = recover(&marks).clone() else {
            return;
        };
        match event {
            Ok(event) if event.need_rescan() => marks.mark(&collection, true),
            Ok(event) if relevant(&event.kind) => marks.mark(&collection, false),
            Ok(_) => {}
            Err(_) => marks.mark(&collection, true),
        }
    })?;
    watcher.watch(
        &PathBuf::from(root),
        if recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        },
    )?;
    Ok(watcher)
}

/// What the store says about Collections, Documents and mappings, compared between catalog
/// changes to find what folder sync must act on.
#[derive(Default)]
struct Catalog {
    /// Mapped Collection → (root path, recursive).
    mappings: HashMap<String, (String, bool)>,
    /// Collection → archived.
    collections: HashMap<String, bool>,
    /// Document → (Collection, title, archived, folder).
    documents: HashMap<String, (String, Option<String>, bool, String)>,
}

impl Catalog {
    fn read(store: &DocumentStore) -> Result<Self, BackendError> {
        let mappings = store
            .folder_mappings()
            .map_err(|_| storage_failed())?
            .into_iter()
            .map(|mapping| {
                (
                    mapping.collection_id,
                    (mapping.root_path, mapping.recursive),
                )
            })
            .collect();
        let collections = store
            .collections()
            .map_err(|_| storage_failed())?
            .into_iter()
            .map(|collection| (collection.id, collection.archived))
            .collect();
        let documents = store
            .documents()
            .map_err(|_| storage_failed())?
            .into_iter()
            .map(|document| {
                (
                    document.id,
                    (
                        document.collection_id,
                        document.title,
                        document.archived,
                        document.folder,
                    ),
                )
            })
            .collect();
        Ok(Self {
            mappings,
            collections,
            documents,
        })
    }

    /// Collections to reconcile in full, and Documents to write back, after `self` became
    /// `next`.
    fn changes(&self, next: &Self) -> (HashSet<String>, HashSet<String>) {
        let mut folders = HashSet::new();
        for (collection, root) in &next.mappings {
            if self.mappings.get(collection) != Some(root)
                || self.collections.get(collection) != next.collections.get(collection)
            {
                folders.insert(collection.clone());
            }
        }
        let documents = next
            .documents
            .iter()
            .filter(|(id, now)| self.documents.get(*id) != Some(*now))
            .map(|(id, _)| id.clone())
            .collect();
        (folders, documents)
    }
}

/// Work gathered since the last pass.
#[derive(Default)]
struct Pending {
    /// Every mapping is reconciled in full.
    everything: bool,
    /// The store may hold Collection, Document or mapping changes.
    catalog: bool,
    folders: HashSet<String>,
    documents: HashSet<String>,
}

impl Pending {
    fn is_empty(&self) -> bool {
        !self.everything && !self.catalog && self.folders.is_empty() && self.documents.is_empty()
    }

    fn merge(&mut self, other: Self) {
        self.everything |= other.everything;
        self.catalog |= other.catalog;
        self.folders.extend(other.folders);
        self.documents.extend(other.documents);
    }
}

/// The session task. It returns when `cancellation` is cancelled or the activation's channels
/// close.
pub(crate) async fn run(
    sync: FolderSync,
    hub: BodyHub,
    mut events: broadcast::Receiver<BodyEvent>,
    mut revisions: watch::Receiver<u64>,
    cancellation: CancellationToken,
) {
    // Readiness: nothing is reconciled before the replay is qualified.
    while !hub.qualified.load(Ordering::Acquire) {
        tokio::select! {
            () = tokio::time::sleep(READINESS_POLL) => {}
            () = cancellation.cancelled() => return,
        }
    }
    let slot: Arc<Mutex<Option<Arc<Marks>>>> = Arc::new(Mutex::new(Some(Arc::default())));
    let marks = recover(&slot).clone().unwrap_or_default();
    let mut session = Loop {
        sync,
        store: hub.store.clone(),
        cancellation: cancellation.clone(),
        slot: slot.clone(),
        watches: HashMap::new(),
        catalog: Catalog::default(),
    };
    // Changes before this point are covered by the first full reconcile.
    revisions.mark_unchanged();
    events = events.resubscribe();
    let mut pending = Pending {
        everything: true,
        catalog: true,
        ..Pending::default()
    };
    let mut retry = Pending::default();
    let mut due: Option<Instant> = Some(Instant::now());
    let mut retry_due = Instant::now() + RETRY;
    loop {
        tokio::select! {
            () = cancellation.cancelled() => break,
            () = marks.wake.notified() => {
                for (collection, urgent) in marks.take() {
                    pending.folders.insert(collection);
                    if urgent {
                        due = Some(Instant::now());
                    }
                }
                due.get_or_insert_with(|| Instant::now() + DEBOUNCE);
            }
            event = events.recv() => {
                match event {
                    Ok(BodyEvent::Update { document, .. } | BodyEvent::Status { document, .. }) => {
                        pending.documents.insert(document);
                    }
                    Ok(BodyEvent::Resync { .. }) | Err(broadcast::error::RecvError::Lagged(_)) => {
                        pending.everything = true;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
                due.get_or_insert_with(|| Instant::now() + DEBOUNCE);
            }
            changed = revisions.changed() => {
                if changed.is_err() {
                    break;
                }
                pending.catalog = true;
                due.get_or_insert_with(|| Instant::now() + DEBOUNCE);
            }
            () = sleep_until(due), if due.is_some() => {
                due = None;
                let work = std::mem::take(&mut pending);
                if let Some(again) = session.pass(work).await {
                    retry.merge(again);
                }
            }
            () = tokio::time::sleep_until(retry_due) => {
                retry_due = Instant::now() + RETRY;
                let mut work = std::mem::take(&mut retry);
                // A root that could not be watched is tried again; once watched, it is
                // reconciled in full.
                work.folders.extend(session.watch_missing().await);
                if !work.is_empty()
                    && let Some(again) = session.pass(work).await
                {
                    retry.merge(again);
                }
            }
        }
        if cancellation.is_cancelled() {
            break;
        }
    }
    // Late watcher events go nowhere, and the watchers stop off the async workers.
    *recover(&slot) = None;
    let watches: Vec<Watch> = session.watches.drain().map(|(_, watch)| watch).collect();
    let _ = tokio::task::spawn_blocking(move || drop(watches)).await;
}

async fn sleep_until(due: Option<Instant>) {
    match due {
        Some(due) => tokio::time::sleep_until(due).await,
        None => std::future::pending().await,
    }
}

/// The session's state between passes.
struct Loop {
    sync: FolderSync,
    store: DocumentStore,
    cancellation: CancellationToken,
    slot: Arc<Mutex<Option<Arc<Marks>>>>,
    watches: HashMap<String, Watch>,
    catalog: Catalog,
}

impl Loop {
    /// Does `work`, and returns what must be retried, if anything.
    async fn pass(&mut self, mut work: Pending) -> Option<Pending> {
        let mut retry = Pending::default();
        if work.catalog {
            match self.blocking(Catalog::read).await {
                Ok(next) => {
                    let (folders, documents) = self.catalog.changes(&next);
                    work.folders.extend(folders);
                    work.documents.extend(documents);
                    self.catalog = next;
                    work.folders.extend(self.update_watches().await);
                }
                Err(_) => retry.catalog = true,
            }
        }
        let folders: Vec<String> = if work.everything {
            // A full reconcile of every mapping also writes back every mapped Document.
            work.documents.clear();
            self.catalog.mappings.keys().cloned().collect()
        } else {
            work.folders
                .into_iter()
                .filter(|collection| self.catalog.mappings.contains_key(collection))
                .collect()
        };
        for collection in folders {
            if self.cancellation.is_cancelled() {
                return None;
            }
            if !settled(self.sync.reconcile_folder(&collection).await) {
                retry.folders.insert(collection.clone());
            }
            self.drop_vanished_watch(&collection).await;
        }
        for document in work.documents {
            if self.cancellation.is_cancelled() {
                return None;
            }
            if !settled(self.sync.reconcile_document(&document).await) {
                retry.documents.insert(document);
            }
        }
        (!retry.is_empty()).then_some(retry)
    }

    /// Watches every mapped root that is not watched yet, and stops watching roots that are no
    /// longer mapped. Returns the Collections newly watched, which need a full reconcile.
    async fn update_watches(&mut self) -> Vec<String> {
        let mappings = &self.catalog.mappings;
        let stale: Vec<String> = self
            .watches
            .iter()
            .filter(|(collection, watch)| {
                mappings.get(*collection) != Some(&(watch.root.clone(), watch.recursive))
            })
            .map(|(collection, _)| collection.clone())
            .collect();
        let dropped: Vec<Watch> = stale
            .iter()
            .filter_map(|collection| self.watches.remove(collection))
            .collect();
        if !dropped.is_empty() {
            let _ = tokio::task::spawn_blocking(move || drop(dropped)).await;
        }
        self.watch_missing().await
    }

    /// Stops the watch of `collection` when its root is no longer a folder: a watch on a removed
    /// folder never reports again, so the root is watched afresh once it returns.
    async fn drop_vanished_watch(&mut self, collection: &str) {
        let Some(root) = self.watches.get(collection).map(|watch| watch.root.clone()) else {
            return;
        };
        let present = teamarness_package_sdk::blocking(&self.cancellation, move |_| {
            Ok(std::fs::metadata(&root).is_ok_and(|meta| meta.is_dir()))
        })
        .await
        .unwrap_or(true);
        if !present && let Some(watch) = self.watches.remove(collection) {
            let _ = tokio::task::spawn_blocking(move || drop(watch)).await;
        }
    }

    /// Tries to watch every mapped root that has no watch. Returns the Collections now watched.
    async fn watch_missing(&mut self) -> Vec<String> {
        let wanted: Vec<(String, (String, bool))> = self
            .catalog
            .mappings
            .iter()
            .filter(|(collection, _)| !self.watches.contains_key(*collection))
            .map(|(collection, root)| (collection.clone(), root.clone()))
            .collect();
        if wanted.is_empty() {
            return Vec::new();
        }
        let slot = self.slot.clone();
        let made = teamarness_package_sdk::blocking(&self.cancellation, move |_| {
            Ok(wanted
                .into_iter()
                .filter_map(|(collection, (root, recursive))| {
                    if !std::fs::metadata(&root).is_ok_and(|meta| meta.is_dir()) {
                        return None;
                    }
                    let watcher = watch_root(&root, recursive, &collection, &slot).ok()?;
                    Some((
                        collection,
                        Watch {
                            root,
                            recursive,
                            _watcher: watcher,
                        },
                    ))
                })
                .collect::<Vec<_>>())
        })
        .await
        .unwrap_or_default();
        let mut watched = Vec::new();
        for (collection, watch) in made {
            watched.push(collection.clone());
            self.watches.insert(collection, watch);
        }
        watched
    }

    async fn blocking<T, F>(&self, work: F) -> Result<T, BackendError>
    where
        T: Send + 'static,
        F: FnOnce(&DocumentStore) -> Result<T, BackendError> + Send + 'static,
    {
        let store = self.store.clone();
        teamarness_package_sdk::blocking(&self.cancellation, move |_| work(&store)).await
    }
}

/// Whether a reconcile finished: it did not fail and left nothing for later.
fn settled(result: Result<Outcome, BackendError>) -> bool {
    match result {
        Ok(outcome) => outcome.pending == 0,
        // A stopping session cancels what it was doing; that is not a failure.
        Err(cause) if cause.code() == "cancelled" => false,
        Err(cause) => {
            eprintln!(
                "documents folder sync reconcile failure: code={} retryable={}",
                cause.code(),
                cause.retryable()
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, RemoveKind};
    use serde_json::{Value as JsonValue, json};

    use super::super::reconcile::tests::remembering_log;
    use crate::backend::tests::{Session, qualify_empty_history, start};

    /// Waits up to ten seconds, polling: file watchers report within a second, but not within
    /// the shorter waits of the Backend tests.
    async fn eventually(mut ready: impl FnMut() -> bool, what: &str) {
        for _ in 0..500 {
            if ready() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("timed out waiting for {what}");
    }

    fn titled(session: &Session, title: &str) -> bool {
        session
            .handles
            .store
            .documents()
            .unwrap()
            .iter()
            .any(|document| document.title.as_deref() == Some(title) && !document.archived)
    }

    async fn request(session: &mut Session, operation: &str, payload: JsonValue) -> JsonValue {
        session
            .harness
            .request(operation, payload)
            .await
            .unwrap_or_else(|cause| panic!("{operation}: {}", cause.code()))
    }

    // Verifies: TEAM.SYS.DOCS-R14 (Export, Import, Safety) — a mapping set by request
    // exports, the running session imports a new file, a suspended session does nothing, and a
    // resumed one reconciles what changed while it was stopped.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn suspend_stops_folder_sync_and_resume_reconciles() {
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("plans");
        std::fs::create_dir(&root).unwrap();
        let mut session = start(data.path(), remembering_log()).await;
        qualify_empty_history(&session).await;
        let collection = request(
            &mut session,
            "documents.collection.create",
            json!({ "name": "Plans" }),
        )
        .await["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let mapped = request(
            &mut session,
            "documents.mapping.set",
            json!({ "collectionId": collection, "path": root.to_str().unwrap() }),
        )
        .await;
        assert_eq!(mapped["direction"], "export");
        // A Collection with a Document is not mapped to a folder with files.
        request(
            &mut session,
            "documents.document.create",
            json!({ "collectionId": collection, "title": "Plan" }),
        )
        .await;
        eventually(|| root.join("Plan.md").is_file(), "the new Document's file").await;

        std::fs::write(root.join("Notes.md"), "# Notes\n\nWritten outside.\n").unwrap();
        eventually(|| titled(&session, "Notes"), "the watched file's Document").await;

        session.harness.suspend().await.unwrap();
        std::fs::write(
            root.join("Later.md"),
            "# Later\n\nWritten while suspended.\n",
        )
        .unwrap();
        // negative-wait: longer than the debounce and a watcher's report, so a suspended
        // session that still reconciled would have made the Document by now.
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(
            !titled(&session, "Later"),
            "a suspended session reconciles nothing"
        );

        session.harness.recover().await.unwrap();
        // The resumed replay would redeliver every record; the test marks it qualified.
        session
            .handles
            .qualified
            .store(true, std::sync::atomic::Ordering::Release);
        eventually(|| titled(&session, "Later"), "the resumed full reconcile").await;

        session.harness.dispose().await.unwrap();
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.DOCS-R14 (Mapping rules) — a non-empty folder and a non-empty Collection are not mapped.
    #[tokio::test]
    async fn a_full_folder_and_a_full_collection_are_not_mapped() {
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("Plan.md"), "# Plan\n").unwrap();
        let mut session = start(data.path(), remembering_log()).await;
        qualify_empty_history(&session).await;
        let collection = request(
            &mut session,
            "documents.collection.create",
            json!({ "name": "Plans" }),
        )
        .await["id"]
            .as_str()
            .unwrap()
            .to_owned();
        request(
            &mut session,
            "documents.document.create",
            json!({ "collectionId": collection, "title": "Other" }),
        )
        .await;
        let refused = session
            .harness
            .request(
                "documents.mapping.set",
                json!({ "collectionId": collection, "path": folder.path().to_str().unwrap() }),
            )
            .await
            .unwrap_err();
        assert_eq!(refused.code(), "documents.mapping_refused");
        assert_eq!(refused.details()["reason"], "not_empty");
        assert!(session.handles.store.folder_mappings().unwrap().is_empty());
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.DOCS-R14, TEAM.SYS.DOCS-R15; no UI subscription is mounted.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn recursive_watch_reconfigures_and_observes_nested_save_and_placement() {
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        std::fs::create_dir_all(root.join("Nested/deep")).unwrap();
        let mut session = start(data.path(), remembering_log()).await;
        qualify_empty_history(&session).await;
        let collection = request(
            &mut session,
            "documents.collection.create",
            json!({"name":"Plans"}),
        )
        .await["id"]
            .as_str()
            .unwrap()
            .to_owned();
        session
            .handles
            .store
            .set_folder_mapping(&crate::store::mappings::FolderMapping {
                collection_id: collection.clone(),
                root_path: root.to_str().unwrap().to_owned(),
                state: crate::store::mappings::MappingState::Active,
                created_at: 1,
                recursive: false,
            })
            .unwrap();
        session
            .handles
            .changes
            .send_modify(|revision| *revision += 1);
        let document = request(
            &mut session,
            "documents.document.create",
            json!({"collectionId":collection,"title":"Root"}),
        )
        .await["id"]
            .as_str()
            .unwrap()
            .to_owned();
        eventually(|| root.join("Root.md").is_file(), "initial flat watch").await;
        request(
            &mut session,
            "documents.mapping.enable-recursive",
            json!({"collectionId":collection,"dryRun":true}),
        )
        .await;
        request(
            &mut session,
            "documents.mapping.enable-recursive",
            json!({"collectionId":collection}),
        )
        .await;
        request(
            &mut session,
            "documents.document.place",
            json!({"documentId":document,"folder":"Nested/deep"}),
        )
        .await;
        eventually(
            || root.join("Nested/deep/Root.md").is_file(),
            "metadata-only placement",
        )
        .await;
        std::fs::write(root.join("Nested/deep/Save.md"), "Nested save.\n").unwrap();
        eventually(|| titled(&session, "Save"), "nested watcher save").await;
        // Drain debounced self-write hints before a metadata-only operation; otherwise a
        // startup or filesystem pass could accidentally cover missing catalog invalidation.
        tokio::time::sleep(Duration::from_millis(600)).await;
        request(
            &mut session,
            "documents.document.place",
            json!({"documentId":document,"folder":"Other"}),
        )
        .await;
        eventually(
            || root.join("Other/Root.md").is_file(),
            "settled metadata-only placement",
        )
        .await;
        session.harness.suspend().await.unwrap();
        std::fs::write(root.join("Nested/deep/Offline.md"), "Offline.\n").unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(!titled(&session, "Offline"));
        session.harness.recover().await.unwrap();
        session.handles.qualified.store(true, Ordering::Release);
        eventually(|| titled(&session, "Offline"), "recursive resume").await;
        session.harness.shutdown().await.unwrap();
    }

    #[test]
    fn reads_and_opens_are_not_file_changes() {
        assert!(!relevant(&EventKind::Access(AccessKind::Open(
            AccessMode::Any
        ))));
        assert!(!relevant(&EventKind::Access(AccessKind::Close(
            AccessMode::Read
        ))));
        assert!(!relevant(&EventKind::Modify(ModifyKind::Metadata(
            MetadataKind::AccessTime
        ))));
        assert!(relevant(&EventKind::Access(AccessKind::Close(
            AccessMode::Write
        ))));
        assert!(relevant(&EventKind::Create(CreateKind::File)));
        assert!(relevant(&EventKind::Remove(RemoveKind::Any)));
        assert!(relevant(&EventKind::Modify(ModifyKind::Any)));
        assert!(relevant(&EventKind::Any));
    }

    #[test]
    fn catalog_changes_name_what_folder_sync_must_act_on() {
        let before = Catalog {
            mappings: HashMap::from([("c".into(), ("/a".into(), false))]),
            collections: HashMap::from([("c".into(), false), ("d".into(), false)]),
            documents: HashMap::from([
                (
                    "1".into(),
                    ("c".into(), Some("One".into()), false, String::new()),
                ),
                (
                    "2".into(),
                    ("c".into(), Some("Two".into()), false, String::new()),
                ),
            ]),
        };
        let same = Catalog {
            mappings: before.mappings.clone(),
            collections: before.collections.clone(),
            documents: before.documents.clone(),
        };
        let (folders, documents) = before.changes(&same);
        assert!(folders.is_empty() && documents.is_empty());

        let after = Catalog {
            mappings: HashMap::from([
                ("c".into(), ("/a".into(), true)),
                ("d".into(), ("/b".into(), false)),
            ]),
            collections: HashMap::from([("c".into(), true), ("d".into(), false)]),
            documents: HashMap::from([
                (
                    "1".into(),
                    ("c".into(), Some("Renamed".into()), false, String::new()),
                ),
                (
                    "2".into(),
                    ("c".into(), Some("Two".into()), false, String::new()),
                ),
                ("3".into(), ("d".into(), None, false, String::new())),
            ]),
        };
        let (folders, documents) = before.changes(&after);
        assert_eq!(folders, HashSet::from(["c".into(), "d".into()]));
        assert_eq!(documents, HashSet::from(["1".into(), "3".into()]));
    }
}
