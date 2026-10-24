use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use loro::Frontiers;
use serde::Deserialize;
use serde_json::{Value as JsonValue, json};
use sha2::{Digest, Sha256};
use teamarness_package_sdk::capabilities::principals::{PrincipalSnapshot, Principals};
use teamarness_package_sdk::capabilities::record_log::{
    Availability, Cursor, OpenResult, RecordEvent, RecordLog, RecordLogSink, ReplayEnd,
    SessionOptions, SyncState, SyncWarning, run_session_from_start,
};
use teamarness_package_sdk::{
    BackendError, CancellationToken, Context, Empty, EventSubscription, Router, event_channel,
    ports::PortActor,
};
use tokio::sync::{Notify, broadcast, watch};

use crate::body::{BodyError, DocumentBody};
use crate::domain::{DomainError, DomainRecord, MAX_UPDATE};
use crate::store::{DocumentStore, ImportClassifier, IngestResult, Ingested, StoreError};

#[path = "ports.rs"]
mod ports;

/// How many distinct synchronization warning codes are remembered at a time.
const MAX_WARNINGS: usize = 8;
/// How often the flusher retries a staged append that failed.
const FLUSH_RETRY: Duration = Duration::from_millis(100);

fn error(code: &str, retryable: bool) -> BackendError {
    BackendError::new(code, retryable, json!({}))
}
fn storage_failed() -> BackendError {
    error("documents.storage_failed", true)
}
fn invalid_request() -> BackendError {
    error("documents.invalid_request", false)
}

fn capability_error(operation: &str, cause: BackendError) -> BackendError {
    eprintln!(
        "documents capability failure: operation={operation} code={} retryable={}",
        cause.code(),
        cause.retryable()
    );
    BackendError::new(
        "documents.capability_unavailable",
        cause.retryable(),
        json!({ "causeCode": cause.code() }),
    )
}

fn domain_error(cause: DomainError) -> BackendError {
    match cause {
        DomainError::InvalidName => error("documents.invalid_name", false),
        DomainError::InvalidTitle => error("documents.invalid_title", false),
        DomainError::InvalidUpdate | DomainError::InvalidGenesis => {
            error("documents.invalid_update", false)
        }
        DomainError::InvalidMarkdown => error("documents.invalid_markdown", false),
        DomainError::InvalidQuote => error("documents.invalid_quote", false),
        _ => invalid_request(),
    }
}

/// Every Document body this activation holds, keyed by Document identity.
type Bodies = Arc<Mutex<HashMap<String, DocumentBody>>>;

/// Raw bytes of one snapshot chunk: 24 KiB once base64-encoded.
const SNAPSHOT_CHUNK_BYTES: usize = 18 * 1024;
/// UTF-8 bytes of one Canonical Markdown chunk.
const MARKDOWN_CHUNK_BYTES: usize = 24 * 1024;

/// What one body change tells the open editor sessions. Sessions filter by Document.
#[derive(Clone, Debug)]
enum BodyEvent {
    /// The body gained operations: one Loro update, relative to what every session holds.
    /// `origin` is the session that submitted it, which never receives it back.
    Update {
        document: String,
        update: Arc<Vec<u8>>,
        origin: Option<u64>,
    },
    /// Editability may have changed, or a request reached the Core record log.
    Status {
        document: String,
        durable: Option<String>,
    },
    /// Every session of the Document must reload from a new snapshot.
    Resync { document: String },
}

impl BodyEvent {
    fn document(&self) -> &str {
        match self {
            Self::Update { document, .. }
            | Self::Status { document, .. }
            | Self::Resync { document } => document,
        }
    }
}

/// One open editor session: the Document it edits, and how a refused update ends it.
#[derive(Clone)]
struct SessionEntry {
    document: String,
    refused: Arc<Notify>,
}

/// Whether a body can be edited, and why not.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BodyAccess {
    editable: bool,
    reason: Option<&'static str>,
    diagnostic: Option<String>,
}

impl BodyAccess {
    fn write(&self, event: &mut JsonValue) {
        event["editable"] = json!(self.editable);
        if let Some(reason) = self.reason {
            event["reason"] = json!(reason);
        }
        if let Some(record) = &self.diagnostic {
            event["diagnosticRecordId"] = json!(record);
        }
    }
}

/// The shared pieces that import remote edits and publish body events, used by the request
/// handlers, the record-log sink, and the flusher alike.
#[derive(Clone)]
struct BodyHub {
    store: DocumentStore,
    bodies: Bodies,
    /// The Installation this activation writes as; records by anyone else are foreign.
    installation: String,
    /// Body events for every Document; editor sessions filter by Document.
    events: broadcast::Sender<BodyEvent>,
    /// Open editor sessions by session peer.
    sessions: Arc<Mutex<HashMap<u64, SessionEntry>>>,
    /// Session peers whose session ended in this process, with their Document, so the same
    /// UI may resume as one.
    ended_sessions: Arc<Mutex<HashMap<u64, String>>>,
    /// Wakes the flusher when a staged append is pending.
    flush: Arc<Notify>,
    /// Serializes append attempts with requests without holding the body mutex across I/O.
    append_locks: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    waiting_metadata: Arc<Mutex<HashSet<String>>>,
    metadata_lock: Arc<tokio::sync::Mutex<()>>,
    qualified: Arc<AtomicBool>,
    identity_index_complete: Arc<AtomicBool>,
    identity_lookup_lock: Arc<tokio::sync::Mutex<()>>,
    /// Serializes conflict-marker commits with recovery event publication, never across I/O.
    publication_gate: Arc<Mutex<()>>,
}

/// Locks `mutex`, taking it as it is when a panic poisoned it: every std mutex here guards
/// state that each holder leaves consistent, so a panic elsewhere must not stop later writes.
fn recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[allow(clippy::too_many_arguments)]
fn import_failure_diagnostic(
    document: &str,
    record: &str,
    author: &str,
    request: Option<&str>,
    update_length: usize,
    sealed_length: usize,
    category: &str,
    phase: &str,
) -> JsonValue {
    json!({
        "event":"documents.import_failure", "category":category, "documentId":document,
        "recordId":record, "author":author.chars().take(128).collect::<String>(),
        "requestId":request, "updateLength":update_length,
        "sealedLength":sealed_length, "phase":phase,
    })
}

impl BodyHub {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, DocumentBody>> {
        self.bodies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The publication gate. A panic while it was held leaves nothing half-published that the
    /// next holder could see, so a poisoned gate is taken as it is.
    fn gate(&self) -> std::sync::MutexGuard<'_, ()> {
        recover(&self.publication_gate)
    }

    /// Runs `with` on the body of `document`, creating it on first use.
    fn with_body<T>(&self, document: &str, with: impl FnOnce(&mut DocumentBody) -> T) -> T {
        let mut bodies = self.lock();
        let body = bodies
            .entry(document.to_owned())
            .or_insert_with(DocumentBody::new);
        with(body)
    }

    fn remember_identity(
        &self,
        document: &str,
        id: &str,
        author: &str,
        digest: [u8; 32],
        durable: bool,
    ) {
        if self
            .with_body(document, |body| {
                body.remember_identity(id.to_owned(), author.to_owned(), digest, durable)
            })
            .is_err()
        {
            // Conflicting sealed identity or an impossible replay never certifies absence.
            self.qualified.store(false, Ordering::Release);
        }
    }

    /// An unopened deferred edit can conflict with an applied edit. Do not show the
    /// applied body until that sealed row has been opened and checked.
    fn refresh_deferred_waiting(&self) -> Result<Vec<String>, BackendError> {
        let waiting = recover(&self.waiting_metadata).clone();
        let targets: HashSet<String> = self
            .store
            .deferred_records()
            .map_err(|_| storage_failed())?
            .into_iter()
            .filter(|row| {
                row.purpose == DomainRecord::EDIT_KIND && waiting.contains(&row.record_id)
            })
            .filter_map(|row| row.target)
            .collect();
        let mut bodies = self.lock();
        let mut changed = Vec::new();
        for document in &targets {
            bodies
                .entry(document.clone())
                .or_insert_with(DocumentBody::new);
        }
        for (document, body) in bodies.iter_mut() {
            if body.set_waiting_deferred(targets.contains(document)) {
                changed.push(document.clone());
            }
        }
        Ok(changed)
    }

    fn refresh_identity_index_complete(&self) {
        let bodies_ready = self.lock().values().all(DocumentBody::editable);
        let metadata_ready = recover(&self.waiting_metadata).is_empty();
        self.identity_index_complete
            .store(bodies_ready && metadata_ready, Ordering::Release);
    }

    #[allow(clippy::too_many_arguments)]
    fn publish_import_failure(
        &self,
        document: &str,
        record: &str,
        author: &str,
        request: Option<&str>,
        update_length: usize,
        sealed_length: usize,
        category: &str,
        phase: &str,
    ) {
        eprintln!(
            "{}",
            import_failure_diagnostic(
                document,
                record,
                author,
                request,
                update_length,
                sealed_length,
                category,
                phase
            )
        );
        let _ = self.events.send(BodyEvent::Status {
            document: document.to_owned(),
            durable: None,
        });
    }

    fn project_with_candidates(
        &self,
        phase: &str,
        on_commit: &dyn Fn(),
        ingest: impl FnOnce(&dyn Fn(), &ImportClassifier<'_>) -> Result<Ingested, StoreError>,
    ) -> Result<Ingested, StoreError> {
        let _publication = self.gate();
        let pending = Mutex::new(Vec::new());
        let failures = Mutex::new(Vec::new());
        let classify = |record: &str,
                        document: &str,
                        author: &str,
                        request: Option<&str>,
                        sealed_length: usize,
                        update: &[u8]| {
            match self.with_body(document, |body| body.import_candidate(update)) {
                Ok((mut candidate, _)) => {
                    if author != self.installation {
                        candidate.note_foreign_record(update);
                    }
                    pending
                        .lock()
                        .unwrap()
                        .push((document.to_owned(), candidate));
                    None
                }
                Err(category) => {
                    failures.lock().unwrap().push((
                        document.to_owned(),
                        record.to_owned(),
                        author.to_owned(),
                        request.map(str::to_owned),
                        update.len(),
                        sealed_length,
                        category.as_str(),
                    ));
                    Some(category.as_str())
                }
            }
        };
        let publish = || {
            // The store holds its lock while publishing, so only the bodies are touched here.
            for (document, candidate) in pending.lock().unwrap().drain(..) {
                self.adopt(&document, candidate, None);
            }
            for (document, record, author, request, update_length, sealed_length, category) in
                failures.lock().unwrap().drain(..)
            {
                self.publish_import_failure(
                    &document,
                    &record,
                    &author,
                    request.as_deref(),
                    update_length,
                    sealed_length,
                    category,
                    phase,
                );
            }
            on_commit();
        };
        ingest(&publish, &classify)
    }

    #[allow(clippy::too_many_arguments)]
    fn ingest_record(
        &self,
        record_id: &str,
        order_key: &str,
        author: &str,
        purpose: &str,
        signature: &str,
        canonical: &[u8],
        sealed: &[u8],
        cursor: Option<&str>,
        on_commit: &dyn Fn(),
    ) -> Result<Ingested, StoreError> {
        self.project_with_candidates("live", on_commit, |publish, classify| {
            self.store.ingest_with_import(
                record_id, order_key, author, purpose, signature, canonical, sealed, cursor,
                publish, classify,
            )
        })
    }

    fn retry_deferred_record(
        &self,
        record_id: &str,
        canonical: &[u8],
    ) -> Result<Ingested, StoreError> {
        self.project_with_candidates("replay", &|| {}, |publish, classify| {
            self.store
                .retry_deferred_with_import(record_id, canonical, publish, classify)
        })
    }

    fn append_lock(&self, document: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.append_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(document.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// Exercises body publication independently of store ingestion in body-event tests.
    #[cfg(test)]
    fn apply_edits(&self, edits: &[(String, Vec<u8>)]) {
        for (document, update) in edits {
            if self.store.identity_conflicted(document).unwrap_or(true) {
                continue;
            }
            match self.with_body(document, |body| body.import_candidate(update)) {
                Ok((candidate, _)) => self.adopt(document, candidate, None),
                Err(_) => eprintln!("documents body import failure: document={document}"),
            }
        }
    }

    fn import_opened_update(
        &self,
        row: &crate::store::StoredRecord,
        document: &str,
        update: &[u8],
        request_id: Option<&str>,
    ) -> Result<(), BackendError> {
        let _publication = self.gate();
        if self
            .store
            .identity_conflicted(document)
            .map_err(|_| storage_failed())?
        {
            return Ok(());
        }
        if let Some(failure_ordinal) = self
            .store
            .import_failure_ordinal(document)
            .map_err(|_| storage_failed())?
            && row.ingestion_ordinal >= failure_ordinal
        {
            return Ok(());
        }
        let imported = self.with_body(document, |body| body.import_candidate(update));
        if let Err(category) = imported {
            self.store
                .mark_import_failure(document, &row.record_id, category.as_str())
                .map_err(|_| storage_failed())?;
            self.publish_import_failure(
                document,
                &row.record_id,
                &row.author,
                request_id,
                update.len(),
                row.sealed_record.len(),
                category.as_str(),
                "replay",
            );
            return Ok(());
        }
        if let Ok((mut candidate, _)) = imported {
            if row.author != self.installation {
                candidate.note_foreign_record(update);
            }
            self.adopt(document, candidate, None);
        }
        Ok(())
    }

    /// Adopts the import `candidate` into the body of `document` ([`DocumentBody::adopt_import`])
    /// and tells the editor sessions what it added: one update of the operations it gained, and
    /// a status when its editability changed.
    /// An update too large for one UI message makes every session reload instead.
    ///
    /// Only the bodies lock is taken, so this may run while the store holds its own.
    fn adopt(&self, document: &str, candidate: DocumentBody, origin: Option<u64>) {
        let (update, editability_changed) = self.with_body(document, |body| {
            let before = body.version_vector();
            let was_editable = body.editable();
            // Bookkeeping can change outside the gate between classify and adopt (an append
            // that succeeded, an identity, the unopened count); only the import is adopted.
            body.adopt_import(candidate);
            (body.updates_since(&before), was_editable != body.editable())
        });
        if let Some(update) = update {
            self.publish_update(document, update, origin);
        }
        if editability_changed {
            self.publish_status(document);
        }
    }

    fn publish_update(&self, document: &str, update: Vec<u8>, origin: Option<u64>) {
        let event = if update.len() > MAX_UPDATE {
            BodyEvent::Resync {
                document: document.to_owned(),
            }
        } else {
            BodyEvent::Update {
                document: document.to_owned(),
                update: Arc::new(update),
                origin,
            }
        };
        let _ = self.events.send(event);
    }

    /// Asks every editor session of `document` to report its editability again.
    fn publish_status(&self, document: &str) {
        let _ = self.events.send(BodyEvent::Status {
            document: document.to_owned(),
            durable: None,
        });
    }

    /// Whether `document` can be edited now, and why not.
    ///
    /// The store is asked first, and the bodies lock only afterwards (ingest order).
    fn access(&self, document: &str) -> BodyAccess {
        let refused = |reason, diagnostic| BodyAccess {
            editable: false,
            reason,
            diagnostic,
        };
        match self.store.identity_conflicted(document) {
            Ok(true) => return refused(Some("identity_conflict"), None),
            Err(_) => return refused(None, None),
            Ok(false) => {}
        }
        match self.store.import_failure(document) {
            Ok(Some((record, _))) => return refused(Some("import_failure"), Some(record)),
            Err(_) => return refused(None, None),
            Ok(None) => {}
        }
        BodyAccess {
            editable: self.with_body(document, |body| body.editable()),
            reason: None,
            diagnostic: None,
        }
    }

    /// The body state `documents.document.open` answers with.
    fn access_value(&self, document: &str) -> JsonValue {
        let mut value = json!({ "documentId": document });
        self.access(document).write(&mut value);
        value
    }

    /// The opening of one editor session as `peer`: `begin`, the snapshot in base64 chunks, and
    /// `end`. Only an editable body, or the last good body after an import failure, has a
    /// snapshot; a pending or conflicted body sends none.
    fn session_opening(&self, document: &str, peer: u64) -> (BodyAccess, Vec<JsonValue>) {
        let access = self.access(document);
        let (version, snapshot) = self.with_body(document, |body| {
            let snapshot = if access.editable {
                body.snapshot()
            } else if access.reason == Some("import_failure") {
                body.applied_snapshot()
            } else {
                Vec::new()
            };
            (body.revision(), snapshot)
        });
        let chunks: Vec<&[u8]> = snapshot.chunks(SNAPSHOT_CHUNK_BYTES).collect();
        let mut begin = json!({
            "event": "begin", "documentId": document, "sessionPeer": peer.to_string(),
            "version": version.to_string(), "chunks": chunks.len(),
        });
        access.write(&mut begin);
        let mut events = vec![begin];
        for (index, chunk) in chunks.into_iter().enumerate() {
            events.push(json!({
                "event": "chunk", "documentId": document, "chunkIndex": index,
                "chunk": STANDARD.encode(chunk),
            }));
        }
        events.push(json!({ "event": "end", "documentId": document }));
        (access, events)
    }

    /// Registers an editor session of `document` and returns its peer: `resume` when the same UI
    /// may continue as it, otherwise a freshly allocated one.
    fn register_session(&self, document: &str, resume: Option<u64>) -> (u64, Arc<Notify>) {
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let resumable = resume.filter(|peer| {
            !sessions.contains_key(peer)
                && self.with_body(document, |body| body.may_resume(*peer))
                && (self
                    .ended_sessions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(peer)
                    .is_some_and(|ended| ended == document)
                    || crate::body::claim_peer(*peer))
        });
        let peer =
            resumable.unwrap_or_else(|| self.with_body(document, |body| body.allocate_peer()));
        self.ended_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&peer);
        let refused = Arc::new(Notify::new());
        sessions.insert(
            peer,
            SessionEntry {
                document: document.to_owned(),
                refused: refused.clone(),
            },
        );
        (peer, refused)
    }

    fn session(&self, peer: u64) -> Option<SessionEntry> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&peer)
            .cloned()
    }

    /// Ends the session of `peer`: it is no longer registered, and the same UI may resume it.
    fn end_session(&self, peer: u64) {
        let ended = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&peer);
        if let Some(entry) = ended {
            entry.refused.notify_one();
            self.ended_sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(peer, entry.document);
        }
    }

    /// Inclusion and durability of each request ID, taken together under the publication gate
    /// so no conflict marker or import failure commits halfway through.
    fn recovery_statuses(
        &self,
        request: &RecoverRequest,
        author: &str,
        looked_up: &Result<IdentityLookups, BackendError>,
        foreign_ids: &HashSet<&str>,
    ) -> Result<Vec<JsonValue>, BackendError> {
        let _publication = self.gate();
        let unresolved = |id: &String| json!({"requestId":id,"inclusion":"unresolved","durability":"unresolved"});
        if self
            .store
            .identity_conflicted(&request.document_id)
            .map_err(|_| storage_failed())?
            || self
                .store
                .import_failure(&request.document_id)
                .map_err(|_| storage_failed())?
                .is_some()
        {
            return Ok(request.request_ids.iter().map(unresolved).collect());
        }
        Ok(self.with_body(&request.document_id, |body| {
            request
                .request_ids
                .iter()
                .map(|id| {
                    match body.identity(id).filter(|known| known.author == *author) {
                        Some(known) => json!({
                            "requestId": id,
                            "inclusion": if body.editable() && !known.projection_pending { "included" } else { "unresolved" },
                            "durability": if known.durable { "durable" } else { "pending" },
                        }),
                        None if looked_up.is_ok()
                            && !foreign_ids.contains(id.as_str())
                            && self.qualified.load(Ordering::Acquire)
                            && body.editable() =>
                        {
                            json!({"requestId":id,"inclusion":"absent","durability":"unresolved"})
                        }
                        None => unresolved(id),
                    }
                })
                .collect()
        }))
    }
}

/// What the Package tells the UI about synchronization.
///
/// Availability changes only when the host sends a state event or the session reports a
/// failure; ingesting a record never implies it.
#[derive(Debug, Clone)]
pub(crate) struct SyncReport {
    pub(crate) availability: Availability,
    pub(crate) warnings: Vec<String>,
}

impl SyncReport {
    fn catching_up() -> Self {
        Self {
            availability: Availability::CatchingUp,
            warnings: Vec::new(),
        }
    }
}

/// The record log session one activation owns.
struct Session {
    cancellation: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

/// Everything one activation of Documents owns.
///
/// Every store call runs inside `Context::blocking`, so SQLite never holds an async worker.
pub struct DocumentsActive {
    store: DocumentStore,
    log: RecordLog,
    names: Principals,
    sync: Arc<Mutex<SyncReport>>,
    changes: watch::Sender<u64>,
    installation: String,
    session: Mutex<Option<Session>>,
    hub: BodyHub,
    flusher: Mutex<Option<Session>>,
}

/// What one activation built, for a test that must look inside.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone)]
pub(crate) struct ActivationHandles {
    pub(crate) store: DocumentStore,
    pub(crate) sync: Arc<Mutex<SyncReport>>,
    pub(crate) bodies: Bodies,
    pub(crate) waiting_metadata: Arc<Mutex<HashSet<String>>>,
    #[cfg(test)]
    qualified: Arc<AtomicBool>,
    #[cfg(test)]
    hub: BodyHub,
    #[cfg(test)]
    log: RecordLog,
    #[cfg(test)]
    changes: watch::Sender<u64>,
}

pub(crate) type ActivationObserver = Arc<Mutex<Option<ActivationHandles>>>;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollectionCreateRequest {
    name: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollectionRenameRequest {
    collection_id: String,
    name: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollectionRequest {
    collection_id: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DocumentCreateRequest {
    collection_id: String,
    title: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DocumentRenameRequest {
    document_id: String,
    title: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DocumentMoveRequest {
    document_id: String,
    collection_id: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DocumentRequest {
    document_id: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CommentsRequest {
    document_id: String,
    #[serde(default)]
    cursor: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CommentAddRequest {
    document_id: String,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    quote: Option<String>,
    markdown: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CommentRequest {
    comment_id: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecoverRequest {
    document_id: String,
    request_ids: Vec<String>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateRequest {
    document_id: String,
    session_peer: String,
    request_id: String,
    update: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MarkdownRequest {
    document_id: String,
    chunk_index: usize,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionRequest {
    document_id: String,
    #[serde(default)]
    resume_peer: Option<String>,
}

/// A session peer as the UI contract writes it: a decimal `u64`.
fn session_peer(text: &str) -> Option<u64> {
    if text.len() > 1 && text.starts_with('0') {
        return None;
    }
    text.parse().ok()
}

/// Opens one editor session: registers its peer, sends `begin`, the snapshot chunks and `end`,
/// then relays the updates of every other writer and the body's status until the session
/// ends. A session that opened with the live body keeps following it while the body waits for
/// history. A refused update, a lagging relay, a conflict, or a body that becomes editable for
/// a session that opened without it ends the session with `resync`, after which the UI opens
/// a new session.
async fn open_session(
    context: &Context<DocumentsActive>,
    request: SessionRequest,
) -> Result<EventSubscription, BackendError> {
    require_document(context, &request.document_id).await?;
    let resume = match request.resume_peer.as_deref() {
        Some(text) => Some(session_peer(text).ok_or_else(invalid_request)?),
        None => None,
    };
    reopen_unopened_edits(context, &request.document_id).await?;
    let hub = context.state().hub.clone();
    let document = request.document_id;
    // Subscribe before taking the snapshot: an update in both is imported twice, harmlessly.
    let mut events = hub.events.subscribe();
    let (peer, refused) = hub.register_session(&document, resume);
    let session = SessionGuard {
        hub: hub.clone(),
        peer,
    };
    let opening = {
        let hub = hub.clone();
        let document = document.clone();
        context
            .blocking(move |_| Ok(hub.session_opening(&document, peer)))
            .await?
    };
    let (sender, subscription) = event_channel(32);
    let context = context.clone();
    tokio::spawn(async move {
        let _session = session;
        let (mut access, opening) = opening;
        // A session opened on an editable body holds the live body: it follows every change,
        // including while the body waits for history, and keeps its own unacknowledged edits.
        // One opened without it (pending or conflicted) must reload once the body is editable.
        let holds_body = access.editable;
        for event in opening {
            if sender.send(event).await.is_err() {
                return;
            }
        }
        let resync = json!({ "event": "resync", "documentId": document });
        loop {
            let event = tokio::select! {
                () = sender.closed() => return,
                () = refused.notified() => {
                    let _ = sender.send(resync).await;
                    return;
                }
                event = events.recv() => event,
            };
            let next = match event {
                Ok(event) if event.document() != document => continue,
                Ok(BodyEvent::Update { update, origin, .. }) => {
                    if origin == Some(peer) || !(access.editable || holds_body) {
                        continue;
                    }
                    json!({
                        "event": "update", "documentId": document,
                        "update": STANDARD.encode(update.as_slice()),
                    })
                }
                Ok(BodyEvent::Status { durable, .. }) => {
                    let hub = hub.clone();
                    let target = document.clone();
                    let Ok(now) = context.blocking(move |_| Ok(hub.access(&target))).await else {
                        return;
                    };
                    // A session without the body reloads once it is editable; every session
                    // reloads when a conflict hides the body.
                    if now != access
                        && ((now.editable && !holds_body)
                            || now.reason == Some("identity_conflict"))
                    {
                        let _ = sender.send(resync).await;
                        return;
                    }
                    access = now;
                    let mut status = json!({
                        "event": "status", "documentId": document,
                        "durable": durable.into_iter().collect::<Vec<_>>(),
                    });
                    access.write(&mut status);
                    status
                }
                Ok(BodyEvent::Resync { .. }) | Err(broadcast::error::RecvError::Lagged(_)) => {
                    let _ = sender.send(resync).await;
                    return;
                }
                Err(broadcast::error::RecvError::Closed) => return,
            };
            if sender.send(next).await.is_err() {
                return;
            }
        }
    });
    Ok(subscription)
}

/// Ends a session's registration when its subscription task ends, however it ends.
struct SessionGuard {
    hub: BodyHub,
    peer: u64,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.hub.end_session(self.peer);
    }
}

/// Validates and appends one update from an open editor session. A refusal that retrying the
/// same bytes cannot cure ends the session with `resync`.
async fn update_from_session(
    context: &Context<DocumentsActive>,
    request: UpdateRequest,
) -> Result<JsonValue, BackendError> {
    let state = context.state();
    let peer = session_peer(&request.session_peer).ok_or_else(invalid_request)?;
    let Some(session) = state.hub.session(peer) else {
        return Err(error("documents.unknown_session", false));
    };
    if session.document != request.document_id {
        state.hub.end_session(peer);
        return Err(error("documents.unknown_session", false));
    }
    let refuse = |cause: BackendError| {
        if !cause.retryable() {
            state.hub.end_session(peer);
        }
        cause
    };
    let update = STANDARD
        .decode(&request.update)
        .map_err(|_| refuse(error("documents.invalid_update", false)))?;
    submit_update(
        &state.log,
        &state.hub,
        &state.changes,
        &state.installation,
        &request.document_id,
        peer,
        &request.request_id,
        &update,
    )
    .await
    .map_err(refuse)?;
    Ok(json!({ "requestId": request.request_id }))
}

/// One chunk of the body's Canonical Markdown, with a digest of the whole text so a reader can
/// tell that every chunk it assembled came from the same text.
fn markdown_chunk(hub: &BodyHub, document: &str, index: usize) -> Result<JsonValue, BackendError> {
    let access = hub.access(document);
    let markdown = match (access.editable, access.reason) {
        (true, _) => hub.with_body(document, |body| body.text().to_owned()),
        (false, Some("import_failure")) => {
            hub.with_body(document, |body| body.applied_text().to_owned())
        }
        (false, Some(_)) => return Err(error("documents.unavailable", false)),
        (false, None) => return Err(error("documents.document_pending", true)),
    };
    let mut chunks = Vec::new();
    let mut offset = 0;
    while offset < markdown.len() {
        let mut end = (offset + MARKDOWN_CHUNK_BYTES).min(markdown.len());
        while !markdown.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(&markdown[offset..end]);
        offset = end;
    }
    if chunks.is_empty() {
        chunks.push("");
    }
    let Some(chunk) = chunks.get(index) else {
        return Err(invalid_request());
    };
    let digest: String = Sha256::digest(markdown.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(json!({
        "documentId": document, "digest": digest, "chunks": chunks.len(),
        "chunkIndex": index, "chunk": chunk,
    }))
}

/// The Documents Backend.
#[must_use]
pub fn router() -> Router<DocumentsActive> {
    router_observing(None)
}

/// The Backend, publishing what each activation built.
///
/// Only a test passes an observer; the shipped router passes `None`.
pub(crate) fn router_observing(observer: Option<ActivationObserver>) -> Router<DocumentsActive> {
    Router::new(move |activation| {
        let observer = observer.clone();
        let initialization = activation.initialization().clone();
        let capabilities = activation.capabilities().clone();
        let cancellation = activation.cancellation().clone();
        async move {
            let directory = initialization.data_directory().path().to_owned();
            let store = teamarness_package_sdk::blocking(&cancellation, move |_| {
                DocumentStore::open(directory.join("documents.sqlite3"))
                    .map_err(|_| storage_failed())
            })
            .await?;
            let replay_store = store.clone();
            teamarness_package_sdk::blocking(&cancellation, move |_| {
                replay_store
                    .begin_replay_qualification()
                    .map_err(|_| storage_failed())
            })
            .await?;
            let log = RecordLog::from_initialization(&initialization, capabilities.clone())
                .map_err(|cause| capability_error("record-log", cause))?;
            let names = Principals::from_initialization(&initialization, capabilities)
                .map_err(|cause| capability_error("principals", cause))?;
            let installation = initialization.installation().identity.clone();
            let (events, _) = broadcast::channel(256);
            let hub = BodyHub {
                store: store.clone(),
                bodies: Arc::new(Mutex::new(HashMap::new())),
                installation: installation.clone(),
                events,
                sessions: Arc::new(Mutex::new(HashMap::new())),
                ended_sessions: Arc::new(Mutex::new(HashMap::new())),
                flush: Arc::new(Notify::new()),
                append_locks: Arc::new(Mutex::new(HashMap::new())),
                waiting_metadata: Arc::new(Mutex::new(HashSet::new())),
                metadata_lock: Arc::new(tokio::sync::Mutex::new(())),
                qualified: Arc::new(AtomicBool::new(false)),
                identity_index_complete: Arc::new(AtomicBool::new(false)),
                identity_lookup_lock: Arc::new(tokio::sync::Mutex::new(())),
                publication_gate: Arc::new(Mutex::new(())),
            };
            rehydrate(&store, &log, &hub, &cancellation).await?;

            let sync = Arc::new(Mutex::new(SyncReport::catching_up()));
            let (changes, _) = watch::channel(0);
            if let Some(observer) = observer {
                *observer
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ActivationHandles {
                    store: store.clone(),
                    sync: sync.clone(),
                    bodies: hub.bodies.clone(),
                    waiting_metadata: hub.waiting_metadata.clone(),
                    #[cfg(test)]
                    qualified: hub.qualified.clone(),
                    #[cfg(test)]
                    hub: hub.clone(),
                    #[cfg(test)]
                    log: log.clone(),
                    #[cfg(test)]
                    changes: changes.clone(),
                });
            }
            let session = start_session(&store, &log, &sync, &changes, &hub);
            let flusher = start_flusher(&log, &hub, &changes, installation.clone());
            Ok(DocumentsActive {
                store,
                log,
                names,
                sync,
                changes,
                installation,
                session: Mutex::new(Some(session)),
                hub,
                flusher: Mutex::new(Some(flusher)),
            })
        }
    })
    .on_suspend(|state: Arc<DocumentsActive>| async move {
        stop_session(&state).await;
        stop_flusher(&state).await;
        Ok(())
    })
    .on_resume(|state: Arc<DocumentsActive>| async move {
        stop_session(&state).await;
        stop_flusher(&state).await;
        let replay_store = state.store.clone();
        tokio::task::spawn_blocking(move || replay_store.begin_replay_qualification())
            .await
            .map_err(|_| storage_failed())?
            .map_err(|_| storage_failed())?;
        let session = start_session(
            &state.store,
            &state.log,
            &state.sync,
            &state.changes,
            &state.hub,
        );
        *state
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(session);
        let flusher = start_flusher(
            &state.log,
            &state.hub,
            &state.changes,
            state.installation.clone(),
        );
        *state
            .flusher
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(flusher);
        Ok(())
    })
    .on_dispose(|state: Arc<DocumentsActive>| async move {
        stop_session(&state).await;
        stop_flusher(&state).await;
        Ok(())
    })
    .request(
        "documents.snapshot",
        |context: Context<DocumentsActive>, _: Empty| async move {
            let state = context.state().clone();
            context.blocking(move |_| state.snapshot_value()).await
        },
    )
    .request(
        "documents.collection.create",
        |context: Context<DocumentsActive>, request: CollectionCreateRequest| async move {
            let author = context.state().installation.clone();
            let record =
                DomainRecord::collection_create(&author, &request.name).map_err(domain_error)?;
            append(&context, record).await
        },
    )
    .request(
        "documents.collection.rename",
        |context: Context<DocumentsActive>, request: CollectionRenameRequest| async move {
            require_collection(&context, &request.collection_id).await?;
            let author = context.state().installation.clone();
            let record =
                DomainRecord::collection_rename(&author, &request.collection_id, &request.name)
                    .map_err(domain_error)?;
            append(&context, record).await
        },
    )
    .request(
        "documents.collection.archive",
        |context: Context<DocumentsActive>, request: CollectionRequest| async move {
            require_collection(&context, &request.collection_id).await?;
            let author = context.state().installation.clone();
            let record = DomainRecord::collection_archive(&author, &request.collection_id)
                .map_err(domain_error)?;
            append(&context, record).await
        },
    )
    .request(
        "documents.collection.restore",
        |context: Context<DocumentsActive>, request: CollectionRequest| async move {
            require_collection(&context, &request.collection_id).await?;
            let author = context.state().installation.clone();
            let record = DomainRecord::collection_restore(&author, &request.collection_id)
                .map_err(domain_error)?;
            append(&context, record).await
        },
    )
    .request(
        "documents.document.create",
        |context: Context<DocumentsActive>, request: DocumentCreateRequest| async move {
            require_collection(&context, &request.collection_id).await?;
            let author = context.state().installation.clone();
            let record = DomainRecord::document_create(
                &author,
                &request.collection_id,
                &request.title,
                crate::body::genesis(),
            )
            .map_err(domain_error)?;
            append(&context, record).await
        },
    )
    .request(
        "documents.document.rename",
        |context: Context<DocumentsActive>, request: DocumentRenameRequest| async move {
            require_document(&context, &request.document_id).await?;
            let author = context.state().installation.clone();
            let record =
                DomainRecord::document_rename(&author, &request.document_id, &request.title)
                    .map_err(domain_error)?;
            append(&context, record).await
        },
    )
    .request(
        "documents.document.move",
        |context: Context<DocumentsActive>, request: DocumentMoveRequest| async move {
            require_document(&context, &request.document_id).await?;
            require_collection(&context, &request.collection_id).await?;
            let author = context.state().installation.clone();
            let record =
                DomainRecord::document_move(&author, &request.document_id, &request.collection_id)
                    .map_err(domain_error)?;
            append(&context, record).await
        },
    )
    .request(
        "documents.document.archive",
        |context: Context<DocumentsActive>, request: DocumentRequest| async move {
            require_document(&context, &request.document_id).await?;
            let author = context.state().installation.clone();
            let record = DomainRecord::document_archive(&author, &request.document_id)
                .map_err(domain_error)?;
            append(&context, record).await
        },
    )
    .request(
        "documents.document.restore",
        |context: Context<DocumentsActive>, request: DocumentRequest| async move {
            require_document(&context, &request.document_id).await?;
            let author = context.state().installation.clone();
            let record = DomainRecord::document_restore(&author, &request.document_id)
                .map_err(domain_error)?;
            append(&context, record).await
        },
    )
    .request(
        "documents.document.open",
        |context: Context<DocumentsActive>, request: DocumentRequest| async move {
            require_document(&context, &request.document_id).await?;
            reopen_unopened_edits(&context, &request.document_id).await?;
            let hub = context.state().hub.clone();
            let document = request.document_id;
            context
                .blocking(move |_| Ok(hub.access_value(&document)))
                .await
        },
    )
    .request(
        "documents.document.recover",
        |context: Context<DocumentsActive>, request: RecoverRequest| async move {
            require_document(&context, &request.document_id).await?;
            if request.request_ids.len() > 8
                || request.request_ids.iter().any(|id| !is_request_id(id))
            {
                return Err(invalid_request());
            }
            let hub = &context.state().hub;
            let author = &context.state().installation;
            let looked_up = lookup_identities(
                &context.state().log,
                &context.state().store,
                hub,
                &request.request_ids,
            )
            .await;
            let mut foreign_ids = HashSet::new();
            if let Ok(found) = &looked_up {
                for (id, (document, owner, digest, durable)) in found {
                    if document == &request.document_id && owner == author {
                        hub.remember_identity(document, id, owner, *digest, *durable);
                    } else {
                        foreign_ids.insert(id.as_str());
                    }
                }
            }
            let statuses = hub.recovery_statuses(&request, author, &looked_up, &foreign_ids)?;
            Ok(json!({ "documentId": request.document_id, "requestStatuses": statuses }))
        },
    )
    .request(
        "documents.document.update",
        |context: Context<DocumentsActive>, request: UpdateRequest| async move {
            update_from_session(&context, request).await
        },
    )
    .request(
        "documents.document.markdown",
        |context: Context<DocumentsActive>, request: MarkdownRequest| async move {
            require_document(&context, &request.document_id).await?;
            reopen_unopened_edits(&context, &request.document_id).await?;
            let hub = context.state().hub.clone();
            context
                .blocking(move |_| markdown_chunk(&hub, &request.document_id, request.chunk_index))
                .await
        },
    )
    .request(
        "documents.comments",
        |context: Context<DocumentsActive>, request: CommentsRequest| async move {
            require_document(&context, &request.document_id).await?;
            let state = context.state().clone();
            let document = request.document_id;
            context
                .blocking(move |_| state.comments_value(&document, request.cursor.as_deref()))
                .await
        },
    )
    .request(
        "documents.comment.add",
        |context: Context<DocumentsActive>, request: CommentAddRequest| async move {
            require_document(&context, &request.document_id).await?;
            if let Some(parent) = &request.parent_id {
                require_comment(&context, parent).await?;
            }
            let author = context.state().installation.clone();
            let created_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| error("documents.invalid_timestamp", false))?
                .as_millis()
                .to_string();
            let record = DomainRecord::comment_add_at(
                &author,
                &request.document_id,
                request.parent_id.as_deref(),
                request.quote.as_deref(),
                &request.markdown,
                Some(&created_at),
            )
            .map_err(domain_error)?;
            append(&context, record).await
        },
    )
    .request(
        "documents.comment.resolve",
        |context: Context<DocumentsActive>, request: CommentRequest| async move {
            require_comment(&context, &request.comment_id).await?;
            let author = context.state().installation.clone();
            let record = DomainRecord::comment_resolve(&author, &request.comment_id)
                .map_err(domain_error)?;
            append(&context, record).await
        },
    )
    .request(
        "documents.comment.reopen",
        |context: Context<DocumentsActive>, request: CommentRequest| async move {
            require_comment(&context, &request.comment_id).await?;
            let author = context.state().installation.clone();
            let record =
                DomainRecord::comment_reopen(&author, &request.comment_id).map_err(domain_error)?;
            append(&context, record).await
        },
    )
    .subscription(
        "documents.document.session",
        |context: Context<DocumentsActive>, request: SessionRequest| async move {
            open_session(&context, request).await
        },
    )
    .subscription(
        "documents.names",
        |context: Context<DocumentsActive>, _: Empty| async move {
            let (sender, subscription) = event_channel(4);
            let names = context.state().names.clone();
            tokio::spawn(async move {
                loop {
                    let mut cursor = match names.current().await {
                        Ok(snapshot) => {
                            let cursor = snapshot.cursor;
                            for event in name_snapshots(snapshot) {
                                if sender.send(event).await.is_err() {
                                    return;
                                }
                            }
                            cursor
                        }
                        Err(_) => {
                            if sender
                                .send(json!({ "available": false, "cursor": "0", "persons": [] }))
                                .await
                                .is_err()
                            {
                                return;
                            }
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            continue;
                        }
                    };
                    if let Ok(mut stream) = names.subscribe(cursor).await {
                        loop {
                            let result = tokio::select! {
                                result = stream.next() => result,
                                () = sender.closed() => {
                                    stream.close().await;
                                    return;
                                }
                            };
                            match result {
                                Some(Ok(snapshot)) => {
                                    if snapshot.cursor >= cursor {
                                        cursor = snapshot.cursor;
                                        for event in name_snapshots(snapshot) {
                                            if sender.send(event).await.is_err() {
                                                stream.close().await;
                                                return;
                                            }
                                        }
                                    }
                                }
                                Some(Err(_)) | None => break,
                            }
                        }
                        stream.close().await;
                    }
                    if sender
                        .send(json!({ "available": false, "cursor": "0", "persons": [] }))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            });
            Ok(subscription)
        },
    )
    .subscription(
        "documents.changes",
        |context: Context<DocumentsActive>, _: Empty| async move {
            let (sender, subscription) = event_channel(32);
            let mut revisions = context.state().changes.subscribe();
            tokio::spawn(async move {
                loop {
                    let state = context.state().clone();
                    let Ok(snapshot) = context.blocking(move |_| state.snapshot_value()).await
                    else {
                        return;
                    };
                    if sender.send(snapshot).await.is_err() {
                        return;
                    }
                    if revisions.changed().await.is_err() {
                        return;
                    }
                }
            });
            Ok(subscription)
        },
    )
    .port_request(
        ports::CONTRACT, "document.list", "query",
        |context: Context<DocumentsActive>, _actor: PortActor, request: ports::ListRequest| async move {
            ports::list(&context, request).await
        },
    )
    .port_request(
        ports::CONTRACT, "document.markdown", "query",
        |context: Context<DocumentsActive>, _actor: PortActor, request: ports::MarkdownRequest| async move {
            ports::read_markdown(&context, request).await
        },
    )
    .port_request(
        ports::CONTRACT, "document.revise", "command",
        |context: Context<DocumentsActive>, actor: PortActor, request: ports::ReviseRequest| async move {
            ports::authorize_person(&context, &actor)?;
            ports::revise(&context, request).await
        },
    )
    .port_event(
        ports::CONTRACT, "documents.changed",
        |context: Context<DocumentsActive>, _actor: PortActor| async move {
            ports::changed(&context)
        },
    )
}

async fn require_collection(
    context: &Context<DocumentsActive>,
    id: &str,
) -> Result<(), BackendError> {
    let state = context.state().clone();
    let id = id.to_owned();
    let exists = context
        .blocking(move |_| {
            state
                .store
                .collection_exists(&id)
                .map_err(|_| storage_failed())
        })
        .await?;
    if exists {
        Ok(())
    } else {
        Err(error("documents.unknown_collection", false))
    }
}

async fn require_document(
    context: &Context<DocumentsActive>,
    id: &str,
) -> Result<(), BackendError> {
    let state = context.state().clone();
    let id = id.to_owned();
    let exists = context
        .blocking(move |_| {
            state
                .store
                .document_exists(&id)
                .map_err(|_| storage_failed())
        })
        .await?;
    if exists {
        Ok(())
    } else {
        Err(error("documents.unknown_document", false))
    }
}

/// Projects each current Principal's signing key into the existing bounded author-name feed.
fn name_snapshots(snapshot: PrincipalSnapshot) -> Vec<JsonValue> {
    const PERSONS_PER_EVENT: usize = 64;
    let persons: Vec<_> = snapshot
        .principals
        .into_iter()
        .map(|principal| {
            json!({
                "identity": principal.keys[0], "personId": principal.principal_id,
                "name": principal.name, "fingerprint": principal.fingerprint,
            })
        })
        .collect();
    let parts = persons.len().max(1).div_ceil(PERSONS_PER_EVENT);
    persons
        .chunks(PERSONS_PER_EVENT)
        .enumerate()
        .map(|(part, chunk)| {
            json!({
                "available": true, "cursor": snapshot.cursor.to_string(),
                "persons": chunk, "part": part, "parts": parts,
            })
        })
        .chain((persons.is_empty()).then(|| {
            json!({
                "available": true, "cursor": snapshot.cursor.to_string(),
                "persons": [], "part": 0, "parts": 1,
            })
        }))
        .collect()
}

async fn require_comment(context: &Context<DocumentsActive>, id: &str) -> Result<(), BackendError> {
    let state = context.state().clone();
    let id = id.to_owned();
    let exists = context
        .blocking(move |_| {
            state
                .store
                .comment_exists(&id)
                .map_err(|_| storage_failed())
        })
        .await?;
    if exists {
        Ok(())
    } else {
        Err(error("documents.unknown_comment", false))
    }
}

/// Both the SDK and host cap an `open` call at 32 records and 512 KiB of decoded sealed input.
fn open_batches(
    rows: &[crate::store::StoredRecord],
) -> impl Iterator<Item = &[crate::store::StoredRecord]> {
    let mut remaining = rows;
    std::iter::from_fn(move || {
        if remaining.is_empty() {
            return None;
        }
        let mut count = 0;
        let mut bytes: usize = 0;
        for row in remaining.iter().take(32) {
            let Some(total) = bytes.checked_add(row.sealed_record.len()) else {
                break;
            };
            if total > 512 * 1024 {
                break;
            }
            count += 1;
            bytes = total;
        }
        // An individually invalid sealed record is handed to the SDK for its usual error;
        // a valid record always fits the capability's per-batch budget.
        let count = count.max(1);
        let (batch, rest) = remaining.split_at(count);
        remaining = rest;
        Some(batch)
    })
}

type IdentityLookups = HashMap<String, (String, String, [u8; 32], bool)>;

/// Read only the exact indexed sealed row to confirm a local retry's immutable digest.
/// A missing row remains unresolved until the qualified replay and all opens succeed.
async fn lookup_identities(
    log: &RecordLog,
    store: &DocumentStore,
    hub: &BodyHub,
    ids: &[String],
) -> Result<IdentityLookups, BackendError> {
    let mut found = HashMap::new();
    for id in ids {
        let cached = hub.lock().iter().find_map(|(document, body)| {
            body.identity(id).map(|known| {
                (
                    document.clone(),
                    known.author.clone(),
                    known.digest,
                    known.durable,
                )
            })
        });
        let reader = store.clone();
        let key = id.clone();
        let row = tokio::task::spawn_blocking(move || reader.indexed_edit_record(&key))
            .await
            .map_err(|_| storage_failed())?
            .map_err(|_| storage_failed())?;
        let Some(row) = row else {
            if store
                .edit_identity(id)
                .map_err(|_| storage_failed())?
                .is_some()
            {
                return Err(error("documents.document_pending", true));
            }
            if let Some(cached) = cached {
                found.insert(id.clone(), cached);
            }
            continue;
        };
        let opened = log
            .open(std::slice::from_ref(&row.sealed_record))
            .await
            .map_err(|cause| capability_error("lookup-edit-identity", cause))?;
        let Some(OpenResult::Ready(record)) = opened.into_iter().next() else {
            return Err(error("documents.document_pending", true));
        };
        if record.record_id.as_str() != row.record_id
            || record.order_key.as_str() != row.order_key
            || record.author != row.author
            || record.purpose != row.purpose
            || record.signature != row.signature
        {
            return Err(error("documents.document_pending", true));
        }
        let Ok(DomainRecord::IdentifiedDocumentEdit {
            author,
            document,
            request_id,
            payload_digest,
            ..
        }) = DomainRecord::decode(&record.canonical)
        else {
            return Err(error("documents.document_pending", true));
        };
        if request_id != *id
            || author != row.author
            || row.target.as_deref() != Some(&document)
            || row.purpose != DomainRecord::EDIT_KIND
        {
            return Err(error("documents.document_pending", true));
        }
        found.insert(id.clone(), (document, author, payload_digest, true));
    }
    Ok(found)
}

/// Re-opens the body records of one Document whose sealed bytes could not be opened before,
/// so a Document becomes editable as soon as history access arrives.
async fn reopen_unopened_edits(
    context: &Context<DocumentsActive>,
    document: &str,
) -> Result<(), BackendError> {
    let hub = context.state().hub.clone();
    if hub.with_body(document, |body| body.unopened()) == 0 {
        return Ok(());
    }
    let store = context.state().store.clone();
    let id = document.to_owned();
    let rows = context
        .blocking(move |_| store.body_records(Some(&id)).map_err(|_| storage_failed()))
        .await?;
    let unopened = import_edit_rows(&context.state().log, &hub, &rows).await?;
    let changed = hub.with_body(document, |body| {
        let changed = body.unopened() != unopened;
        body.set_unopened(unopened);
        changed
    });
    if changed {
        hub.refresh_identity_index_complete();
        hub.publish_status(document);
    }
    Ok(())
}

/// Opens `rows` in batches, imports the ready ones, and returns how many stayed unopened.
///
/// A body rebuilt this way imports every record, including ones it already holds; Loro
/// ignores operations it knows, so re-opening is idempotent.
async fn import_edit_rows(
    log: &RecordLog,
    hub: &BodyHub,
    rows: &[crate::store::StoredRecord],
) -> Result<usize, BackendError> {
    let mut unopened = 0;
    for batch in open_batches(rows) {
        let sealed: Vec<Vec<u8>> = batch.iter().map(|row| row.sealed_record.clone()).collect();
        let results = log
            .open(&sealed)
            .await
            .map_err(|cause| capability_error("open-edit-records", cause))?;
        if results.len() != batch.len() {
            return Err(error("documents.document_pending", true));
        }
        for (row, result) in batch.iter().zip(results) {
            match result {
                OpenResult::Ready(record)
                    if record.record_id.as_str() == row.record_id
                        && record.order_key.as_str() == row.order_key
                        && record.author == row.author
                        && record.purpose == row.purpose
                        && record.signature == row.signature =>
                {
                    match DomainRecord::decode(&record.canonical) {
                        Ok(DomainRecord::DocumentCreate { genesis, .. }) => {
                            hub.import_opened_update(row, &row.record_id, &genesis, None)?;
                        }
                        Ok(DomainRecord::IdentifiedDocumentEdit {
                            author,
                            document,
                            request_id,
                            payload_digest,
                            update,
                        }) => {
                            let indexed = {
                                let _publication = hub.gate();
                                hub.store.index_opened_edit(row, &record.canonical)
                            };
                            match indexed {
                                Ok(false) => {
                                    // A newly opened collision invalidates a previously visible body.
                                    for affected in hub
                                        .store
                                        .conflicted_documents()
                                        .map_err(|_| storage_failed())?
                                    {
                                        hub.publish_status(&affected);
                                    }
                                    continue;
                                }
                                Err(_) => {
                                    hub.qualified.store(false, Ordering::Release);
                                    unopened += 1;
                                    continue;
                                }
                                Ok(true) => {}
                            }
                            hub.import_opened_update(row, &document, &update, Some(&request_id))?;
                            hub.remember_identity(
                                &document,
                                &request_id,
                                &author,
                                payload_digest,
                                true,
                            );
                        }
                        _ => unopened += 1,
                    }
                }
                OpenResult::Ready(_)
                | OpenResult::WaitingForHistoryAccess
                | OpenResult::WrongScope
                | OpenResult::Corrupt => unopened += 1,
            }
        }
    }
    Ok(unopened)
}

impl DocumentsActive {
    /// Pages newest-first by immutable OrderKey and record identity, measuring serialized JSON.
    fn comments_value(
        &self,
        document: &str,
        cursor: Option<&str>,
    ) -> Result<JsonValue, BackendError> {
        const PAGE_BUDGET: usize = 240 * 1024;
        let all = self
            .store
            .comments(document)
            .map_err(|_| storage_failed())?;
        let end = match cursor {
            None => all.len(),
            Some(cursor) => all
                .iter()
                .position(|comment| format!("{}:{}", comment.order_key, comment.id) == cursor)
                .ok_or_else(|| error("documents.invalid_cursor", false))?,
        };
        let mut comments: Vec<JsonValue> = Vec::new();
        let mut next_cursor = None;
        for comment in all[..end].iter().rev() {
            let value = {
                let mut value = json!({
                    "id": comment.id,
                    "documentId": comment.document_id,
                    "author": comment.author,
                    "position": comment.order_key,
                    "resolved": comment.resolved,
                    "isLocal": comment.author == self.installation,
                });
                if let Some(parent) = &comment.parent_id {
                    value["parentId"] = json!(parent);
                }
                if let Some(markdown) = &comment.markdown {
                    value["markdown"] = json!(markdown);
                }
                if let Some(quote) = &comment.quote {
                    value["quote"] = json!(quote);
                }
                if let Some(time) = &comment.created_at {
                    value["createdAt"] = json!(time);
                }
                value
            };
            // Count the complete wire response, including cursor, JSON escaping and framing.
            let candidate = json!({
                "comments": comments.iter().chain(std::iter::once(&value)).collect::<Vec<_>>(),
                "nextCursor": format!("{}:{}", comment.order_key, comment.id),
                "hasMore": true,
            });
            if candidate.to_string().len() > PAGE_BUDGET || comments.len() == 256 {
                if comments.is_empty() {
                    return Err(error("documents.comment_page_too_large", false));
                }
                break;
            }
            next_cursor = Some(format!("{}:{}", comment.order_key, comment.id));
            comments.push(value);
        }
        let has_more = end > comments.len();
        Ok(
            json!({ "comments": comments, "nextCursor": if has_more { next_cursor } else { None }, "hasMore": has_more }),
        )
    }

    pub(crate) fn report(&self) -> SyncReport {
        self.sync
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The Collection and Document listing, availability, and warnings the UI renders.
    ///
    /// A name or title whose record has not been opened yet is shown as `Untitled`; the
    /// listing never invents content and the id remains the identity.
    fn snapshot_value(&self) -> Result<JsonValue, BackendError> {
        let report = self.report();
        let collections: Vec<JsonValue> = self
            .store
            .collections()
            .map_err(|_| storage_failed())?
            .into_iter()
            .map(|collection| {
                json!({
                    "id": collection.id,
                    "name": collection.name.unwrap_or_else(|| "Untitled".into()),
                    "archived": collection.archived,
                })
            })
            .collect();
        let documents: Vec<JsonValue> = self
            .store
            .documents()
            .map_err(|_| storage_failed())?
            .into_iter()
            .map(|document| {
                json!({
                    "id": document.id,
                    "collectionId": document.collection_id,
                    "title": document.title.unwrap_or_else(|| "Untitled".into()),
                    "archived": document.archived,
                })
            })
            .collect();
        let mut warnings = report.warnings.clone();
        if self
            .store
            .has_projection_warnings()
            .map_err(|_| storage_failed())?
        {
            warnings.push("Some synchronized records could not be applied.".into());
        }
        Ok(json!({
            "collections": collections,
            "documents": documents,
            "availability": report.availability.as_str(),
            "warnings": warnings,
        }))
    }
}

/// Appends one domain record and projects it locally, answering with the record identity,
/// which is the Collection or Document identity when the record created one.
async fn append(
    context: &Context<DocumentsActive>,
    record: DomainRecord,
) -> Result<JsonValue, BackendError> {
    let purpose = record.kind();
    let canonical = record.encode();
    let appended = context
        .state()
        .log
        .append(purpose, &canonical)
        .await
        .map_err(|cause| capability_error(purpose, cause))?;
    let state = context.state().clone();
    let id = appended.record_id.as_str().to_owned();
    let ingested = context
        .blocking(move |_| {
            state
                .hub
                .ingest_record(
                    appended.record_id.as_str(),
                    appended.order_key.as_str(),
                    &appended.author,
                    purpose,
                    &appended.signature,
                    &canonical,
                    &appended.sealed_record,
                    None,
                    &|| {
                        state
                            .changes
                            .send_modify(|revision| *revision = revision.saturating_add(1))
                    },
                )
                .map_err(|_| storage_failed())
        })
        .await?;
    match ingested.result {
        IngestResult::Applied | IngestResult::Duplicate => {}
        IngestResult::Deferred(_) => return Err(error("documents.deferred", true)),
        IngestResult::Quarantined(_)
        | IngestResult::IdentityConflict(_)
        | IngestResult::ImportFailure(_) => {
            return Err(error("documents.rejected", false));
        }
    }
    Ok(json!({ "ok": true, "id": id }))
}

/// Re-opens metadata still waiting for history without re-ingesting already projected rows.
async fn reopen_metadata(
    store: &DocumentStore,
    log: &RecordLog,
    hub: &BodyHub,
    cancellation: &CancellationToken,
    waiting_only: bool,
) -> Result<(), BackendError> {
    let _guard = hub.metadata_lock.lock().await;
    if waiting_only && recover(&hub.waiting_metadata).is_empty() {
        return Ok(());
    }
    for deferred in [false, true] {
        let reader = store.clone();
        let rows = teamarness_package_sdk::blocking(cancellation, move |_| {
            (if deferred {
                reader.deferred_records()
            } else {
                reader.control_records()
            })
            .map_err(|_| storage_failed())
        })
        .await?;
        let rows: Vec<_> = if waiting_only {
            let waiting = recover(&hub.waiting_metadata);
            rows.into_iter()
                .filter(|row| waiting.contains(&row.record_id))
                .collect()
        } else {
            rows
        };
        for batch in open_batches(&rows) {
            let sealed: Vec<Vec<u8>> = batch.iter().map(|row| row.sealed_record.clone()).collect();
            let results = log.open(&sealed).await.map_err(|cause| {
                capability_error(
                    if deferred {
                        "open-deferred-records"
                    } else {
                        "open-control-records"
                    },
                    cause,
                )
            })?;
            for (row, result) in batch.iter().zip(results) {
                match result {
                    OpenResult::WaitingForHistoryAccess => {
                        recover(&hub.waiting_metadata).insert(row.record_id.clone());
                    }
                    OpenResult::Ready(record) => {
                        if deferred && row.purpose == DomainRecord::EDIT_KIND {
                            if record.record_id.as_str() != row.record_id
                                || record.order_key.as_str() != row.order_key
                                || record.author != row.author
                                || record.purpose != row.purpose
                                || record.signature != row.signature
                            {
                                recover(&hub.waiting_metadata).insert(row.record_id.clone());
                                continue;
                            }
                            let indexed = {
                                let _publication = hub.gate();
                                store
                                    .index_opened_edit(row, &record.canonical)
                                    .map_err(|_| storage_failed())?
                            };
                            if !indexed {
                                recover(&hub.waiting_metadata).remove(&row.record_id);
                                for affected in
                                    store.conflicted_documents().map_err(|_| storage_failed())?
                                {
                                    hub.publish_status(&affected);
                                }
                                continue;
                            }
                        }
                        if deferred {
                            hub.retry_deferred_record(&row.record_id, &record.canonical)
                                .map_err(|_| storage_failed())?;
                        } else {
                            store
                                .remember_opened(&row.record_id, &row.order_key, &record.canonical)
                                .map_err(|_| storage_failed())?;
                        }
                        recover(&hub.waiting_metadata).remove(&row.record_id);
                    }
                    OpenResult::WrongScope | OpenResult::Corrupt => {
                        let mut waiting = recover(&hub.waiting_metadata);
                        if deferred && row.purpose == DomainRecord::EDIT_KIND {
                            waiting.insert(row.record_id.clone());
                        } else {
                            waiting.remove(&row.record_id);
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Re-opens the sealed metadata records the listing needs, and retries deferred ones.
async fn rehydrate(
    store: &DocumentStore,
    log: &RecordLog,
    hub: &BodyHub,
    cancellation: &CancellationToken,
) -> Result<(), BackendError> {
    // Index *all* authenticated applied and deferred edits before any body can be imported
    // or a deferred target retried. Neither arrival order nor v4 row order picks a winner.
    let reader = store.clone();
    let candidates = teamarness_package_sdk::blocking(cancellation, move |_| {
        reader.identity_candidates().map_err(|_| storage_failed())
    })
    .await?;
    for batch in open_batches(&candidates) {
        let sealed: Vec<Vec<u8>> = batch.iter().map(|row| row.sealed_record.clone()).collect();
        let opened = log
            .open(&sealed)
            .await
            .map_err(|cause| capability_error("index-edit-records", cause))?;
        if opened.len() != batch.len() {
            return Err(error("documents.document_pending", true));
        }
        for (row, result) in batch.iter().zip(opened) {
            match result {
                OpenResult::Ready(record)
                    if record.record_id.as_str() == row.record_id
                        && record.order_key.as_str() == row.order_key
                        && record.author == row.author
                        && record.purpose == row.purpose
                        && record.signature == row.signature =>
                {
                    let _publication = hub.gate();
                    store
                        .index_opened_edit(row, &record.canonical)
                        .map_err(|_| storage_failed())?;
                }
                // Applied rows are counted by import_edit_rows; deferred rows are
                // tracked by reopen_metadata until they can be opened.
                _ => {}
            }
        }
    }
    reopen_metadata(store, log, hub, cancellation, false).await?;
    // Rebuild every body from its genesis and edit records; a Document whose records cannot be
    // opened yet stays pending until `documents.document.open` retries it.
    let reader = store.clone();
    let edits = teamarness_package_sdk::blocking(cancellation, move |_| {
        reader.body_records(None).map_err(|_| storage_failed())
    })
    .await?;
    let mut by_document: HashMap<String, Vec<crate::store::StoredRecord>> = HashMap::new();
    for row in edits {
        let Some(document) = row.target.clone() else {
            continue;
        };
        by_document.entry(document).or_default().push(row);
    }
    for (document, rows) in by_document {
        hub.with_body(&document, |body| body.set_unopened(rows.len()));
        let unopened = import_edit_rows(log, hub, &rows).await?;
        hub.with_body(&document, |body| body.set_unopened(unopened));
    }
    hub.refresh_deferred_waiting()?;
    hub.refresh_identity_index_complete();
    Ok(())
}

/// Appends staged local updates as `document.edit` records.
///
/// One task per activation wakes when an update is staged, and retries every
/// [`FLUSH_RETRY`] while an append that failed is still staged.
fn start_flusher(
    log: &RecordLog,
    hub: &BodyHub,
    changes: &watch::Sender<u64>,
    author: String,
) -> Session {
    let log = log.clone();
    let hub = hub.clone();
    let changes = changes.clone();
    let cancellation = CancellationToken::new();
    let token = cancellation.clone();
    let task = tokio::spawn(async move {
        loop {
            // Ingest takes the store lock and then the bodies lock, so the bodies lock is
            // released before the store is asked anything.
            let staged: Vec<String> = hub
                .lock()
                .iter()
                .filter(|(_, body)| {
                    body.has_pending()
                        && (body.pending_request_id().is_none()
                            || hub.qualified.load(Ordering::Acquire))
                })
                .map(|(id, _)| id.clone())
                .collect();
            let pending: Vec<String> = staged
                .into_iter()
                .filter(|id| {
                    !hub.store.identity_conflicted(id).unwrap_or(true)
                        && hub
                            .store
                            .import_failure(id)
                            .is_ok_and(|failure| failure.is_none())
                })
                .collect();
            if pending.is_empty() {
                tokio::select! {
                    () = hub.flush.notified() => {}
                    () = token.cancelled() => return,
                }
                continue;
            }
            for document in pending {
                let append_lock = hub.append_lock(&document);
                let _guard = append_lock.lock().await;
                if let Err(cause) = drain_document(&hub, &document, |update| {
                    append_edit(&log, &hub, &changes, &author, &document, update)
                })
                .await
                {
                    eprintln!(
                        "documents edit append failure: document={document} code={} retryable={}",
                        cause.code(),
                        cause.retryable()
                    );
                }
            }
            tokio::select! {
                () = tokio::time::sleep(FLUSH_RETRY) => {}
                () = token.cancelled() => return,
            }
        }
    });
    Session { cancellation, task }
}

/// The caller serializes append attempts; failed exports remain at the front of the queue.
async fn drain_document<F, Fut>(
    hub: &BodyHub,
    document: &str,
    mut append: F,
) -> Result<(), BackendError>
where
    F: FnMut(Vec<u8>) -> Fut,
    Fut: Future<Output = Result<(), BackendError>>,
{
    let mut appended = false;
    let mut completed_id: Option<String> = None;
    loop {
        if hub
            .store
            .identity_conflicted(document)
            .map_err(|_| storage_failed())?
        {
            return Err(error("documents.identity_conflict", false));
        }
        if hub
            .store
            .import_failure(document)
            .map_err(|_| storage_failed())?
            .is_some()
        {
            return Err(error("documents.import_failure", false));
        }
        let next = {
            let _publication = hub.gate();
            if hub.with_body(document, |body| body.pending_request_id().is_some())
                && !hub.qualified.load(Ordering::Acquire)
            {
                return Err(error("documents.document_pending", true));
            }
            if hub
                .store
                .import_failure(document)
                .map_err(|_| storage_failed())?
                .is_some()
            {
                return Err(error("documents.import_failure", false));
            }
            hub.with_body(document, |body| body.staged())
        };
        let Some(update) = next else {
            if appended {
                let saved = hub.with_body(document, |body| {
                    completed_id
                        .as_deref()
                        .is_some_and(|id| body.identity(id).is_some())
                });
                if saved {
                    let _publication = hub.gate();
                    if hub
                        .store
                        .identity_conflicted(document)
                        .map_err(|_| storage_failed())?
                    {
                        return Err(error("documents.identity_conflict", false));
                    }
                    if hub
                        .store
                        .import_failure(document)
                        .map_err(|_| storage_failed())?
                        .is_some()
                    {
                        return Err(error("documents.import_failure", false));
                    }
                    let _ = hub.events.send(BodyEvent::Status {
                        document: document.to_owned(),
                        durable: completed_id,
                    });
                }
            }
            return Ok(());
        };
        let id = hub.with_body(document, |body| {
            body.pending_request_id().map(str::to_owned)
        });
        append(update).await?;
        if hub
            .store
            .identity_conflicted(document)
            .map_err(|_| storage_failed())?
        {
            return Err(error("documents.identity_conflict", false));
        }
        let _publication = hub.gate();
        if hub
            .store
            .import_failure(document)
            .map_err(|_| storage_failed())?
            .is_some()
        {
            hub.with_body(document, DocumentBody::append_succeeded);
            return Err(error("documents.import_failure", false));
        }
        completed_id = id;
        hub.with_body(document, DocumentBody::append_succeeded);
        appended = true;
    }
}

async fn append_edit(
    log: &RecordLog,
    hub: &BodyHub,
    changes: &watch::Sender<u64>,
    author: &str,
    document: &str,
    update: Vec<u8>,
) -> Result<(), BackendError> {
    let identity = hub.with_body(document, |body| {
        body.pending_request_id()
            .and_then(|id| body.identity(id).map(|known| (id.to_owned(), known.digest)))
    });
    // Every staged update was accepted under a request identity.
    let Some((id, digest)) = identity.as_ref() else {
        return Err(error("documents.edit_failed", true));
    };
    let record = DomainRecord::identified_document_edit(author, document, id, digest, update)
        .map_err(domain_error)?;
    if hub
        .store
        .identity_conflicted(document)
        .map_err(|_| storage_failed())?
    {
        return Err(error("documents.identity_conflict", false));
    }
    let purpose = record.kind();
    let canonical = record.encode();
    let appended = log.append(purpose, &canonical).await.map_err(|cause| {
        if cause.retryable() {
            hub.qualified.store(false, Ordering::Release);
        }
        capability_error(purpose, cause)
    })?;
    // From the moment Core accepts the bytes until projection commits, recovery must not
    // mistake the local preview for an imported body. A replay may commit while SQLite
    // projection is in flight; only that commit may clear this flag.
    if let Some((id, digest)) = &identity {
        let _publication = hub.gate();
        hub.remember_identity(document, id, author, *digest, true);
        hub.with_body(document, |body| body.set_projection_pending(id, true));
    }
    let changes = changes.clone();
    let classifier = hub.clone();
    let ingested = tokio::task::spawn_blocking(move || {
        classifier
            .ingest_record(
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
            .map_err(|_| storage_failed())
    })
    .await;
    // Core has durably accepted the exact update. A projection error must not append it twice;
    // activation replay rebuilds the projection from the sealed Core record.
    match ingested {
        Ok(Ok(ingested)) => {
            if matches!(ingested.result, IngestResult::IdentityConflict(_)) {
                hub.qualified.store(false, Ordering::Release);
                hub.with_body(document, DocumentBody::append_succeeded);
                return Err(error("documents.request_id_conflict", false));
            }
            if matches!(
                ingested.result,
                IngestResult::ImportFailure(_) | IngestResult::Quarantined(_)
            ) || hub
                .store
                .import_failure(document)
                .map_or(true, |failure| failure.is_some())
            {
                hub.with_body(document, DocumentBody::append_succeeded);
                hub.publish_status(document);
                return Err(error("documents.import_failure", false));
            }
            if hub.store.identity_conflicted(document).unwrap_or(true) {
                return Err(error("documents.identity_conflict", false));
            }
            if let Some((id, digest)) = &identity {
                hub.remember_identity(document, id, author, *digest, true);
                hub.with_body(document, |body| body.set_projection_pending(id, false));
            }
            // Ingest installed successful candidates only after the store committed.
        }
        Ok(Err(cause)) => {
            hub.with_body(document, DocumentBody::append_succeeded);
            eprintln!(
                "documents edit projection failure after append: {}",
                cause.code()
            );
            return Err(cause);
        }
        Err(_) => {
            hub.with_body(document, DocumentBody::append_succeeded);
            return Err(storage_failed());
        }
    }
    Ok(())
}

fn is_request_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The UI-facing error for an update the body refused; nothing was staged.
fn update_error(cause: BodyError) -> BackendError {
    match cause {
        BodyError::ForeignPeer => error("documents.foreign_peer", false),
        BodyError::RecordTooLarge => error("documents.edit_request_too_large", false),
        BodyError::Schema(_) => error("documents.schema_violation", false),
        BodyError::TooLarge => error("documents.document_too_large", false),
        BodyError::InvalidUpdate => error("documents.invalid_update", false),
        BodyError::BaseUnavailable => error("documents.base_unavailable", false),
        BodyError::NotReady => error("documents.document_pending", true),
        BodyError::IdentityMismatch => invalid_request(),
        BodyError::Loro => error("documents.edit_failed", true),
    }
}

/// What one local write asks of a body.
#[derive(Clone, Copy)]
enum LocalWrite<'a> {
    /// An editor session's update, holding operations only from the session's peer.
    Update { peer: u64, bytes: &'a [u8] },
    /// A Markdown revision made against `base`, merged as one update from a fresh peer.
    Revision {
        base: &'a Frontiers,
        markdown: &'a str,
    },
}

/// A local write the body accepted. The Document's append lock is held until it is dropped, so
/// the caller drains exactly this write before any other local write is staged.
struct Staged {
    revision: u64,
    /// The body's version once the write was staged; it includes the write.
    version: Frontiers,
    /// `false` when a revision changed nothing: nothing was staged and no record follows.
    changed: bool,
    _append: tokio::sync::OwnedMutexGuard<()>,
    _identity: tokio::sync::OwnedMutexGuard<()>,
}

/// Validates one local write against the body and stages it as the next identified
/// `document.edit` record. The caller drains the staged record.
///
/// The request ID and the payload digest identify the record. A retry of the same ID and bytes
/// after an unknown outcome is answered from what is known, and never staged twice; a new ID
/// is accepted only once a qualified replay can prove it absent. A revision always carries a
/// fresh ID, and its digest is taken from the update the merge produced.
async fn stage_local_write(
    log: &RecordLog,
    hub: &BodyHub,
    author: &str,
    document: &str,
    request_id: &str,
    write: LocalWrite<'_>,
) -> Result<Staged, BackendError> {
    if !is_request_id(request_id) {
        return Err(invalid_request());
    }
    let append_guard = hub.append_lock(document).lock_owned().await;
    if hub
        .store
        .identity_conflicted(document)
        .map_err(|_| storage_failed())?
    {
        return Err(error("documents.identity_conflict", false));
    }
    if hub
        .store
        .import_failure(document)
        .map_err(|_| storage_failed())?
        .is_some()
    {
        return Err(error("documents.import_failure", false));
    }
    let digest: Option<[u8; 32]> = match write {
        LocalWrite::Update { bytes, .. } => Some(Sha256::digest(bytes).into()),
        LocalWrite::Revision { .. } => None,
    };
    // A revision is merged first, on a blocking thread and holding nothing but this Document's
    // append lock, so a slow merge delays only this write.
    let mut merged = match write {
        LocalWrite::Revision { base, markdown } => {
            Some(merge_off_lock(hub, document, base, markdown).await?)
        }
        LocalWrite::Update { .. } => None,
    };
    let identity_guard = hub.identity_lookup_lock.clone().lock_owned().await;
    let found = lookup_identities(log, &hub.store, hub, &[request_id.to_owned()]).await?;
    if let Some((old_document, old_author, old_digest, durable)) = found.get(request_id) {
        let Some(digest) = digest else {
            // A revision's ID is fresh; one that already names a record is never reused.
            return Err(invalid_request());
        };
        if old_document != document || old_author != author || old_digest != &digest {
            return Err(invalid_request());
        }
        hub.remember_identity(document, request_id, author, digest, *durable);
    }
    // A new ID is absent only after the fenced, gap-free from-start replay and
    // successful opening of every relevant prior edit.
    if found.is_empty()
        && !hub
            .lock()
            .values()
            .any(|body| body.identity(request_id).is_some())
        && !hub.qualified.load(Ordering::Acquire)
    {
        return Err(error("documents.document_pending", true));
    }
    // One ID cannot name a request on another Document or Installation.
    if hub
        .lock()
        .iter()
        .any(|(id, body)| id != document && body.identity(request_id).is_some())
    {
        return Err(invalid_request());
    }
    let publication = hub.gate();
    if hub
        .store
        .import_failure(document)
        .map_err(|_| storage_failed())?
        .is_some()
    {
        return Err(error("documents.import_failure", false));
    }
    let (revision, version, staged, changed) = hub.with_body(document, |body| {
        if !body.editable() {
            return Err(error("documents.document_pending", true));
        }
        let digest = match (write, digest) {
            (LocalWrite::Update { peer, bytes }, Some(digest)) => {
                if let Some(known) = body.identity(request_id) {
                    if known.author != author || known.digest != digest {
                        return Err(invalid_request());
                    }
                    return Ok((known.revision, body.frontiers(), None, true));
                }
                body.accept_local_update(peer, request_id.to_owned(), bytes)
                    .map_err(update_error)?;
                digest
            }
            (LocalWrite::Revision { .. }, _) => {
                if body.identity(request_id).is_some() {
                    return Err(invalid_request());
                }
                let Some((peer, merged)) = merged.take() else {
                    return Err(error("documents.edit_failed", true));
                };
                if !body
                    .stage_merged(peer, request_id.to_owned(), &merged)
                    .map_err(update_error)?
                {
                    return Ok((body.revision(), body.frontiers(), None, false));
                }
                let update = body
                    .staged()
                    .ok_or_else(|| error("documents.edit_failed", true))?;
                Sha256::digest(update).into()
            }
            (LocalWrite::Update { .. }, None) => return Err(invalid_request()),
        };
        body.remember_identity(request_id.to_owned(), author.to_owned(), digest, false)
            .map_err(|_| invalid_request())?;
        Ok((body.revision(), body.frontiers(), body.staged(), true))
    })?;
    if staged.is_none()
        && changed
        && hub.with_body(document, |body| {
            body.identity(request_id)
                .is_some_and(|known| known.projection_pending)
        })
    {
        return Err(error("documents.document_pending", true));
    }
    if let Some(update) = staged {
        hub.flush.notify_one();
        // Other sessions see the accepted update at once; its own session already holds it.
        let origin = match write {
            LocalWrite::Update { peer, .. } => Some(peer),
            LocalWrite::Revision { .. } => None,
        };
        hub.publish_update(document, update, origin);
    }
    drop(publication);
    Ok(Staged {
        revision,
        version,
        changed,
        _append: append_guard,
        _identity: identity_guard,
    })
}

/// Merges a Markdown revision of `base` into a fork of the body of `document`, as a fresh peer,
/// on a blocking thread. The bodies lock is held only to fork; the merge holds no lock, so other
/// Documents keep reading, writing and publishing while it runs. Returns the peer and the merge.
async fn merge_off_lock(
    hub: &BodyHub,
    document: &str,
    base: &Frontiers,
    markdown: &str,
) -> Result<(u64, crate::markdown::Merged), BackendError> {
    let (source, peer) = hub
        .with_body(document, |body| body.merge_source())
        .map_err(update_error)?;
    let base = base.clone();
    let markdown = markdown.to_owned();
    #[cfg(test)]
    let pause = tests::merge_pause(document);
    let merged = tokio::task::spawn_blocking(move || {
        #[cfg(test)]
        if let Some((started, release)) = pause {
            let _ = started.send(());
            let _ = release.recv();
        }
        crate::body::merge_revision(&source, &base, &markdown, peer)
    })
    .await
    .map_err(|_| error("documents.edit_failed", true))?
    .map_err(update_error)?;
    Ok((peer, merged))
}

/// Waits until every staged update of `document` has reached the Core record log.
async fn drain_local_writes(
    log: &RecordLog,
    hub: &BodyHub,
    changes: &watch::Sender<u64>,
    author: &str,
    document: &str,
) -> Result<(), BackendError> {
    drain_document(hub, document, |staged| {
        append_edit(log, hub, changes, author, document, staged)
    })
    .await
}

/// The one local write path for an editor session: validates `update` from `session_peer`
/// against the body, then appends it as exactly one identified `document.edit` record and
/// waits for that append. The Markdown port reaches the body through [`stage_local_write`].
#[allow(clippy::too_many_arguments)]
async fn submit_update(
    log: &RecordLog,
    hub: &BodyHub,
    changes: &watch::Sender<u64>,
    author: &str,
    document: &str,
    session_peer: u64,
    request_id: &str,
    update: &[u8],
) -> Result<JsonValue, BackendError> {
    let staged = stage_local_write(
        log,
        hub,
        author,
        document,
        request_id,
        LocalWrite::Update {
            peer: session_peer,
            bytes: update,
        },
    )
    .await?;
    // Never acknowledge the update until its record has reached the Core record log.
    drain_local_writes(log, hub, changes, author, document).await?;
    Ok(json!({ "revision": staged.revision.to_string(), "requestId": request_id }))
}

async fn stop_flusher(state: &DocumentsActive) {
    let flusher = state
        .flusher
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    if let Some(flusher) = flusher {
        flusher.cancellation.cancel();
        let _ = flusher.task.await;
    }
    let documents: Vec<String> = state
        .hub
        .lock()
        .iter()
        .filter(|(_, body)| body.has_pending())
        .map(|(id, _)| id.clone())
        .collect();
    for document in documents {
        let append_lock = state.hub.append_lock(&document);
        let _guard = append_lock.lock().await;
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            drain_document(&state.hub, &document, |update| {
                append_edit(
                    &state.log,
                    &state.hub,
                    &state.changes,
                    &state.installation,
                    &document,
                    update,
                )
            }),
        )
        .await;
        if !matches!(result, Ok(Ok(()))) {
            eprintln!("documents pending edit not drained on shutdown: document={document}");
        }
    }
}

/// Starts the record log session that ingests everything the host delivers.
fn start_session(
    store: &DocumentStore,
    log: &RecordLog,
    sync: &Arc<Mutex<SyncReport>>,
    changes: &watch::Sender<u64>,
    hub: &BodyHub,
) -> Session {
    let sink = Arc::new(DocumentsSink {
        store: store.clone(),
        changes: changes.clone(),
        sync: sync.clone(),
        hub: hub.clone(),
        log: log.clone(),
        cancellation: CancellationToken::new(),
    });
    let cancellation = sink.cancellation.clone();
    let task = tokio::spawn(run_session_from_start(
        log.clone(),
        sink,
        cancellation.clone(),
        SessionOptions::default(),
    ));
    Session { cancellation, task }
}

/// Cancels the session and waits for it to finish, so no ingest outlives the activation.
async fn stop_session(state: &DocumentsActive) {
    let session = state
        .session
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    if let Some(session) = session {
        session.cancellation.cancel();
        let _ = session.task.await;
    }
}

/// Where the SDK's record log session commits what it receives.
///
/// Every method runs on the blocking pool, and the session credits a delivery only after
/// `ingest` has returned, so a record whose ingest fails is delivered again rather than skipped.
pub(crate) struct DocumentsSink {
    store: DocumentStore,
    changes: watch::Sender<u64>,
    sync: Arc<Mutex<SyncReport>>,
    hub: BodyHub,
    log: RecordLog,
    cancellation: CancellationToken,
}

impl DocumentsSink {
    fn with_report(&self, change: impl FnOnce(&mut SyncReport)) {
        let mut report = self
            .sync
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        change(&mut report);
    }
}

impl RecordLogSink for DocumentsSink {
    fn committed_cursor(&self) -> Result<Option<Cursor>, BackendError> {
        let stored = self.store.cursor().map_err(|_| storage_failed())?;
        Ok(stored.and_then(|value| Cursor::parse(value).ok()))
    }

    fn ingest(&self, record: &RecordEvent) -> Result<(), BackendError> {
        let ingested = self
            .hub
            .ingest_record(
                record.record_id.as_str(),
                record.order_key.as_str(),
                &record.author,
                &record.purpose,
                &record.signature,
                &record.canonical,
                &record.sealed_record,
                Some(record.cursor.as_str()),
                &|| self.changes.send_modify(|x| *x = x.saturating_add(1)),
            )
            .map_err(|_| storage_failed())?;
        if let IngestResult::IdentityConflict(ref document) = ingested.result {
            eprintln!("documents identity conflict quarantined: document={document}");
            for affected in self
                .store
                .conflicted_documents()
                .map_err(|_| storage_failed())?
            {
                self.hub.publish_status(&affected);
            }
            self.store
                .note_replayed(record.record_id.as_str())
                .map_err(|_| storage_failed())?;
            self.with_report(|report| {
                let code = "request_id_conflict".to_owned();
                if !report.warnings.contains(&code) && report.warnings.len() < MAX_WARNINGS {
                    report.warnings.push(code);
                }
            });
            return Ok(());
        }
        if matches!(ingested.result, IngestResult::Quarantined(_)) {
            // A sealed invalid record is handled; it must not halt unrelated delivery.
            self.store
                .note_replayed(record.record_id.as_str())
                .map_err(|_| storage_failed())?;
            return Ok(());
        }
        if matches!(
            ingested.result,
            IngestResult::Applied | IngestResult::Duplicate | IngestResult::ImportFailure(_)
        ) {
            self.store
                .note_replayed(record.record_id.as_str())
                .map_err(|_| storage_failed())?;
        }
        if matches!(
            ingested.result,
            IngestResult::Applied | IngestResult::Duplicate
        ) && let Ok(DomainRecord::IdentifiedDocumentEdit {
            author,
            document,
            request_id,
            payload_digest,
            ..
        }) = DomainRecord::decode(&record.canonical)
        {
            self.hub
                .remember_identity(&document, &request_id, &author, payload_digest, true);
            if matches!(ingested.result, IngestResult::Applied) {
                self.hub.with_body(&document, |body| {
                    body.set_projection_pending(&request_id, false)
                });
            }
        }
        Ok(())
    }

    fn reset_cursor(&self) -> Result<(), BackendError> {
        self.store.reset_cursor().map_err(|_| storage_failed())
    }

    fn on_qualified_replay(&self, _boundary: &ReplayEnd) {
        // Core's gap-free prefix is independent of unrelated Documents' ability to open
        // their sealed edits. The local/recovery paths additionally require the target
        // Document's own body and identity history to be ready before proving absence.
        self.hub.qualified.store(
            self.store.replayed_previous_projection().unwrap_or(false),
            Ordering::Release,
        );
        if self.hub.qualified.load(Ordering::Acquire) {
            self.hub.flush.notify_one();
        }
    }

    fn on_replay_invalidated(&self) {
        let _bodies = self.hub.lock();
        self.hub.qualified.store(false, Ordering::Release);
    }

    fn on_state(&self, state: &SyncState) {
        self.with_report(|report| report.availability = state.availability);
        // History access can arrive while availability remains `current`; every session state
        // observation gives pending editors another opportunity to open sealed edits.
        self.changes
            .send_modify(|revision| *revision = revision.saturating_add(1));
        if recover(&self.hub.waiting_metadata).is_empty() {
            return;
        }
        let store = self.store.clone();
        let log = self.log.clone();
        let hub = self.hub.clone();
        let cancellation = self.cancellation.clone();
        let changes = self.changes.clone();
        tokio::spawn(async move {
            if reopen_metadata(&store, &log, &hub, &cancellation, true)
                .await
                .is_ok()
                && let Ok(changed) = hub.refresh_deferred_waiting()
            {
                hub.refresh_identity_index_complete();
                for document in changed {
                    hub.publish_status(&document);
                }
                changes.send_modify(|revision| *revision = revision.saturating_add(1));
            }
        });
    }

    fn on_warning(&self, warning: &SyncWarning) {
        self.with_report(|report| {
            if !report.warnings.iter().any(|code| code == &warning.code)
                && report.warnings.len() < MAX_WARNINGS
            {
                report.warnings.push(warning.code.clone());
            }
        });
    }

    fn on_session_error(&self, error: &BackendError) {
        eprintln!(
            "documents session failure: code={} retryable={}",
            error.code(),
            error.retryable()
        );
        if error.retryable() {
            return;
        }
        let code = error.code().to_owned();
        self.with_report(|report| {
            report.availability = Availability::Offline;
            if !report.warnings.iter().any(|entry| entry == &code)
                && report.warnings.len() < MAX_WARNINGS
            {
                report.warnings.push(code);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use teamarness_package_contract::ui_contract::MAX_UI_PAYLOAD_BYTES;
    use teamarness_package_sdk::testing::assert_router_matches_contract;

    #[test]
    fn all_replay_paths_partition_seventeen_large_sealed_records() {
        let rows: Vec<crate::store::StoredRecord> = (0..17)
            .map(|seed| crate::store::StoredRecord {
                record_id: hex(64, seed),
                order_key: hex(82, seed),
                author: ALICE.into(),
                purpose: "document.edit".into(),
                signature: "sig".into(),
                sealed_record: vec![b'x'; 32 * 1024],
                cursor: None,
                ingestion_ordinal: seed,
                target: None,
            })
            .collect();
        let batches: Vec<_> = open_batches(&rows).collect();
        assert_eq!(batches.iter().map(|batch| batch.len()).sum::<usize>(), 17);
        assert!(batches.len() >= 2);
        for batch in batches {
            assert!(batch.len() <= 32);
            assert!(
                batch
                    .iter()
                    .map(|row| row.sealed_record.len())
                    .sum::<usize>()
                    <= 512 * 1024
            );
        }
        let mut small = vec![rows[0].clone(); 33];
        for row in &mut small {
            row.sealed_record = vec![1];
        }
        assert_eq!(
            open_batches(&small)
                .map(|batch| batch.len())
                .collect::<Vec<_>>(),
            vec![32, 1]
        );
    }

    fn test_hub(store: DocumentStore, events: broadcast::Sender<BodyEvent>) -> BodyHub {
        BodyHub {
            store,
            bodies: Arc::new(Mutex::new(HashMap::new())),
            installation: ALICE.into(),
            events,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            ended_sessions: Arc::new(Mutex::new(HashMap::new())),
            flush: Arc::new(Notify::new()),
            append_locks: Arc::new(Mutex::new(HashMap::new())),
            waiting_metadata: Arc::new(Mutex::new(HashSet::new())),
            metadata_lock: Arc::new(tokio::sync::Mutex::new(())),
            qualified: Arc::new(AtomicBool::new(false)),
            identity_index_complete: Arc::new(AtomicBool::new(false)),
            identity_lookup_lock: Arc::new(tokio::sync::Mutex::new(())),
            publication_gate: Arc::new(Mutex::new(())),
        }
    }

    fn ui_contract() -> teamarness_package_contract::ui_contract::CompiledUiContract {
        teamarness_package_contract::ui_contract::compile_ui_contract(include_bytes!(
            "../../ui-contract.json"
        ))
        .unwrap()
    }

    /// The snapshot a session opening carries, reassembled from its chunks.
    fn reassembled(opening: &[JsonValue]) -> Vec<u8> {
        assert_eq!(opening[0]["event"], "begin");
        assert_eq!(opening.last().unwrap()["event"], "end");
        let chunks = &opening[1..opening.len() - 1];
        assert_eq!(opening[0]["chunks"], chunks.len());
        let mut snapshot = Vec::new();
        for (index, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk["event"], "chunk");
            assert_eq!(chunk["chunkIndex"], index);
            snapshot.extend(STANDARD.decode(chunk["chunk"].as_str().unwrap()).unwrap());
        }
        snapshot
    }

    /// The Canonical Markdown of a snapshot, as the UI's doc would hold it.
    fn markdown_of(snapshot: &[u8]) -> String {
        let doc = loro::LoroDoc::new();
        if !snapshot.is_empty() {
            doc.import(snapshot).unwrap();
        }
        crate::markdown::export(&doc).unwrap_or_default()
    }

    // Verifies: TEAM.SYS.DOCS-R05, TEAM.SYS.DOCS-R09 — the snapshot reaches a session in chunks
    // that reassemble to the body, and no session event, including the largest possible update,
    // exceeds one UI message.
    #[test]
    fn session_events_reassemble_and_never_exceed_one_ui_message() {
        let directory = tempfile::tempdir().unwrap();
        let store = DocumentStore::open(directory.path().join("documents.sqlite3")).unwrap();
        let (events, mut receiver) = broadcast::channel(256);
        let hub = test_hub(store, events);
        let mut alice = Writer::new();
        let mut bob = Writer::new();
        let from_alice = alice.write(&format!("{}\n", unique_text(110 * 1024)));
        let from_bob = bob.write(&format!("{}\n", "\u{0001}😀".repeat(20_000)));
        hub.apply_edits(&[("doc".into(), genesis())]);
        hub.apply_edits(&[("doc".into(), from_alice)]);
        hub.apply_edits(&[("doc".into(), from_bob)]);
        let merged = hub.with_body("doc", |body| body.text().to_owned());
        assert!(merged.len() > crate::body::MAX_TEXT_BYTES);
        let contract = ui_contract();
        let document = hex(64, 7);
        hub.apply_edits(&[(document.clone(), genesis())]);
        let (_, opening) = hub.session_opening("doc", 42);
        assert!(opening.len() > 3, "a large body arrives in several chunks");
        assert_eq!(markdown_of(&reassembled(&opening)), merged);
        let mut published = Vec::new();
        while let Ok(BodyEvent::Update { update, .. }) = receiver.try_recv() {
            published.push(json!({
                "event": "update", "documentId": document,
                "update": STANDARD.encode(update.as_slice()),
            }));
        }
        assert!(!published.is_empty());
        // The largest update a session can receive: exactly one record's worth.
        let largest = json!({
            "event": "update", "documentId": document,
            "update": STANDARD.encode(vec![0xa5; MAX_UPDATE]),
        });
        for event in opening.iter().chain(&published).chain([&largest]) {
            let mut event = event.clone();
            event["documentId"] = json!(document);
            assert!(event.to_string().len() <= MAX_UI_PAYLOAD_BYTES);
            contract
                .validate_event("documents.document.session", &event)
                .unwrap_or_else(|error| panic!("{error:?}: {}", event["event"]));
        }
        assert!(largest.to_string().len() > 200 * 1024, "the bound is tight");
        // One byte more never reaches a session as an update: every session reloads instead.
        hub.publish_update("doc", vec![0; MAX_UPDATE + 1], None);
        assert!(matches!(
            receiver.try_recv(),
            Ok(BodyEvent::Resync { document }) if document == "doc"
        ));
        hub.publish_update("doc", vec![0; MAX_UPDATE], None);
        assert!(matches!(receiver.try_recv(), Ok(BodyEvent::Update { .. })));
    }

    // Verifies: TEAM.SYS.DOCS-R11 — the Markdown request delivers the Canonical Markdown in bounded
    // chunks of one text.
    #[test]
    fn markdown_chunks_reassemble_to_the_canonical_export_within_one_ui_message() {
        let directory = tempfile::tempdir().unwrap();
        let store = DocumentStore::open(directory.path().join("documents.sqlite3")).unwrap();
        let (events, _receiver) = broadcast::channel(256);
        let hub = test_hub(store, events);
        let document = hex(64, 7);
        let mut writer = Writer::new();
        let large = writer.write(&format!("# Plan\n\n{}\n", "\u{0001}😀 ".repeat(20_000)));
        hub.apply_edits(&[(document.clone(), genesis()), (document.clone(), large)]);
        let contract = ui_contract();
        let first = markdown_chunk(&hub, &document, 0).unwrap();
        let chunks = first["chunks"].as_u64().unwrap() as usize;
        assert!(chunks > 4);
        let mut markdown = String::new();
        for index in 0..chunks {
            let chunk = markdown_chunk(&hub, &document, index).unwrap();
            assert_eq!(chunk["digest"], first["digest"]);
            assert!(chunk.to_string().len() <= MAX_UI_PAYLOAD_BYTES);
            contract
                .validate_response("documents.document.markdown", &chunk)
                .unwrap();
            markdown.push_str(chunk["chunk"].as_str().unwrap());
        }
        assert_eq!(
            markdown,
            hub.with_body(&document, |body| body.text().to_owned())
        );
        assert_eq!(
            markdown_chunk(&hub, &document, chunks).unwrap_err().code(),
            "documents.invalid_request"
        );
        let empty = hex(64, 8);
        hub.apply_edits(&[(empty.clone(), genesis())]);
        let only = markdown_chunk(&hub, &empty, 0).unwrap();
        assert_eq!(
            (only["chunks"].clone(), only["chunk"].clone()),
            (json!(1), json!(""))
        );
    }

    // Verifies: TEAM.SYS.DOCS-R05 — a Document whose records cannot all be opened is pending, never
    // partial.
    #[test]
    fn pending_body_opens_a_session_without_a_partial_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let store = DocumentStore::open(directory.path().join("documents.sqlite3")).unwrap();
        let (events, mut receiver) = broadcast::channel(16);
        let hub = test_hub(store, events);
        let update = Writer::new().write("complete\n");
        hub.apply_edits(&[("doc".into(), genesis())]);
        hub.with_body("doc", |body| body.set_unopened(2));
        while receiver.try_recv().is_ok() {}
        hub.apply_edits(&[("doc".into(), update.clone())]);
        let (access, opening) = hub.session_opening("doc", 1);
        assert!(!access.editable);
        assert_eq!(opening[0]["editable"], false);
        assert_eq!(opening[0]["chunks"], 0, "no partial body is sent");
        assert_eq!(hub.access_value("doc")["editable"], false);
        assert!(matches!(
            markdown_chunk(&hub, "doc", 0),
            Err(cause) if cause.code() == "documents.document_pending"
        ));
        hub.with_body("doc", |body| body.set_unopened(0));
        let (access, opening) = hub.session_opening("doc", 2);
        assert!(access.editable);
        assert_eq!(markdown_of(&reassembled(&opening)), "complete\n");
    }

    #[test]
    fn router_matches_the_ui_contract() {
        assert_router_matches_contract(
            &router().operations(),
            include_str!("../../ui-contract.json"),
        );
    }

    use std::time::Duration;

    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use teamarness_package_sdk::testing::{Harness, HarnessBuilder, ScriptedCapability};

    const RECORD_LOG: &str = "teamarness.synchronization.immutable-record-log";
    const ALICE: &str = "alice";

    fn hex(length: usize, seed: u64) -> String {
        format!("{seed:0length$x}")
    }

    /// One genesis shared by every test Document created from a record, so a [`Writer`] made
    /// without a body writes updates any of them can import.
    static TEST_GENESIS: std::sync::LazyLock<Vec<u8>> =
        std::sync::LazyLock::new(crate::body::genesis);

    fn genesis() -> Vec<u8> {
        TEST_GENESIS.clone()
    }

    /// Another Installation's editor of one Document: each write revises the whole body to the
    /// given Markdown and returns the one update, from a fresh peer, that its record carries.
    struct Writer {
        body: DocumentBody,
    }

    impl Writer {
        /// A writer of a Document created with [`genesis`].
        fn new() -> Self {
            let mut body = DocumentBody::new();
            body.import(&TEST_GENESIS).unwrap();
            Self { body }
        }

        /// A writer of a Document this activation holds, starting from its current body.
        fn of(handles: &ActivationHandles, document: &str) -> Self {
            Self {
                body: handles.bodies.lock().unwrap()[document].writer_copy(),
            }
        }

        fn write(&mut self, markdown: &str) -> Vec<u8> {
            let base = self.body.frontiers();
            assert!(
                self.body
                    .merge_markdown(&base, markdown, hex(32, 0xab))
                    .unwrap()
            );
            let update = self.body.staged().unwrap();
            self.body.append_succeeded();
            self.body.import(&update).unwrap();
            update
        }
    }

    /// What one UI session would submit: the body's Markdown revised to `markdown`, as one
    /// update from a fresh session peer.
    fn session_update(
        handles: &ActivationHandles,
        document: &str,
        markdown: &str,
    ) -> (u64, Vec<u8>) {
        let bodies = handles.bodies.lock().unwrap();
        let body = &bodies[document];
        let peer = body.allocate_peer();
        (peer, body.revision_update(peer, markdown))
    }

    /// Submits a session update through the one local write path, as Alice's Installation.
    async fn submit(
        handles: &ActivationHandles,
        document: &str,
        request_id: &str,
        (peer, update): &(u64, Vec<u8>),
    ) -> Result<JsonValue, BackendError> {
        submit_update(
            &handles.log,
            &handles.hub,
            &handles.changes,
            ALICE,
            document,
            *peer,
            request_id,
            update,
        )
        .await
    }

    /// A capability that opens nothing, appends deterministically, and refuses the rest.
    fn scripted_log() -> ScriptedCapability {
        let appended = Arc::new(std::sync::atomic::AtomicU64::new(100));
        ScriptedCapability::new().calls(move |method, payload| match method {
            "open" => {
                let count = payload
                    .get("sealedRecords")
                    .and_then(JsonValue::as_array)
                    .map_or(0, Vec::len);
                Ok(json!({
                    "results": (0..count)
                        .map(|_| json!({ "status": "waiting_for_history_access" }))
                        .collect::<Vec<_>>(),
                }))
            }
            "append" => {
                let seed = appended.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(json!({
                    "recordId": hex(64, seed),
                    "orderKey": hex(82, seed),
                    "author": ALICE,
                    "signature": "c2ln",
                    "sealedRecord": "c2VhbGVk",
                    "durability": "committed",
                }))
            }
            _ => Err(BackendError::new(
                "capability_unavailable",
                false,
                json!({}),
            )),
        })
    }

    fn record_frame(seed: u64, author: &str, record: &DomainRecord) -> JsonValue {
        json!({
            "kind": "record",
            "cursor": hex(50, seed),
            "recordId": hex(64, seed),
            "orderKey": hex(82, seed),
            "author": author,
            "purpose": record.kind(),
            "signature": "c2ln",
            "canonical": URL_SAFE_NO_PAD.encode(record.encode()),
            "sealedRecord": "c2VhbGVk",
        })
    }

    struct Session {
        harness: Harness,
        handles: ActivationHandles,
        scripted: ScriptedCapability,
    }

    async fn start(directory: &std::path::Path, scripted: ScriptedCapability) -> Session {
        let observer: ActivationObserver = Arc::new(Mutex::new(None));
        let mut harness =
            HarnessBuilder::new(router_observing(Some(observer.clone())).into_backend())
                .package_identity("dev.teamarness.documents")
                .installation(ALICE, "Alice")
                .data_directory(directory)
                .capability(RECORD_LOG, scripted.clone())
                .capability("teamarness.principals", ScriptedCapability::new())
                .start()
                .await;
        harness.activate().await.unwrap();
        let handles = observer
            .lock()
            .unwrap()
            .clone()
            .expect("the activation published its handles");
        Session {
            harness,
            handles,
            scripted,
        }
    }

    async fn wait_until(mut ready: impl FnMut() -> bool, what: &str) {
        for _ in 0..600 {
            if ready() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("timed out waiting for {what}");
    }

    async fn qualify_empty_history(session: &Session) {
        let stream = session.scripted.stream();
        tokio::time::timeout(Duration::from_secs(2), stream.opened())
            .await
            .unwrap();
        assert!(
            stream
                .send(json!({"kind":"replay_end", "cursor":hex(50, 0), "gapFree":true}))
                .await
        );
        wait_until(
            || session.handles.qualified.load(Ordering::Acquire),
            "qualified initial replay",
        )
        .await;
    }

    // Verifies: AC1, AC4 — a local create is appended, projected, and answered with its identity.
    #[tokio::test]
    async fn local_creates_are_appended_projected_and_identified() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        let created = session
            .harness
            .request("documents.collection.create", json!({ "name": "Planning" }))
            .await
            .unwrap();
        let collection = created["id"].as_str().unwrap().to_owned();
        assert_eq!(collection, hex(64, 100));
        let document = session
            .harness
            .request(
                "documents.document.create",
                json!({ "collectionId": collection, "title": "Q4 plan" }),
            )
            .await
            .unwrap();
        assert_eq!(document["id"], hex(64, 101));
        let snapshot = session
            .harness
            .request("documents.snapshot", json!({}))
            .await
            .unwrap();
        assert_eq!(snapshot["collections"][0]["name"], "Planning");
        assert_eq!(snapshot["documents"][0]["title"], "Q4 plan");
        assert_eq!(snapshot["documents"][0]["collectionId"], collection);
        // The local echo never moves the delivery cursor.
        assert_eq!(session.handles.store.cursor().unwrap(), None);
        let unknown = session
            .harness
            .request(
                "documents.document.create",
                json!({ "collectionId": hex(64, 7), "title": "x" }),
            )
            .await
            .unwrap_err();
        assert_eq!(unknown.code(), "documents.unknown_collection");
        let invalid = session
            .harness
            .request("documents.collection.create", json!({ "name": " padded" }))
            .await
            .unwrap_err();
        assert_eq!(invalid.code(), "documents.invalid_name");
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn sink_opposite_arrival_orders_publish_conflict_without_stopping_other_documents() {
        let document = hex(64, 2);
        let other = hex(64, 3);
        let request_id = hex(32, 9);
        let mut edits = Vec::new();
        for (seed, text) in [(4, "one"), (5, "two")] {
            let update = Writer::new().write(&format!("{text}\n"));
            edits.push((
                seed,
                DomainRecord::identified_document_edit(
                    "bob",
                    &document,
                    &request_id,
                    &[seed as u8; 32],
                    update,
                )
                .unwrap(),
            ));
        }
        for reverse in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let mut session = start(directory.path(), scripted_log()).await;
            let stream = session.scripted.stream();
            tokio::time::timeout(Duration::from_secs(2), stream.opened())
                .await
                .unwrap();
            for (seed, record) in [
                (1, DomainRecord::collection_create("bob", "Plan").unwrap()),
                (
                    2,
                    DomainRecord::document_create("bob", &hex(64, 1), "Notes", genesis()).unwrap(),
                ),
                (
                    3,
                    DomainRecord::document_create("bob", &hex(64, 1), "Other", genesis()).unwrap(),
                ),
            ] {
                assert!(stream.send(record_frame(seed, "bob", &record)).await);
                let store = session.handles.store.clone();
                wait_until(
                    || store.cursor().unwrap().as_deref() == Some(hex(50, seed).as_str()),
                    "create",
                )
                .await;
            }
            let mut ordered = edits.clone();
            if reverse {
                ordered.reverse();
            }
            for (seed, record) in ordered {
                assert!(stream.send(record_frame(seed, "bob", &record)).await);
                let store = session.handles.store.clone();
                wait_until(
                    || store.cursor().unwrap().as_deref() == Some(hex(50, seed).as_str()),
                    "edit",
                )
                .await;
                if !store.identity_conflicted(&document).unwrap() {
                    assert!(
                        !session.handles.bodies.lock().unwrap()[&document]
                            .text()
                            .is_empty()
                    );
                }
            }
            let hidden = body_view(&mut session.harness, &document).await;
            assert_eq!(hidden["reason"], "identity_conflict");
            assert_eq!(hidden["text"], "");
            assert_eq!(hidden["chunks"], 0);
            session.handles.qualified.store(true, Ordering::Release);
            let recovered = session
                .harness
                .request(
                    "documents.document.recover",
                    json!({"documentId":document,"requestIds":[hex(32, 89)]}),
                )
                .await
                .unwrap();
            assert_eq!(recovered["requestStatuses"][0]["inclusion"], "unresolved");
            let rejected = submit(&session.handles, &document, &hex(32, 89), &(1, vec![1]))
                .await
                .unwrap_err();
            assert_eq!(rejected.code(), "documents.identity_conflict");
            let unrelated_update = Writer::new().write("live\n");
            let unrelated = DomainRecord::identified_document_edit(
                "bob",
                &other,
                &hex(32, 606),
                &[6; 32],
                unrelated_update,
            )
            .unwrap();
            assert!(stream.send(record_frame(6, "bob", &unrelated)).await);
            let store = session.handles.store.clone();
            wait_until(
                || store.cursor().unwrap().as_deref() == Some(hex(50, 6).as_str()),
                "unrelated edit",
            )
            .await;
            assert_eq!(store.edit_records(Some(&other)).unwrap().len(), 1);
            assert_eq!(
                session.handles.bodies.lock().unwrap()[&other].text(),
                "live\n"
            );
            assert!(!store.identity_conflicted(&other).unwrap());
            session.harness.shutdown().await.unwrap();
        }
    }

    #[tokio::test]
    async fn recover_lookup_interleaved_with_conflict_never_publishes_chunked_text() {
        let directory = tempfile::tempdir().unwrap();
        let session = start(directory.path(), scripted_log()).await;
        let document = hex(64, 2);
        let stream = session.scripted.stream();
        tokio::time::timeout(Duration::from_secs(2), stream.opened())
            .await
            .unwrap();
        for (seed, record) in [
            (1, DomainRecord::collection_create("bob", "Plan").unwrap()),
            (
                2,
                DomainRecord::document_create("bob", &hex(64, 1), "Notes", genesis()).unwrap(),
            ),
        ] {
            assert!(stream.send(record_frame(seed, "bob", &record)).await);
        }
        let store = session.handles.store.clone();
        wait_until(
            || store.cursor().unwrap().as_deref() == Some(hex(50, 2).as_str()),
            "metadata",
        )
        .await;
        let (events, _receiver) = broadcast::channel(64);
        let hub = test_hub(store.clone(), events);
        hub.qualified.store(true, Ordering::Release);
        hub.identity_index_complete.store(true, Ordering::Release);
        // An accepted local update, large enough that its text needs a chunked transfer.
        hub.with_body(&document, |body| {
            body.import(&genesis()).unwrap();
            let peer = body.allocate_peer();
            let update = body.revision_update(peer, &format!("{}\n", "A".repeat(150 * 1024)));
            body.accept_local_update(peer, hex(32, 54), &update)
                .unwrap();
        });
        let request_id = hex(32, 9);
        let request = RecoverRequest {
            document_id: document.clone(),
            request_ids: vec![request_id.clone()],
        };
        let (release, pending_lookup) = tokio::sync::oneshot::channel::<()>();
        let recovering = hub.clone();
        let publishing = tokio::spawn(async move {
            pending_lookup.await.unwrap();
            recovering
                .recovery_statuses(&request, ALICE, &Ok(HashMap::new()), &HashSet::new())
                .unwrap()
        });
        let update = Writer::new().write("signed\n");
        for seed in [3, 4] {
            let signed = DomainRecord::identified_document_edit(
                "bob",
                &document,
                &request_id,
                &[seed as u8; 32],
                update.clone(),
            )
            .unwrap();
            assert!(stream.send(record_frame(seed, "bob", &signed)).await);
            wait_until(
                || store.cursor().unwrap().as_deref() == Some(hex(50, seed).as_str()),
                "signed edit",
            )
            .await;
        }
        assert!(store.identity_conflicted(&document).unwrap());
        release.send(()).unwrap();
        let statuses = publishing.await.unwrap();
        assert_eq!(
            statuses,
            vec![
                json!({"requestId":request_id,"inclusion":"unresolved","durability":"unresolved"})
            ]
        );
        // The conflicted body opens with no snapshot at all.
        let (access, opening) = hub.session_opening(&document, 3);
        assert_eq!(access.reason, Some("identity_conflict"));
        assert_eq!(opening.len(), 2);
        assert_eq!(opening[0]["editable"], false);
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn sink_quarantines_evicted_remote_id_and_keeps_later_records_live() {
        let directory = tempfile::tempdir().unwrap();
        let session = start(directory.path(), scripted_log()).await;
        let stream = session.scripted.stream();
        tokio::time::timeout(Duration::from_secs(2), stream.opened())
            .await
            .unwrap();
        assert!(
            stream
                .send(record_frame(
                    1,
                    "bob",
                    &DomainRecord::collection_create("bob", "Plan").unwrap()
                ))
                .await
        );
        let document = hex(64, 2);
        assert!(
            stream
                .send(record_frame(
                    2,
                    "bob",
                    &DomainRecord::document_create("bob", &hex(64, 1), "Notes", genesis()).unwrap()
                ))
                .await
        );
        let mut source = Writer::new();
        let first_update = source.write("first\n");
        let first_id = hex(32, 10);
        let edit = |id: &str, update: Vec<u8>| {
            DomainRecord::identified_document_edit("bob", &document, id, &[7; 32], update).unwrap()
        };
        assert!(
            stream
                .send(record_frame(
                    3,
                    "bob",
                    &edit(&first_id, first_update.clone())
                ))
                .await
        );
        for offset in 0..=crate::body::MAX_RECENT_IDENTITIES {
            let seed = 100 + offset as u64;
            assert!(
                stream
                    .send(record_frame(
                        seed,
                        "bob",
                        &edit(&hex(32, seed), first_update.clone())
                    ))
                    .await
            );
        }
        let store = session.handles.store.clone();
        wait_until(
            || store.cursor().unwrap().as_deref() == Some(hex(50, 356).as_str()),
            "eviction replay",
        )
        .await;
        let bad = Writer::new().write("bad\n");
        assert!(
            stream
                .send(record_frame(500, "bob", &edit(&first_id, bad)))
                .await
        );
        wait_until(
            || store.cursor().unwrap().as_deref() == Some(hex(50, 500).as_str()),
            "conflict acknowledgement",
        )
        .await;
        assert!(store.has_projection_warnings().unwrap());
        assert!(store.identity_conflicted(&document).unwrap());
        let next = source.write("firstlater\n");
        assert!(
            stream
                .send(record_frame(501, "bob", &edit(&hex(32, 501), next)))
                .await
        );
        wait_until(
            || store.cursor().unwrap().as_deref() == Some(hex(50, 501).as_str()),
            "later edit",
        )
        .await;
        assert_eq!(
            session.handles.bodies.lock().unwrap()[&document].text(),
            "first\n",
            "later edit is sealed but not applied after conflict"
        );
        assert_eq!(
            store.edit_identity(&first_id).unwrap().unwrap().0,
            hex(64, 3)
        );
        assert!(
            session
                .handles
                .sync
                .lock()
                .unwrap()
                .warnings
                .contains(&"request_id_conflict".into())
        );
        session.harness.shutdown().await.unwrap();
    }

    /// Like [`scripted_log`], but `document.edit` appends fail, retryably, while `fail` is set.
    fn failing_edit_log(fail: Arc<AtomicBool>) -> ScriptedCapability {
        let next = Arc::new(std::sync::atomic::AtomicU64::new(100));
        ScriptedCapability::new().calls(move |method, payload| match method {
            "open" => Ok(json!({"results":payload["sealedRecords"].as_array().unwrap().iter()
                .map(|_| json!({"status":"waiting_for_history_access"})).collect::<Vec<_>>()})),
            "append" if payload["purpose"] == "document.edit" && fail.load(Ordering::SeqCst) => {
                Err(error("unavailable", true))
            }
            "append" => {
                let seed = next.fetch_add(1, Ordering::SeqCst);
                Ok(json!({"recordId":hex(64, seed),"orderKey":hex(82, seed),
                    "author":ALICE,"signature":"c2ln","sealedRecord":"c2VhbGVk","durability":"committed"}))
            }
            _ => Err(error("unavailable", false)),
        })
    }

    #[tokio::test]
    async fn invalid_remote_edit_while_a_local_update_is_staged_preserves_applied_baseline() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(
            directory.path(),
            failing_edit_log(Arc::new(AtomicBool::new(true))),
        )
        .await;
        qualify_empty_history(&session).await;
        let collection = session
            .harness
            .request("documents.collection.create", json!({"name":"Plan"}))
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let document = session
            .harness
            .request(
                "documents.document.create",
                json!({"collectionId":collection,"title":"Doc"}),
            )
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let valid = Writer::of(&session.handles, &document).write("good\n");
        let stream = session.scripted.stream();
        let remote = |id: u64, update: Vec<u8>| {
            DomainRecord::identified_document_edit("bob", &document, &hex(32, id), &[1; 32], update)
                .unwrap()
        };
        assert!(
            stream
                .send(record_frame(8, "bob", &remote(57, valid)))
                .await
        );
        wait_until(
            || {
                session
                    .handles
                    .bodies
                    .lock()
                    .unwrap()
                    .get(&document)
                    .is_some_and(|body| body.text() == "good\n")
            },
            "applied text",
        )
        .await;
        let mut text = UiSession::open(&mut session.harness, &document).await;
        let draft = session_update(&session.handles, &document, "good draft\n");
        assert!(
            submit(&session.handles, &document, &hex(32, 55), &draft)
                .await
                .is_err()
        );
        assert_eq!(
            session.handles.bodies.lock().unwrap()[&document].text(),
            "good draft\n",
            "the accepted update stays staged as a local preview"
        );
        assert!(session.handles.bodies.lock().unwrap()[&document].has_pending());
        assert!(
            stream
                .send(record_frame(9, "bob", &remote(56, b"not loro".to_vec())))
                .await
        );
        wait_until(
            || {
                session
                    .handles
                    .store
                    .import_failure(&document)
                    .unwrap()
                    .is_some()
            },
            "invalid disposition",
        )
        .await;
        let opened = body_view(&mut session.harness, &document).await;
        assert_eq!(
            opened["text"], "good\n",
            "an unappended preview is not authoritative last-good text"
        );
        assert_eq!(opened["reason"], "import_failure");
        let mut saved = false;
        while let Ok(Some(Ok(event))) =
            tokio::time::timeout(Duration::from_millis(20), text.events.next_event()).await
        {
            saved |= event["event"] == "status" && event["durable"] != json!([]);
        }
        assert!(
            !saved,
            "a delayed append cannot publish Saved after invalidation"
        );
        text.close().await;
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn signed_invalid_loro_edit_retains_last_good_text_without_certifying_inclusion() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        let stream = session.scripted.stream();
        tokio::time::timeout(Duration::from_secs(2), stream.opened())
            .await
            .unwrap();
        let document = hex(64, 2);
        for (seed, record) in [
            (1, DomainRecord::collection_create("bob", "Plan").unwrap()),
            (
                2,
                DomainRecord::document_create("bob", &hex(64, 1), "Notes", genesis()).unwrap(),
            ),
        ] {
            assert!(stream.send(record_frame(seed, "bob", &record)).await);
        }
        let store = session.handles.store.clone();
        wait_until(|| store.document_exists(&document).unwrap(), "document").await;
        let mut source = Writer::new();
        let good = source.write("good\n");
        let first =
            DomainRecord::identified_document_edit("bob", &document, &hex(32, 3), &[3; 32], good)
                .unwrap();
        assert!(stream.send(record_frame(3, "bob", &first)).await);
        wait_until(
            || {
                session
                    .handles
                    .bodies
                    .lock()
                    .unwrap()
                    .get(&document)
                    .is_some_and(|b| b.text() == "good\n")
            },
            "good body",
        )
        .await;
        let bad_id = hex(32, 77);
        let bad = DomainRecord::identified_document_edit(
            "bob",
            &document,
            &bad_id,
            &[7; 32],
            b"not loro".to_vec(),
        )
        .unwrap();
        assert!(stream.send(record_frame(4, "bob", &bad)).await);
        wait_until(
            || store.cursor().unwrap().as_deref() == Some(hex(50, 4).as_str()),
            "invalid edit cursor",
        )
        .await;
        let opened = body_view(&mut session.harness, &document).await;
        assert_eq!(opened["reason"], "import_failure");
        assert_eq!(opened["editable"], false);
        assert_eq!(opened["text"], "good\n");
        assert_eq!(opened["diagnosticRecordId"], hex(64, 4));
        assert_eq!(
            store.import_failure(&document).unwrap(),
            Some((hex(64, 4), "decode_or_checksum".into()))
        );
        let recovery = session
            .harness
            .request(
                "documents.document.recover",
                json!({ "documentId":document, "requestIds":[bad_id] }),
            )
            .await
            .unwrap();
        assert_eq!(recovery["requestStatuses"][0]["inclusion"], "unresolved");
        assert_eq!(store.edit_identity(&bad_id).unwrap().unwrap().0, hex(64, 4));
        let later = source.write("good later\n");
        assert!(
            stream
                .send(record_frame(
                    5,
                    "bob",
                    &DomainRecord::identified_document_edit(
                        "bob",
                        &document,
                        &hex(32, 5),
                        &[5; 32],
                        later
                    )
                    .unwrap()
                ))
                .await
        );
        let other = hex(64, 6);
        assert!(
            stream
                .send(record_frame(
                    6,
                    "bob",
                    &DomainRecord::document_create("bob", &hex(64, 1), "Other", genesis()).unwrap()
                ))
                .await
        );
        let other_update = Writer::new().write("live\n");
        assert!(
            stream
                .send(record_frame(
                    7,
                    "bob",
                    &DomainRecord::identified_document_edit(
                        "bob",
                        &other,
                        &hex(32, 7),
                        &[7; 32],
                        other_update
                    )
                    .unwrap()
                ))
                .await
        );
        wait_until(
            || store.cursor().unwrap().as_deref() == Some(hex(50, 7).as_str()),
            "unrelated edit cursor",
        )
        .await;
        assert_eq!(
            session.handles.bodies.lock().unwrap()[&document].text(),
            "good\n"
        );
        assert_eq!(
            session.handles.bodies.lock().unwrap()[&other].text(),
            "live\n"
        );
        assert_eq!(store.record_count().unwrap(), 7);
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn deferred_invalid_edit_is_marked_when_its_document_arrives() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        let stream = session.scripted.stream();
        tokio::time::timeout(Duration::from_secs(2), stream.opened())
            .await
            .unwrap();
        let document = hex(64, 4);
        assert!(
            stream
                .send(record_frame(
                    1,
                    "bob",
                    &DomainRecord::collection_create("bob", "Plan").unwrap()
                ))
                .await
        );
        let edit = DomainRecord::identified_document_edit(
            "bob",
            &document,
            &hex(32, 8),
            &[2; 32],
            b"not loro".to_vec(),
        )
        .unwrap();
        assert!(stream.send(record_frame(2, "bob", &edit)).await);
        let store = session.handles.store.clone();
        wait_until(
            || store.cursor().unwrap().as_deref() == Some(hex(50, 2).as_str()),
            "deferred edit cursor",
        )
        .await;
        assert!(store.import_failure(&document).unwrap().is_none());
        assert!(
            stream
                .send(record_frame(
                    4,
                    "bob",
                    &DomainRecord::document_create("bob", &hex(64, 1), "Doc", genesis()).unwrap()
                ))
                .await
        );
        wait_until(
            || store.import_failure(&document).unwrap().is_some(),
            "deferred import disposition",
        )
        .await;
        wait_until(
            || store.cursor().unwrap().as_deref() == Some(hex(50, 4).as_str()),
            "create cursor after deferred disposition",
        )
        .await;
        assert_eq!(
            store.import_failure(&document).unwrap().unwrap().0,
            hex(64, 2)
        );
        let opened = body_view(&mut session.harness, &document).await;
        assert_eq!(opened["reason"], "import_failure");
        assert_eq!(opened["editable"], false);
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn activation_migrates_old_invalid_edit_before_exposing_last_good_text() {
        for old_version in [4, 5] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("documents.sqlite3");
            let document = hex(64, 2);
            let other = hex(64, 3);
            let valid = Writer::new().write("last good\n");
            let records = [
                DomainRecord::collection_create("bob", "Plan").unwrap(),
                DomainRecord::document_create("bob", &hex(64, 1), "Doc", genesis()).unwrap(),
                DomainRecord::document_create("bob", &hex(64, 1), "Other", genesis()).unwrap(),
                DomainRecord::identified_document_edit(
                    "bob",
                    &document,
                    &hex(32, 10),
                    &[6; 32],
                    valid,
                )
                .unwrap(),
                DomainRecord::identified_document_edit(
                    "bob",
                    &document,
                    &hex(32, 11),
                    &[7; 32],
                    b"not loro".to_vec(),
                )
                .unwrap(),
            ];
            let store = DocumentStore::open(&path).unwrap();
            let encoded: Vec<(String, String)> = records
                .iter()
                .map(|record| {
                    (
                        record.kind().to_owned(),
                        URL_SAFE_NO_PAD.encode(record.encode()),
                    )
                })
                .collect();
            for (index, record) in records.iter().enumerate() {
                let seed = index as u64 + 1;
                store
                    .ingest(
                        &hex(64, seed),
                        &hex(82, seed),
                        "bob",
                        record.kind(),
                        "c2ln",
                        &record.encode(),
                        &[seed as u8],
                        None,
                    )
                    .unwrap();
            }
            drop(store);
            let connection = rusqlite::Connection::open(&path).unwrap();
            if old_version == 4 {
                connection.execute_batch("DROP TABLE edit_identities; DROP TABLE document_identity_conflicts; DROP TABLE document_import_failures; PRAGMA user_version=4;").unwrap();
            } else {
                connection
                    .execute_batch("DROP TABLE document_import_failures; PRAGMA user_version=5;")
                    .unwrap();
            }
            drop(connection);
            let scripted = ScriptedCapability::new().calls(move |method, payload| match method {
                "open" => Ok(json!({"results":payload["sealedRecords"].as_array().unwrap().iter().map(|sealed| {
                    let seed = URL_SAFE_NO_PAD.decode(sealed.as_str().unwrap()).unwrap()[0] as usize;
                    let (purpose, canonical) = &encoded[seed - 1];
                    json!({"status":"ready","recordId":hex(64,seed as u64),"orderKey":hex(82,seed as u64),
                        "author":"bob","purpose":purpose,"signature":"c2ln","canonical":canonical})
                }).collect::<Vec<_>>()})),
                _ => Err(error("unavailable", false)),
            });
            let mut session = start(directory.path(), scripted).await;
            let opened = body_view(&mut session.harness, &document).await;
            assert_eq!(opened["text"], "last good\n");
            assert_eq!(opened["reason"], "import_failure");
            assert_eq!(opened["editable"], false);
            assert_eq!(
                session
                    .handles
                    .store
                    .import_failure(&document)
                    .unwrap()
                    .unwrap()
                    .0,
                hex(64, 5)
            );
            let healthy = body_view(&mut session.harness, &other).await;
            assert_eq!(healthy["editable"], true);
            session.harness.shutdown().await.unwrap();
            let store = DocumentStore::open(&path).unwrap();
            assert_eq!(
                store.import_failure(&document).unwrap().unwrap().0,
                hex(64, 5)
            );
        }
    }

    #[tokio::test]
    async fn qualified_core_prefix_allows_local_edit_on_open_document_while_another_is_unopened() {
        let directory = tempfile::tempdir().unwrap();
        let collection = hex(64, 1);
        let unavailable = hex(64, 2);
        let healthy = hex(64, 3);
        let update = Writer::new().write("private\n");
        let records = [
            DomainRecord::collection_create("bob", "Plan").unwrap(),
            DomainRecord::document_create("bob", &collection, "Unavailable", genesis()).unwrap(),
            DomainRecord::document_create("bob", &collection, "Healthy", genesis()).unwrap(),
            DomainRecord::identified_document_edit(
                "bob",
                &unavailable,
                &hex(32, 8),
                &[3; 32],
                update,
            )
            .unwrap(),
        ];
        let store = DocumentStore::open(directory.path().join("documents.sqlite3")).unwrap();
        for (index, record) in records.iter().enumerate() {
            let seed = index as u64 + 1;
            store
                .ingest(
                    &hex(64, seed),
                    &hex(82, seed),
                    "bob",
                    record.kind(),
                    "c2ln",
                    &record.encode(),
                    &[seed as u8],
                    None,
                )
                .unwrap();
        }
        drop(store);
        let encoded: Vec<(String, String)> = records
            .iter()
            .map(|record| {
                (
                    record.kind().to_owned(),
                    URL_SAFE_NO_PAD.encode(record.encode()),
                )
            })
            .collect();
        let scripted = ScriptedCapability::new().calls(move |method, payload| match method {
            "open" => Ok(json!({"results": payload["sealedRecords"].as_array().unwrap().iter().map(|sealed| {
                let seed = URL_SAFE_NO_PAD.decode(sealed.as_str().unwrap()).unwrap()[0] as usize;
                if seed == 4 { json!({"status":"waiting_for_history_access"}) }
                else {
                    let (purpose, canonical) = &encoded[seed - 1];
                    json!({"status":"ready","recordId":hex(64, seed as u64),
                        "orderKey":hex(82, seed as u64),"author":"bob","purpose":purpose,
                        "signature":"c2ln","canonical":canonical})
                }
            }).collect::<Vec<_>>() })),
            "append" => Ok(json!({"recordId":hex(64, 100),"orderKey":hex(82, 100),
                "author":ALICE,"signature":"c2ln","sealedRecord":"c2VhbGVk","durability":"committed"})),
            _ => Err(error("unavailable", false)),
        });
        let mut session = start(directory.path(), scripted).await;
        assert_eq!(
            session.handles.bodies.lock().unwrap()[&unavailable].unopened(),
            1
        );
        let stream = session.scripted.stream();
        tokio::time::timeout(Duration::from_secs(2), stream.opened())
            .await
            .unwrap();
        for (index, record) in records.iter().enumerate() {
            assert!(
                stream
                    .send(record_frame(index as u64 + 1, "bob", record))
                    .await
            );
        }
        assert!(
            stream
                .send(json!({"kind":"replay_end", "cursor":hex(50, 4), "gapFree":true}))
                .await
        );
        let store = session.handles.store.clone();
        wait_until(
            || session.handles.qualified.load(Ordering::Acquire),
            "Core prefix",
        )
        .await;
        let local = session_update(&session.handles, &healthy, "healthy\n");
        let edited = submit(&session.handles, &healthy, &hex(32, 99), &local)
            .await
            .unwrap();
        assert_eq!(edited["revision"], "1");
        wait_until(
            || store.edit_identity(&hex(32, 99)).unwrap().is_some(),
            "healthy append",
        )
        .await;
        let unresolved = session
            .harness
            .request(
                "documents.document.recover",
                json!({"documentId":unavailable,"requestIds":[hex(32, 92)]}),
            )
            .await
            .unwrap();
        assert_eq!(unresolved["requestStatuses"][0]["inclusion"], "unresolved");
        let update = Writer::new().write("remote\n");
        let edit =
            DomainRecord::identified_document_edit("bob", &healthy, &hex(32, 98), &[6; 32], update)
                .unwrap();
        assert!(stream.send(record_frame(5, "bob", &edit)).await);
        wait_until(
            || store.cursor().unwrap().as_deref() == Some(hex(50, 5).as_str()),
            "healthy incoming cursor",
        )
        .await;
        assert!(
            session.handles.bodies.lock().unwrap()[&healthy]
                .text()
                .contains("remote")
        );
        assert_eq!(
            session.handles.bodies.lock().unwrap()[&unavailable].unopened(),
            1
        );
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn unopenable_deferred_identity_hides_applied_body_until_collision_is_known() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("documents.sqlite3");
        let document = hex(64, 2);
        let id = hex(32, 9);
        let update = Writer::new().write("private\n");
        let first =
            DomainRecord::identified_document_edit("bob", &document, &id, &[1; 32], update.clone())
                .unwrap();
        let second =
            DomainRecord::identified_document_edit("bob", &document, &id, &[2; 32], update)
                .unwrap();
        let mut stored = HashMap::new();
        {
            let store = DocumentStore::open(&path).unwrap();
            for (seed, record) in [
                (1, DomainRecord::collection_create("bob", "Plan").unwrap()),
                (
                    2,
                    DomainRecord::document_create("bob", &hex(64, 1), "Notes", genesis()).unwrap(),
                ),
                (3, first.clone()),
                (4, second.clone()),
            ] {
                store
                    .ingest(
                        &hex(64, seed),
                        &hex(82, seed),
                        "bob",
                        record.kind(),
                        "c2ln",
                        &record.encode(),
                        &[seed as u8],
                        None,
                    )
                    .unwrap();
                stored.insert(
                    seed,
                    (
                        record.kind().to_owned(),
                        URL_SAFE_NO_PAD.encode(record.encode()),
                    ),
                );
            }
            store.inner.lock().unwrap().conn.execute(
                "UPDATE records SET quarantine_code='waiting_for_target',target=?2 WHERE record_id=?1",
                rusqlite::params![hex(64, 4), document],
            ).unwrap();
        }
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch("DROP TABLE edit_identities; DROP TABLE document_identity_conflicts; DROP TABLE document_import_failures; PRAGMA user_version=4;").unwrap();
        drop(connection);
        let access = Arc::new(AtomicBool::new(false));
        let gained = access.clone();
        let scripted = ScriptedCapability::new().calls(move |method, payload| match method {
            "open" => Ok(
                json!({"results":payload["sealedRecords"].as_array().unwrap().iter().map(|sealed| {
                let seed = URL_SAFE_NO_PAD.decode(sealed.as_str().unwrap()).unwrap()[0] as u64;
                if seed == 4 && !gained.load(Ordering::SeqCst) {
                    json!({"status":"waiting_for_history_access"})
                } else {
                    let (purpose, canonical) = &stored[&seed];
                    json!({"status":"ready","recordId":hex(64,seed),"orderKey":hex(82,seed),
                        "author":"bob","purpose":purpose,"signature":"c2ln","canonical":canonical})
                }
            }).collect::<Vec<_>>()}),
            ),
            _ => Err(error("unavailable", false)),
        });
        let mut session = start(directory.path(), scripted).await;
        let pending = body_view(&mut session.harness, &document).await;
        assert_eq!(pending["editable"], false);
        assert_eq!(pending["text"], "");
        let stream = session.scripted.stream();
        tokio::time::timeout(Duration::from_secs(2), stream.opened())
            .await
            .unwrap();
        let other = hex(64, 5);
        assert!(
            stream
                .send(record_frame(
                    5,
                    "bob",
                    &DomainRecord::document_create("bob", &hex(64, 1), "Other", genesis()).unwrap()
                ))
                .await
        );
        let store = session.handles.store.clone();
        wait_until(
            || store.cursor().unwrap().as_deref() == Some(hex(50, 5).as_str()),
            "other Document create",
        )
        .await;
        let update = Writer::new().write("live\n");
        let identified =
            DomainRecord::identified_document_edit("bob", &other, &hex(32, 15), &[3; 32], update)
                .unwrap();
        assert!(stream.send(record_frame(6, "bob", &identified)).await);
        assert!(
            stream
                .send(record_frame(
                    7,
                    "bob",
                    &DomainRecord::collection_rename("bob", &hex(64, 1), "Later").unwrap()
                ))
                .await
        );
        wait_until(
            || store.cursor().unwrap().as_deref() == Some(hex(50, 7).as_str()),
            "cursor past unrelated identified edit",
        )
        .await;
        assert_eq!(
            session.handles.bodies.lock().unwrap()[&other].text(),
            "live\n"
        );
        let unresolved = session
            .harness
            .request(
                "documents.document.recover",
                json!({"documentId":document,"requestIds":[hex(32, 17)]}),
            )
            .await
            .unwrap();
        assert_eq!(unresolved["requestStatuses"][0]["inclusion"], "unresolved");
        access.store(true, Ordering::SeqCst);
        let stream = session.scripted.stream();
        tokio::time::timeout(Duration::from_secs(2), stream.opened())
            .await
            .unwrap();
        assert!(
            stream
                .send(json!({"kind":"state","availability":"current","pendingRecordCount":"0"}))
                .await
        );
        let store = session.handles.store.clone();
        wait_until(
            || store.identity_conflicted(&document).unwrap(),
            "opened deferred collision",
        )
        .await;
        let hidden = body_view(&mut session.harness, &document).await;
        assert_eq!(hidden["reason"], "identity_conflict");
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn migrated_deferred_identity_is_indexed_before_remote_collision() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("documents.sqlite3");
        let absent_document = hex(64, 9);
        let request_id = hex(32, 13);
        let update = Writer::new().write("safe\n");
        let original = DomainRecord::identified_document_edit(
            "bob",
            &absent_document,
            &request_id,
            &[1; 32],
            update.clone(),
        )
        .unwrap();
        let mut stored = HashMap::new();
        {
            let store = DocumentStore::open(&path).unwrap();
            let create = DomainRecord::collection_create("bob", "Plan").unwrap();
            let doc =
                DomainRecord::document_create("bob", &hex(64, 1), "Notes", genesis()).unwrap();
            for (seed, record) in [(1, create), (2, doc), (3, original.clone())] {
                let sealed = format!("row-{seed}");
                let result = store
                    .ingest(
                        &hex(64, seed),
                        &hex(82, seed),
                        "bob",
                        record.kind(),
                        "c2ln",
                        &record.encode(),
                        sealed.as_bytes(),
                        None,
                    )
                    .unwrap();
                assert!(matches!(
                    result,
                    IngestResult::Applied | IngestResult::Deferred(_)
                ));
                stored.insert(
                    URL_SAFE_NO_PAD.encode(sealed),
                    (
                        seed,
                        record.kind().to_owned(),
                        URL_SAFE_NO_PAD.encode(record.encode()),
                    ),
                );
            }
        }
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch("DROP TABLE edit_identities; DROP TABLE document_identity_conflicts; DROP TABLE document_import_failures; PRAGMA user_version=4;")
            .unwrap();
        drop(connection);
        let scripted = ScriptedCapability::new().calls(move |method, payload| match method {
            "open" => Ok(
                json!({"results":payload["sealedRecords"].as_array().unwrap().iter().map(|sealed| {
                let (seed,purpose,canonical) = &stored[sealed.as_str().unwrap()];
                json!({"status":"ready","recordId":hex(64,*seed),"orderKey":hex(82,*seed),
                    "author":"bob","purpose":purpose,"signature":"c2ln","canonical":canonical})
            }).collect::<Vec<_>>() }),
            ),
            _ => Err(error("unavailable", false)),
        });
        let session = start(directory.path(), scripted).await;
        let store = session.handles.store.clone();
        assert_eq!(
            store.edit_identity(&request_id).unwrap().unwrap().0,
            hex(64, 3)
        );
        let stream = session.scripted.stream();
        tokio::time::timeout(Duration::from_secs(2), stream.opened())
            .await
            .unwrap();
        let conflicting = DomainRecord::identified_document_edit(
            "bob",
            &absent_document,
            &request_id,
            &[2; 32],
            update.clone(),
        )
        .unwrap();
        assert!(stream.send(record_frame(500, "bob", &conflicting)).await);
        wait_until(
            || store.cursor().unwrap().as_deref() == Some(hex(50, 500).as_str()),
            "deferred ID collision",
        )
        .await;
        let valid = DomainRecord::identified_document_edit(
            "bob",
            &hex(64, 2),
            &hex(32, 501),
            &[3; 32],
            update,
        )
        .unwrap();
        assert!(stream.send(record_frame(501, "bob", &valid)).await);
        wait_until(
            || store.cursor().unwrap().as_deref() == Some(hex(50, 501).as_str()),
            "later valid edit",
        )
        .await;
        assert_eq!(
            session.handles.bodies.lock().unwrap()[&hex(64, 2)].text(),
            "safe\n"
        );
        assert!(store.has_projection_warnings().unwrap());
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn core_committed_unprojected_edit_is_indexed_before_replay_absence_proof() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        let stream = session.scripted.stream();
        tokio::time::timeout(Duration::from_secs(2), stream.opened())
            .await
            .unwrap();
        let document = hex(64, 2);
        let id = hex(32, 88);
        assert!(session.handles.store.edit_identity(&id).unwrap().is_none());
        assert!(
            stream
                .send(record_frame(
                    1,
                    "bob",
                    &DomainRecord::collection_create("bob", "Plan").unwrap()
                ))
                .await
        );
        assert!(
            stream
                .send(record_frame(
                    2,
                    "bob",
                    &DomainRecord::document_create("bob", &hex(64, 1), "Notes", genesis()).unwrap()
                ))
                .await
        );
        let store = session.handles.store.clone();
        wait_until(
            || store.document_exists(&document).unwrap(),
            "document arrives",
        )
        .await;
        let unresolved = session
            .harness
            .request(
                "documents.document.recover",
                json!({"documentId":document,"requestIds":[id]}),
            )
            .await
            .unwrap();
        assert_eq!(unresolved["requestStatuses"][0]["inclusion"], "unresolved");
        let update = Writer::new().write("replayed\n");
        let edit = DomainRecord::identified_document_edit(ALICE, &document, &id, &[7; 32], update)
            .unwrap();
        assert!(stream.send(record_frame(3, ALICE, &edit)).await);
        assert!(
            stream
                .send(json!({"kind":"replay_end","cursor":hex(50, 3),"gapFree":true}))
                .await
        );
        let store = session.handles.store.clone();
        wait_until(
            || {
                store.edit_identity(&id).unwrap().is_some()
                    && session.handles.qualified.load(Ordering::Acquire)
            },
            "Core replay and index",
        )
        .await;
        let received = session
            .harness
            .request(
                "documents.document.recover",
                json!({"documentId":document,"requestIds":[id,hex(32,89)]}),
            )
            .await
            .unwrap();
        assert_eq!(received["requestStatuses"][0]["inclusion"], "included");
        assert_eq!(
            received["requestStatuses"][1]["inclusion"], "unresolved",
            "an unopenable indexed row cannot certify other IDs absent"
        );
        assert_eq!(store.edit_identity(&id).unwrap().unwrap().0, hex(64, 3));
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: AC1, AC7 — delivered records are projected and the cursor is committed.
    #[tokio::test]
    async fn delivered_records_are_projected_and_cursor_committed() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        let stream = session.scripted.stream();
        tokio::time::timeout(Duration::from_secs(2), stream.opened())
            .await
            .unwrap();
        let create = DomainRecord::collection_create("bob", "Shared").unwrap();
        assert!(stream.send(record_frame(1, "bob", &create)).await);
        let title = DomainRecord::document_create("bob", &hex(64, 1), "Notes", genesis()).unwrap();
        assert!(stream.send(record_frame(2, "bob", &title)).await);
        let store = session.handles.store.clone();
        wait_until(
            || store.documents().map(|d| d.len() == 1).unwrap_or(false),
            "projection",
        )
        .await;
        wait_until(
            || store.cursor().ok().flatten().as_deref() == Some(hex(50, 2).as_str()),
            "cursor commit",
        )
        .await;
        assert!(
            stream
                .send(
                    json!({ "kind": "state", "availability": "current", "pendingRecordCount": "0" })
                )
                .await
        );
        let handles = session.handles.clone();
        wait_until(
            || handles.sync.lock().unwrap().availability == Availability::Current,
            "availability",
        )
        .await;
        let snapshot = session
            .harness
            .request("documents.snapshot", json!({}))
            .await
            .unwrap();
        assert_eq!(snapshot["availability"], "current");
        assert_eq!(snapshot["collections"][0]["name"], "Shared");
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn waiting_metadata_and_deferred_records_recover_on_history_state_without_restart() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let directory = tempfile::tempdir().unwrap();
        let store = DocumentStore::open(directory.path().join("documents.sqlite3")).unwrap();
        let collection = DomainRecord::collection_create("bob", "Plan").unwrap();
        let document =
            DomainRecord::document_create("bob", &hex(64, 1), "Notes", genesis()).unwrap();
        let comment = DomainRecord::comment_add("bob", &hex(64, 2), None, None, "Hello").unwrap();
        let deferred =
            DomainRecord::document_create("bob", &hex(64, 99), "Later", genesis()).unwrap();
        let records = [collection, document, comment, deferred];
        let mut canonical = HashMap::new();
        for (index, record) in records.iter().enumerate() {
            let seed = index as u64 + 1;
            store
                .ingest(
                    &hex(64, seed),
                    &hex(82, seed),
                    "bob",
                    record.kind(),
                    "sig",
                    &record.encode(),
                    &vec![seed as u8; 32 * 1024],
                    None,
                )
                .unwrap();
            canonical.insert(
                seed as u8,
                (
                    record.kind().to_owned(),
                    URL_SAFE_NO_PAD.encode(record.encode()),
                ),
            );
        }
        drop(store);
        let access = Arc::new(AtomicBool::new(false));
        let gained = access.clone();
        let scripted = ScriptedCapability::new().calls(move |method, payload| match method {
            "open" => {
                let sealed = payload["sealedRecords"].as_array().unwrap();
                assert!(sealed.len() <= 32);
                assert!(sealed.iter().map(|value| URL_SAFE_NO_PAD.decode(value.as_str().unwrap()).unwrap().len()).sum::<usize>() <= 512 * 1024);
                Ok(json!({"results": sealed.iter().map(|value| {
                    let seed = URL_SAFE_NO_PAD.decode(value.as_str().unwrap()).unwrap()[0];
                    if gained.load(Ordering::SeqCst) {
                        let (purpose, canonical) = &canonical[&seed];
                        json!({"status":"ready", "recordId":hex(64, seed as u64), "orderKey":hex(82, seed as u64),
                            "author":"bob", "purpose":purpose, "signature":"c2ln", "canonical":canonical})
                    } else { json!({"status":"waiting_for_history_access"}) }
                }).collect::<Vec<_>>() }))
            }
            _ => Err(error("unavailable", false)),
        });
        let session = start(directory.path(), scripted.clone()).await;
        assert_eq!(session.handles.store.collections().unwrap()[0].name, None);
        assert_eq!(
            session.handles.store.comments(&hex(64, 2)).unwrap()[0].markdown,
            None
        );
        access.store(true, Ordering::SeqCst);
        let stream = scripted.stream();
        tokio::time::timeout(Duration::from_secs(2), stream.opened())
            .await
            .unwrap();
        assert!(
            stream
                .send(json!({"kind":"state", "availability":"current", "pendingRecordCount":"0"}))
                .await
        );
        let store = session.handles.store.clone();
        wait_until(
            || {
                store.collections().unwrap()[0].name.as_deref() == Some("Plan")
                    && store.comments(&hex(64, 2)).unwrap()[0].markdown.as_deref() == Some("Hello")
            },
            "waiting titles and comments",
        )
        .await;
        assert_eq!(
            store.documents().unwrap()[0].title.as_deref(),
            Some("Notes")
        );
        let waiting = session.handles.waiting_metadata.clone();
        wait_until(
            || waiting.lock().unwrap().is_empty(),
            "deferred metadata retry",
        )
        .await;
        store
            .ingest(
                &hex(64, 99),
                &hex(82, 99),
                "bob",
                "collection.create",
                "sig",
                &DomainRecord::collection_create("bob", "Later collection")
                    .unwrap()
                    .encode(),
                b"sealed",
                None,
            )
            .unwrap();
        assert_eq!(
            store.documents().unwrap().len(),
            2,
            "opened deferred record applies when parent arrives"
        );
        let count = scripted
            .recorded_calls()
            .iter()
            .filter(|call| call.method == "open")
            .count();
        assert!(count >= 2, "metadata was retried on state change");
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn seventeen_large_records_replay_in_all_paths_and_history_becomes_editable() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let directory = tempfile::tempdir().unwrap();
        let collection = hex(64, 1);
        let document = hex(64, 2);
        let update = Writer::new().write("recovered\n");
        let store = DocumentStore::open(directory.path().join("documents.sqlite3")).unwrap();
        let mut seed = 1;
        let mut canonicals = HashMap::new();
        let mut save = |record: DomainRecord, target: u8| {
            canonicals.insert(
                seed,
                (
                    record.kind().to_owned(),
                    URL_SAFE_NO_PAD.encode(record.encode()),
                ),
            );
            let mut sealed = vec![target; 32 * 1024];
            sealed[1] = seed as u8;
            store
                .ingest(
                    &hex(64, seed),
                    &hex(82, seed),
                    "bob",
                    record.kind(),
                    "sig",
                    &record.encode(),
                    &sealed,
                    None,
                )
                .unwrap();
            seed += 1;
        };
        save(DomainRecord::collection_create("bob", "Plan").unwrap(), 1);
        save(
            DomainRecord::document_create("bob", &collection, "Notes", genesis()).unwrap(),
            2,
        );
        for _ in 0..17 {
            save(
                DomainRecord::collection_rename("bob", &collection, "Plan").unwrap(),
                3,
            );
        }
        for _ in 0..17 {
            save(
                DomainRecord::collection_rename("bob", &hex(64, 99), "Waiting").unwrap(),
                4,
            );
        }
        for index in 0..17 {
            let edit = DomainRecord::identified_document_edit(
                "bob",
                &document,
                &hex(32, 700 + index),
                &[5; 32],
                update.clone(),
            )
            .unwrap();
            save(edit, 5);
        }
        let access = Arc::new(AtomicBool::new(false));
        let gained = access.clone();
        let scripted = ScriptedCapability::new().calls(move |method, payload| match method {
            "open" => {
                let sealed = payload["sealedRecords"].as_array().unwrap();
                let total: usize = sealed.iter().map(|value| URL_SAFE_NO_PAD.decode(value.as_str().unwrap()).unwrap().len()).sum();
                assert!(sealed.len() <= 32 && total <= 512 * 1024);
                Ok(json!({"results": sealed.iter().map(|value| {
                    let bytes = URL_SAFE_NO_PAD.decode(value.as_str().unwrap()).unwrap();
                    // One edit opens early; the rest, and the create holding the genesis, wait.
                    let ready = match bytes[0] {
                        2 => gained.load(Ordering::SeqCst),
                        5 => gained.load(Ordering::SeqCst) || bytes[1] == 37,
                        _ => false,
                    };
                    if ready {
                        let (purpose, canonical) = &canonicals[&u64::from(bytes[1])];
                        json!({"status":"ready", "recordId":hex(64, u64::from(bytes[1])), "orderKey":hex(82, u64::from(bytes[1])),
                            "author":"bob", "purpose":purpose, "signature":"sig", "canonical":canonical})
                    } else { json!({"status":"waiting_for_history_access"}) }
                }).collect::<Vec<_>>() }))
            }
            _ => Err(error("unavailable", false)),
        });
        let mut session = start(directory.path(), scripted.clone()).await;
        assert_eq!(
            session.handles.bodies.lock().unwrap()[&document].unopened(),
            17,
            "sixteen edits and the create that carries the genesis"
        );
        let mut subscription = UiSession::open(&mut session.harness, &document).await;
        assert_eq!(subscription.begin["editable"], false);
        assert_eq!(subscription.begin["chunks"], 0);
        let pending = body_view(&mut session.harness, &document).await;
        assert_eq!(pending["editable"], false);
        assert_eq!(
            pending["text"], "",
            "one ready edit must not reveal partial body"
        );
        access.store(true, Ordering::SeqCst);
        let ready = body_view(&mut session.harness, &document).await;
        assert_eq!(ready["editable"], true);
        assert_eq!(ready["text"], "recovered\n");
        assert_eq!(
            session.handles.bodies.lock().unwrap()[&document].unopened(),
            0
        );
        let mut reloaded = false;
        for _ in 0..4 {
            if subscription.next().await["event"] == "resync" {
                reloaded = true;
                break;
            }
        }
        assert!(
            reloaded,
            "the existing session reloads once the body becomes editable"
        );
        let calls = scripted.recorded_calls();
        let activation_opens = calls.iter().filter(|call| call.method == "open").count();
        assert!(
            activation_opens >= 6,
            "control, deferred and edit paths each require two batches"
        );
        subscription.close().await;
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.DOCS-R05 — diagnostics exclude body bytes and payload digests.
    #[test]
    fn import_failure_diagnostic_has_only_bounded_provenance_fields() {
        let diagnostic = import_failure_diagnostic(
            &hex(64, 1),
            &hex(64, 2),
            &"a".repeat(200),
            Some(&hex(32, 3)),
            42,
            64,
            "decode_or_checksum",
            "replay",
        );
        let fields: Vec<_> = diagnostic
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            fields,
            [
                "author",
                "category",
                "documentId",
                "event",
                "phase",
                "recordId",
                "requestId",
                "sealedLength",
                "updateLength"
            ]
        );
        assert_eq!(diagnostic["author"].as_str().unwrap().len(), 128);
        assert!(diagnostic.get("payloadDigest").is_none());
        assert!(diagnostic.get("body").is_none());
        assert!(diagnostic.get("canonical").is_none());
        assert!(diagnostic.get("sealedRecord").is_none());
        assert_eq!(diagnostic["updateLength"], 42);
        assert_eq!(diagnostic["sealedLength"], 64);
    }

    // Verifies: TEAM.SYS.DOCS-R05 — Core durability does not prove local body inclusion.
    #[tokio::test]
    async fn post_append_projection_failure_remains_unresolved_until_replay() {
        let appended = Arc::new(Mutex::new(Vec::<(u64, String, String)>::new()));
        let saved = appended.clone();
        let next = Arc::new(std::sync::atomic::AtomicU64::new(100));
        let scripted = ScriptedCapability::new().calls(move |method, payload| match method {
            "append" => {
                let seed = next.fetch_add(1, Ordering::SeqCst);
                saved.lock().unwrap().push((seed, payload["purpose"].as_str().unwrap().to_owned(),
                    payload["canonical"].as_str().unwrap().to_owned()));
                Ok(json!({"recordId":hex(64,seed),"orderKey":hex(82,seed),
                    "author":ALICE,"signature":"c2ln","sealedRecord":"c2VhbGVk","durability":"committed"}))
            }
            "open" => Ok(json!({"results":payload["sealedRecords"].as_array().unwrap().iter()
                .map(|_| json!({"status":"waiting_for_history_access"})).collect::<Vec<_>>()})),
            _ => Err(error("unavailable", false)),
        });
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted).await;
        qualify_empty_history(&session).await;
        let collection = session
            .harness
            .request("documents.collection.create", json!({"name":"Plan"}))
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let document = session
            .harness
            .request(
                "documents.document.create",
                json!({"collectionId":collection,"title":"Draft"}),
            )
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let text = UiSession::open(&mut session.harness, &document).await;
        let store = session.handles.store.clone();
        store.inner.lock().unwrap().conn.execute_batch(
            "CREATE TRIGGER fail_edit_projection BEFORE INSERT ON records WHEN NEW.purpose='document.edit' BEGIN SELECT RAISE(FAIL, 'injected'); END;"
        ).unwrap();
        let id = hex(32, 83);
        let draft = session_update(&session.handles, &document, "draft\n");
        assert_eq!(
            submit(&session.handles, &document, &id, &draft)
                .await
                .unwrap_err()
                .code(),
            "documents.storage_failed"
        );
        assert_eq!(
            appended.lock().unwrap().len(),
            3,
            "Core accepted exactly one edit"
        );
        assert!(store.edit_identity(&id).unwrap().is_none());
        let recover = |_seed: u64| json!({"documentId":document,"requestIds":[id]});
        let before = session
            .harness
            .request("documents.document.recover", recover(84))
            .await
            .unwrap();
        assert_eq!(before["requestStatuses"][0]["durability"], "durable");
        assert_eq!(before["requestStatuses"][0]["inclusion"], "unresolved");
        assert_eq!(
            submit(&session.handles, &document, &id, &draft)
                .await
                .unwrap_err()
                .code(),
            "documents.document_pending"
        );
        assert_eq!(appended.lock().unwrap().len(), 3, "retry must not reappend");
        store
            .inner
            .lock()
            .unwrap()
            .conn
            .execute_batch("DROP TRIGGER fail_edit_projection")
            .unwrap();
        let (_, purpose, canonical) = appended.lock().unwrap()[2].clone();
        let record = DomainRecord::decode(&URL_SAFE_NO_PAD.decode(canonical).unwrap()).unwrap();
        assert_eq!(record.kind(), purpose);
        assert!(
            session
                .scripted
                .stream()
                .send(record_frame(102, ALICE, &record))
                .await
        );
        wait_until(
            || store.edit_identity(&id).unwrap().is_some(),
            "replayed edit projection",
        )
        .await;
        let after = session
            .harness
            .request("documents.document.recover", recover(85))
            .await
            .unwrap();
        assert_eq!(after["requestStatuses"][0]["inclusion"], "included");
        assert_eq!(after["requestStatuses"][0]["durability"], "durable");
        assert_eq!(appended.lock().unwrap().len(), 3);
        text.close().await;
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.DOCS-R05 — an indexed foreign ID overrides a pending cached retry.
    #[tokio::test]
    async fn cached_pending_retry_checks_indexed_foreign_document_after_eviction() {
        let directory = tempfile::tempdir().unwrap();
        let indexed = Arc::new(Mutex::new(None::<(String, String)>));
        let opener = indexed.clone();
        let openable = Arc::new(AtomicBool::new(false));
        let allow_open = openable.clone();
        let appends = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let appended = appends.clone();
        let scripted = ScriptedCapability::new().calls(move |method, payload| match method {
            "append" => {
                let seed = 100 + appended.fetch_add(1, Ordering::SeqCst);
                Ok(json!({"recordId":hex(64,seed),"orderKey":hex(82,seed),
                    "author":ALICE,"signature":"c2ln","sealedRecord":"c2VhbGVk","durability":"committed"}))
            }
            "open" => Ok(json!({"results":payload["sealedRecords"].as_array().unwrap().iter()
                .map(|_| match (&*opener.lock().unwrap(), allow_open.load(Ordering::SeqCst)) {
                    (Some((purpose, canonical)), true) => json!({"status":"ready",
                        "recordId":hex(64,400),"orderKey":hex(82,400),"author":ALICE,
                        "signature":"c2ln","purpose":purpose,"canonical":canonical}),
                    _ => json!({"status":"waiting_for_history_access"}),
                }).collect::<Vec<_>>()})),
            _ => Err(error("unavailable", false)),
        });
        let mut session = start(directory.path(), scripted).await;
        qualify_empty_history(&session).await;
        let collection = session
            .harness
            .request("documents.collection.create", json!({"name":"Plan"}))
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let a = session
            .harness
            .request(
                "documents.document.create",
                json!({"collectionId":collection,"title":"A"}),
            )
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let b = session
            .harness
            .request(
                "documents.document.create",
                json!({"collectionId":collection,"title":"B"}),
            )
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        session
            .harness
            .request("documents.document.open", json!({"documentId":a}))
            .await
            .unwrap();
        session
            .harness
            .request("documents.document.open", json!({"documentId":b}))
            .await
            .unwrap();
        let id = hex(32, 91);
        let draft = session_update(&session.handles, &b, "draft\n");
        let digest: [u8; 32] = Sha256::digest(&draft.1).into();
        session
            .handles
            .bodies
            .lock()
            .unwrap()
            .get_mut(&b)
            .unwrap()
            .remember_identity(id.clone(), ALICE.into(), digest, false)
            .unwrap();
        let update = Writer::of(&session.handles, &a).write("on A\n");
        let record =
            DomainRecord::identified_document_edit(ALICE, &a, &id, &[5; 32], update).unwrap();
        *indexed.lock().unwrap() = Some((
            record.kind().to_owned(),
            URL_SAFE_NO_PAD.encode(record.encode()),
        ));
        let store = session.handles.store.clone();
        store
            .ingest(
                &hex(64, 400),
                &hex(82, 400),
                ALICE,
                record.kind(),
                "c2ln",
                &record.encode(),
                b"sealed",
                None,
            )
            .unwrap();
        session
            .handles
            .bodies
            .lock()
            .unwrap()
            .get_mut(&a)
            .unwrap()
            .remember_identity(id.clone(), ALICE.into(), [5; 32], true)
            .unwrap();
        for seed in 0..=crate::body::MAX_RECENT_IDENTITIES {
            session
                .handles
                .bodies
                .lock()
                .unwrap()
                .get_mut(&a)
                .unwrap()
                .remember_identity(format!("{:032x}", seed + 1000), ALICE.into(), [0; 32], true)
                .unwrap();
        }
        assert!(
            session.handles.bodies.lock().unwrap()[&a]
                .identity(&id)
                .is_none()
        );
        assert!(
            session.handles.bodies.lock().unwrap()[&b]
                .identity(&id)
                .is_some()
        );
        // The scripted opener cannot authenticate this row: fail closed instead of accepting B's cache.
        let result = submit(&session.handles, &b, &id, &draft).await.unwrap_err();
        assert_eq!(result.code(), "documents.document_pending");
        assert_eq!(
            appends.load(Ordering::SeqCst),
            3,
            "unopenable row cannot cause an append"
        );
        openable.store(true, Ordering::SeqCst);
        let rejected = submit(&session.handles, &b, &id, &draft).await.unwrap_err();
        assert_eq!(rejected.code(), "documents.invalid_request");
        assert_eq!(
            appends.load(Ordering::SeqCst),
            3,
            "known foreign ID must not append"
        );
        assert!(!store.identity_conflicted(&a).unwrap());
        assert!(!store.identity_conflicted(&b).unwrap());
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn recover_never_certifies_pending_identity_before_or_after_failed_append() {
        let fail = Arc::new(AtomicBool::new(true));
        let failing = fail.clone();
        let next = Arc::new(std::sync::atomic::AtomicU64::new(100));
        let scripted = ScriptedCapability::new().calls(move |method, payload| match method {
            "open" => Ok(json!({"results":payload["sealedRecords"].as_array().unwrap().iter()
                .map(|_| json!({"status":"waiting_for_history_access"})).collect::<Vec<_>>()})),
            "append" if payload["purpose"] == "document.edit" && failing.load(Ordering::SeqCst) => Err(error("unavailable", true)),
            "append" => {
                let seed = next.fetch_add(1, Ordering::SeqCst);
                Ok(json!({"recordId":hex(64, seed),"orderKey":hex(82, seed),
                    "author":ALICE,"signature":"c2ln","sealedRecord":"c2VhbGVk","durability":"committed"}))
            },
            _ => Err(error("unavailable", false)),
        });
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted).await;
        qualify_empty_history(&session).await;
        let collection = session
            .harness
            .request("documents.collection.create", json!({"name":"Plans"}))
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let document = session
            .harness
            .request(
                "documents.document.create",
                json!({"collectionId":collection,"title":"Draft"}),
            )
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let text = UiSession::open(&mut session.harness, &document).await;
        let id = hex(32, 31);
        let draft = session_update(&session.handles, &document, "draft\n");
        let editing = {
            let (handles, document, id, draft) = (
                session.handles.clone(),
                document.clone(),
                id.clone(),
                draft.clone(),
            );
            tokio::spawn(async move { submit(&handles, &document, &id, &draft).await })
        };
        wait_until(
            || {
                session
                    .handles
                    .bodies
                    .lock()
                    .unwrap()
                    .get(&document)
                    .and_then(|body| body.identity(&id))
                    .is_some()
            },
            "pending identity",
        )
        .await;
        let recover = |_transfer: u64| json!({"documentId":document,"requestIds":[id]});
        let before = session
            .harness
            .request("documents.document.recover", recover(40))
            .await
            .unwrap();
        assert_eq!(before["requestStatuses"][0]["inclusion"], "included");
        assert_eq!(before["requestStatuses"][0]["durability"], "pending");
        assert!(editing.await.unwrap().is_err());
        let failed = session
            .harness
            .request("documents.document.recover", recover(41))
            .await
            .unwrap();
        assert_eq!(failed["requestStatuses"][0]["durability"], "pending");
        fail.store(false, Ordering::SeqCst);
        let premature = submit(&session.handles, &document, &id, &draft)
            .await
            .unwrap_err();
        assert_eq!(premature.code(), "documents.document_pending");
        let stream = session.scripted.stream();
        assert!(
            stream
                .send(json!({"kind":"replay_end","cursor":hex(50, 50),"gapFree":true}))
                .await
        );
        wait_until(
            || session.handles.qualified.load(Ordering::Acquire),
            "new Core prefix",
        )
        .await;
        submit(&session.handles, &document, &id, &draft)
            .await
            .unwrap();
        let saved = session
            .harness
            .request("documents.document.recover", recover(42))
            .await
            .unwrap();
        assert_eq!(saved["requestStatuses"][0]["durability"], "durable");
        text.close().await;
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn edit_failure_is_retryable_and_retries_without_duplicate_operations() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let fail = Arc::new(AtomicBool::new(true));
        let failure = fail.clone();
        let appended = Arc::new(std::sync::atomic::AtomicU64::new(100));
        let accepted = Arc::new(Mutex::new(None::<(u64, String)>));
        let saved = accepted.clone();
        let scripted = ScriptedCapability::new().calls(move |method, payload| {
            if method == "append" && payload["purpose"] == "document.edit"
                && failure.load(Ordering::SeqCst) {
                return Err(error("unavailable", true));
            }
            match method {
                "open" => Ok(json!({ "results": (0..payload["sealedRecords"].as_array().map_or(0, Vec::len))
                    .map(|_| match &*saved.lock().unwrap() {
                        Some((seed, canonical)) => json!({"status":"ready", "recordId":hex(64,*seed),
                            "orderKey":hex(82,*seed), "author":ALICE, "signature":"c2ln",
                            "purpose":"document.edit", "canonical":canonical}),
                        None => json!({"status":"waiting_for_history_access"}),
                    }).collect::<Vec<_>>() })),
                "append" => {
                    let seed = appended.fetch_add(1, Ordering::SeqCst);
                    if payload["purpose"] == "document.edit" {
                        *saved.lock().unwrap() = Some((seed, payload["canonical"].as_str().unwrap().to_owned()));
                    }
                    Ok(json!({ "recordId": hex(64, seed), "orderKey": hex(82, seed),
                        "author": ALICE, "signature": "c2ln", "sealedRecord": "c2VhbGVk",
                        "durability": "committed" }))
                }
                _ => Err(error("unavailable", false)),
            }
        });
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted).await;
        qualify_empty_history(&session).await;
        let collection = session
            .harness
            .request("documents.collection.create", json!({"name":"Plans"}))
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let document = session
            .harness
            .request(
                "documents.document.create",
                json!({"collectionId": collection, "title":"Q4"}),
            )
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut text = UiSession::open(&mut session.harness, &document).await;
        let id = "11111111111111111111111111111111";
        let draft = session_update(&session.handles, &document, "draft\n");
        let result = submit(&session.handles, &document, id, &draft).await;
        assert_eq!(
            result.unwrap_err().code(),
            "documents.capability_unavailable"
        );
        assert!(session.handles.bodies.lock().unwrap()[&document].has_pending());
        fail.store(false, Ordering::SeqCst);
        let stream = session.scripted.stream();
        assert!(
            stream
                .send(json!({"kind":"replay_end","cursor":hex(50, 60),"gapFree":true}))
                .await
        );
        wait_until(
            || session.handles.qualified.load(Ordering::Acquire),
            "retry prefix",
        )
        .await;
        let handles = session.handles.clone();
        let waiting_document = document.clone();
        wait_until(
            || !handles.bodies.lock().unwrap()[&waiting_document].has_pending(),
            "background append retry",
        )
        .await;
        let seen = text.follow_until(|markdown| markdown == "draft\n").await;
        assert!(
            seen.iter().all(|event| event["event"] == "update"),
            "the staged update reaches the session before it is durable"
        );
        let saved = text.next().await;
        assert_eq!(saved["event"], "status");
        assert_eq!(saved["durable"], json!([id]));
        assert_eq!(saved["editable"], true);
        let response = submit(&session.handles, &document, id, &draft)
            .await
            .unwrap();
        assert_eq!(response["revision"], "1");
        assert!(!session.handles.bodies.lock().unwrap()[&document].has_pending());
        assert_eq!(
            session.handles.bodies.lock().unwrap()[&document].text(),
            "draft\n"
        );
        fail.store(true, Ordering::SeqCst);
        let second = session_update(&session.handles, &document, "draft!\n");
        assert!(
            submit(
                &session.handles,
                &document,
                "22222222222222222222222222222222",
                &second
            )
            .await
            .is_err()
        );
        assert!(session.handles.bodies.lock().unwrap()[&document].has_pending());
        wait_until(
            || !session.handles.qualified.load(Ordering::Acquire),
            "uncertain append invalidates prefix",
        )
        .await;
        fail.store(false, Ordering::SeqCst);
        session.harness.dispose().await.unwrap();
        assert!(
            session.handles.bodies.lock().unwrap()[&document].has_pending(),
            "shutdown does not reappend an uncertain edit without a new Core prefix"
        );
        text.close().await;
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn acknowledged_edit_replays_from_core_records_after_restart() {
        let records = Arc::new(Mutex::new(HashMap::<String, (u64, String, String)>::new()));
        let next = Arc::new(std::sync::atomic::AtomicU64::new(100));
        let saved = records.clone();
        let scripted = ScriptedCapability::new().calls(move |method, payload| match method {
            "append" => {
                let seed = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let sealed = URL_SAFE_NO_PAD.encode(seed.to_string());
                saved.lock().unwrap().insert(
                    sealed.clone(),
                    (
                        seed,
                        payload["purpose"].as_str().unwrap().to_owned(),
                        payload["canonical"].as_str().unwrap().to_owned(),
                    ),
                );
                Ok(
                    json!({ "recordId": hex(64, seed), "orderKey": hex(82, seed),
                    "author": ALICE, "signature": "c2ln", "sealedRecord": sealed,
                    "durability": "committed" }),
                )
            }
            "open" => Ok(
                json!({ "results": payload["sealedRecords"].as_array().unwrap()
                .iter().map(|sealed| {
                    let (seed, purpose, canonical) = saved.lock().unwrap()
                        [sealed.as_str().unwrap()].clone();
                    json!({ "status": "ready", "recordId": hex(64, seed),
                        "orderKey": hex(82, seed), "author": ALICE, "signature": "c2ln",
                        "purpose": purpose, "canonical": canonical })
                }).collect::<Vec<_>>() }),
            ),
            _ => Err(error("unavailable", false)),
        });
        let directory = tempfile::tempdir().unwrap();
        let mut first = start(directory.path(), scripted.clone()).await;
        qualify_empty_history(&first).await;
        let collection = first
            .harness
            .request("documents.collection.create", json!({"name":"Plans"}))
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let document = first
            .harness
            .request(
                "documents.document.create",
                json!({"collectionId": collection, "title":"Q4"}),
            )
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let old_id = "33333333333333333333333333333333";
        let durable = session_update(&first.handles, &document, "durable\n");
        let response = submit(&first.handles, &document, old_id, &durable)
            .await
            .unwrap();
        assert_eq!(response["revision"], "1");
        assert_eq!(
            records.lock().unwrap().len(),
            3,
            "the edit was appended before ACK"
        );
        first.harness.shutdown().await.unwrap();
        // Populate more sealed edit IDs than the bounded recent-ID cache can retain.
        let original = records
            .lock()
            .unwrap()
            .values()
            .find(|(seed, _, _)| *seed == 102)
            .unwrap()
            .2
            .clone();
        let DomainRecord::IdentifiedDocumentEdit {
            payload_digest,
            update,
            ..
        } = DomainRecord::decode(&URL_SAFE_NO_PAD.decode(original).unwrap()).unwrap()
        else {
            panic!("identified edit");
        };
        let persisted = DocumentStore::open(directory.path().join("documents.sqlite3")).unwrap();
        for index in 0..=crate::body::MAX_RECENT_IDENTITIES {
            let seed = 1000 + index as u64;
            let id = format!("{seed:032x}");
            let record = DomainRecord::identified_document_edit(
                ALICE,
                &document,
                &id,
                &payload_digest,
                update.clone(),
            )
            .unwrap();
            let canonical = record.encode();
            let sealed = seed.to_string();
            assert_eq!(
                persisted
                    .ingest(
                        &hex(64, seed),
                        &hex(82, seed),
                        ALICE,
                        record.kind(),
                        "c2ln",
                        &canonical,
                        sealed.as_bytes(),
                        None
                    )
                    .unwrap(),
                IngestResult::Applied
            );
            records.lock().unwrap().insert(
                URL_SAFE_NO_PAD.encode(sealed),
                (
                    seed,
                    record.kind().to_owned(),
                    URL_SAFE_NO_PAD.encode(canonical),
                ),
            );
        }
        drop(persisted);
        let fail_lookup = Arc::new(AtomicBool::new(false));
        let lookup_failure = fail_lookup.clone();
        let saved = records.clone();
        let restart_log = ScriptedCapability::new().calls(move |method, payload| match method {
            "open" if lookup_failure.load(Ordering::SeqCst) => Err(error("unavailable", true)),
            "open" => Ok(json!({ "results": payload["sealedRecords"].as_array().unwrap().iter().map(|sealed| {
                let (seed, purpose, canonical) = saved.lock().unwrap()[sealed.as_str().unwrap()].clone();
                json!({ "status":"ready", "recordId":hex(64,seed), "orderKey":hex(82,seed),
                    "author":ALICE, "signature":"c2ln", "purpose":purpose, "canonical":canonical })
            }).collect::<Vec<_>>() })),
            _ => Err(error("unavailable", false)),
        });
        let mut restarted = start(directory.path(), restart_log).await;
        assert_eq!(
            restarted.handles.bodies.lock().unwrap()[&document].text(),
            "durable\n"
        );
        assert!(!restarted.handles.bodies.lock().unwrap()[&document].has_pending());
        assert!(
            restarted.handles.bodies.lock().unwrap()[&document]
                .identity("33333333333333333333333333333333")
                .is_none(),
            "the old ID was evicted from the bounded cache"
        );
        let old_count = records.lock().unwrap().len();
        fail_lookup.store(true, Ordering::SeqCst);
        assert_eq!(
            submit(&restarted.handles, &document, old_id, &durable)
                .await
                .unwrap_err()
                .code(),
            "documents.capability_unavailable"
        );
        let missing = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
        let query = json!({"documentId":document,"requestIds":["33333333333333333333333333333333",missing]});
        let first = restarted
            .harness
            .request("documents.document.recover", query.clone())
            .await
            .unwrap();
        assert_eq!(
            first["requestStatuses"][0]["inclusion"], "unresolved",
            "failed sealed lookup is not an absence proof"
        );
        assert_eq!(first["requestStatuses"][1]["inclusion"], "unresolved");
        fail_lookup.store(false, Ordering::SeqCst);
        let repeated = submit(&restarted.handles, &document, old_id, &durable)
            .await
            .unwrap();
        assert_eq!(repeated["revision"], "1");
        assert_eq!(
            records.lock().unwrap().len(),
            old_count,
            "evicted retry must not append again"
        );
        let conflict = session_update(&restarted.handles, &document, "other\n");
        assert_eq!(
            submit(&restarted.handles, &document, old_id, &conflict)
                .await
                .unwrap_err()
                .code(),
            "documents.invalid_request"
        );
        let found = restarted
            .harness
            .request("documents.document.recover", query.clone())
            .await
            .unwrap();
        assert_eq!(found["requestStatuses"][0]["inclusion"], "included");
        assert_eq!(found["requestStatuses"][0]["durability"], "durable");
        let stream = restarted.scripted.stream();
        tokio::time::timeout(Duration::from_secs(2), stream.opened())
            .await
            .unwrap();
        let mut existing = records
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        existing.sort_by_key(|(seed, _, _)| *seed);
        for (seed, purpose, canonical) in existing {
            assert!(stream.send(json!({
                "kind":"record","cursor":hex(50,seed),"recordId":hex(64,seed),"orderKey":hex(82,seed),
                "author":ALICE,"purpose":purpose,"signature":"c2ln","canonical":canonical,
                "sealedRecord":URL_SAFE_NO_PAD.encode(seed.to_string())
            })).await);
        }
        assert!(
            stream
                .send(json!({"kind":"replay_end","cursor":hex(50, 1000 + crate::body::MAX_RECENT_IDENTITIES as u64),"gapFree":true}))
                .await
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            while !restarted.handles.bodies.lock().unwrap()[&document].editable() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        // The SDK delivers the callback after the credited marker; wait for absence.
        let absent = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let event = restarted
                    .harness
                    .request("documents.document.recover", query.clone())
                    .await
                    .unwrap();
                if event["requestStatuses"][1]["inclusion"] == "absent" {
                    break event;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(absent["requestStatuses"][0]["durability"], "durable");
        restarted.harness.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn failed_append_keeps_the_staged_update_for_retry_and_shutdown_drain() {
        let directory = tempfile::tempdir().unwrap();
        let store = DocumentStore::open(directory.path().join("documents.sqlite3")).unwrap();
        let (events, _) = broadcast::channel(8);
        let hub = test_hub(store, events);
        hub.qualified.store(true, Ordering::Release);
        let staged = hub.with_body("doc", |body| {
            body.import(&genesis()).unwrap();
            let peer = body.allocate_peer();
            let update = body.revision_update(peer, "saved\n");
            body.accept_local_update(peer, hex(32, 1), &update).unwrap();
            update
        });
        let failure = drain_document(&hub, "doc", |_update| async {
            Err(error("documents.capability_unavailable", true))
        })
        .await;
        assert!(failure.is_err());
        assert!(hub.with_body("doc", |body| body.has_pending()));
        let mut persisted = Vec::new();
        drain_document(&hub, "doc", |update| {
            persisted.push(update);
            std::future::ready(Ok(()))
        })
        .await
        .unwrap();
        assert_eq!(
            persisted,
            vec![staged],
            "a retry appends exactly the same bytes"
        );
        assert!(!hub.with_body("doc", |body| body.has_pending()));
        let mut restarted = DocumentBody::new();
        restarted.import(&genesis()).unwrap();
        restarted.import(&persisted[0]).unwrap();
        assert_eq!(restarted.text(), "saved\n");
    }

    /// A tree update the shared schema does not allow, on top of [`genesis`].
    fn schema_invalid_update() -> Vec<u8> {
        let doc = loro::LoroDoc::new();
        doc.import(&TEST_GENESIS).unwrap();
        let before = doc.oplog_vv();
        let Some(loro::ValueOrContainer::Container(loro::Container::List(children))) =
            doc.get_map("doc").get("children")
        else {
            panic!("the genesis has a children list");
        };
        let node = children.insert_container(0, loro::LoroMap::new()).unwrap();
        node.insert("nodeName", "table").unwrap();
        doc.commit();
        doc.export(loro::ExportMode::updates(&before)).unwrap()
    }

    // Verifies: TEAM.SYS.DOCS-R05 — a remote record that leaves a schema-invalid tree is an import
    // failure of category `schema_violation`, and the last good body stays visible, read-only and
    // incomplete.
    #[tokio::test]
    async fn schema_invalid_remote_record_is_an_import_failure_that_keeps_the_last_good_body() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        let stream = session.scripted.stream();
        tokio::time::timeout(Duration::from_secs(2), stream.opened())
            .await
            .unwrap();
        let document = hex(64, 2);
        let good = Writer::new().write("good\n");
        let edit = |id: u64, update: Vec<u8>| {
            DomainRecord::identified_document_edit("bob", &document, &hex(32, id), &[1; 32], update)
                .unwrap()
        };
        for (seed, record) in [
            (1, DomainRecord::collection_create("bob", "Plan").unwrap()),
            (
                2,
                DomainRecord::document_create("bob", &hex(64, 1), "Notes", genesis()).unwrap(),
            ),
            (3, edit(3, good)),
            (4, edit(4, schema_invalid_update())),
        ] {
            assert!(stream.send(record_frame(seed, "bob", &record)).await);
        }
        let store = session.handles.store.clone();
        wait_until(
            || store.cursor().unwrap().as_deref() == Some(hex(50, 4).as_str()),
            "invalid record cursor",
        )
        .await;
        assert_eq!(
            store.import_failure(&document).unwrap(),
            Some((hex(64, 4), "schema_violation".into()))
        );
        let opened = body_view(&mut session.harness, &document).await;
        assert_eq!(opened["reason"], "import_failure");
        assert_eq!(opened["editable"], false);
        assert_eq!(opened["text"], "good\n");
        assert_eq!(opened["diagnosticRecordId"], hex(64, 4));
        let refused = submit(
            &session.handles,
            &document,
            &hex(32, 5),
            &session_update(&session.handles, &document, "good more\n"),
        )
        .await
        .unwrap_err();
        assert_eq!(refused.code(), "documents.import_failure");
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.DOCS-R05, TEAM.SYS.DOCS-R09 — each local update is appended once as its
    // own record, a refused update appends nothing, a remote record is relayed to the open editor
    // session, and the Package SQLite never holds the body's text.
    #[tokio::test]
    async fn local_updates_are_appended_once_and_remote_edits_pushed_without_plaintext() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        qualify_empty_history(&session).await;
        let collection = session
            .harness
            .request("documents.collection.create", json!({ "name": "Plans" }))
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let document = session
            .harness
            .request(
                "documents.document.create",
                json!({ "collectionId": collection, "title": "Q4" }),
            )
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let opened = body_view(&mut session.harness, &document).await;
        assert_eq!(opened["text"], "");
        assert_eq!(opened["version"], "0");
        assert_eq!(opened["editable"], true);
        let mut text = UiSession::open(&mut session.harness, &document).await;
        assert_eq!(text.markdown(), "");

        let first = session_update(&session.handles, &document, "# Zebra plan\n");
        let edited = submit(&session.handles, &document, &hex(32, 0x44), &first)
            .await
            .unwrap();
        assert_eq!(edited["revision"], "1");
        text.follow_until(|markdown| markdown == "# Zebra plan\n")
            .await;
        let (peer, update) = session_update(&session.handles, &document, "x\n");
        let foreign = submit(
            &session.handles,
            &document,
            &hex(32, 0x55),
            &(peer ^ 1, update),
        )
        .await
        .unwrap_err();
        assert_eq!(foreign.code(), "documents.foreign_peer");
        let second = session_update(&session.handles, &document, "# Zebra plan\n\nmore\n");
        submit(&session.handles, &document, &hex(32, 0x66), &second)
            .await
            .unwrap();

        // Each sequentially acknowledged request has its own durable append.
        let edit_appends = |scripted: &ScriptedCapability| {
            scripted
                .recorded_calls()
                .iter()
                .filter(|call| {
                    call.method == "append" && call.payload["purpose"] == "document.edit"
                })
                .count()
        };
        let scripted = session.scripted.clone();
        wait_until(|| edit_appends(&scripted) == 2, "two durable edit appends").await;
        tokio::time::sleep(FLUSH_RETRY * 3).await;
        assert_eq!(
            edit_appends(&session.scripted),
            2,
            "no repeated record for an appended update, and none for the refused one"
        );
        {
            let bodies = session.handles.bodies.lock().unwrap();
            assert_eq!(bodies[&document].text(), "# Zebra plan\n\nmore\n");
            assert!(!bodies[&document].has_pending());
        }

        // A remote peer's edit arrives as a record and is pushed to the subscription.
        let update = Writer::of(&session.handles, &document)
            .write("# Zebra plan\n\nmore\n\nbob: platypus\n");
        let remote = DomainRecord::identified_document_edit(
            "bob",
            &document,
            &hex(32, 0x77),
            &[7; 32],
            update,
        )
        .unwrap();
        let stream = session.scripted.stream();
        tokio::time::timeout(Duration::from_secs(2), stream.opened())
            .await
            .unwrap();
        assert!(stream.send(record_frame(9, "bob", &remote)).await);
        text.follow_until(|markdown| markdown.contains("platypus"))
            .await;
        let reopened = body_view(&mut session.harness, &document).await;
        assert_eq!(reopened["version"], "3");
        assert_eq!(reopened["text"], json!(text.markdown()));
        let merged = reopened["text"].as_str().unwrap();
        assert!(
            merged.contains("# Zebra plan\n") && merged.contains("bob: platypus"),
            "{merged:?}"
        );
        text.close().await;
        session.harness.shutdown().await.unwrap();
        for entry in std::fs::read_dir(directory.path()).unwrap() {
            let bytes = std::fs::read(entry.unwrap().path()).unwrap();
            for plaintext in [&b"Zebra"[..], b"platypus", b"more\n"] {
                assert!(
                    !bytes
                        .windows(plaintext.len())
                        .any(|window| window == plaintext),
                    "the Package data directory holds body text"
                );
            }
        }
    }

    /// A qualified activation holding one new, empty Document.
    async fn editable_document(directory: &std::path::Path) -> (Session, String) {
        let mut session = start(directory, scripted_log()).await;
        qualify_empty_history(&session).await;
        let collection = session
            .harness
            .request("documents.collection.create", json!({ "name": "Plans" }))
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let document = session
            .harness
            .request(
                "documents.document.create",
                json!({ "collectionId": collection, "title": "Q4" }),
            )
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        (session, document)
    }

    fn edit_appends(scripted: &ScriptedCapability) -> Vec<DomainRecord> {
        scripted
            .recorded_calls()
            .iter()
            .filter(|call| call.method == "append" && call.payload["purpose"] == "document.edit")
            .map(|call| {
                DomainRecord::decode(
                    &URL_SAFE_NO_PAD
                        .decode(call.payload["canonical"].as_str().unwrap())
                        .unwrap(),
                )
                .unwrap()
            })
            .collect()
    }

    // Verifies: TEAM.SYS.DOCS-R05 — two views on one Installation are two sessions with distinct
    // peers; both edit, each receives only the other's update, and both converge on an intact
    // history.
    #[tokio::test]
    async fn two_sessions_on_one_backend_relay_each_others_updates_and_converge() {
        let directory = tempfile::tempdir().unwrap();
        let (mut session, document) = editable_document(directory.path()).await;
        let mut left = UiSession::open(&mut session.harness, &document).await;
        let mut right = UiSession::open(&mut session.harness, &document).await;
        assert_ne!(left.peer, right.peer, "each session has its own peer");
        assert_eq!(left.markdown(), "");
        assert_eq!(left.begin["editable"], true);

        let from_left = type_into(&left.doc, 0, true, "left ");
        let answer = left
            .send(&mut session.harness, &document, &hex(32, 0xa1), &from_left)
            .await
            .unwrap();
        assert_eq!(answer, json!({ "requestId": hex(32, 0xa1) }));
        let from_right = type_into(&right.doc, 0, false, "right ");
        right
            .send(&mut session.harness, &document, &hex(32, 0xa2), &from_right)
            .await
            .unwrap();

        // Each side receives the other's update and never its own.
        let seen_by_right = right
            .follow_until(|markdown| markdown.contains("left"))
            .await;
        let seen_by_left = left
            .follow_until(|markdown| markdown.contains("right"))
            .await;
        for (seen, own) in [(&seen_by_left, &from_left), (&seen_by_right, &from_right)] {
            for event in seen.iter().filter(|event| event["event"] == "update") {
                assert_ne!(
                    STANDARD.decode(event["update"].as_str().unwrap()).unwrap(),
                    **own
                );
            }
        }
        assert_eq!(left.markdown(), right.markdown());
        let body = session.handles.bodies.lock().unwrap()[&document]
            .text()
            .to_owned();
        assert_eq!(left.markdown(), body);
        assert!(body.contains("left") && body.contains("right"), "{body:?}");

        // The record history is intact: one record per request, each holding only its
        // session's operations, and the records alone rebuild the same body.
        let records = edit_appends(&session.scripted);
        assert_eq!(records.len(), 2);
        let mut rebuilt = DocumentBody::new();
        rebuilt.import(&genesis_of(&session.scripted)).unwrap();
        for (record, peer) in records.iter().zip([left.peer, right.peer]) {
            let DomainRecord::IdentifiedDocumentEdit { update, .. } = record else {
                panic!("an identified edit");
            };
            let meta = loro::LoroDoc::decode_import_blob_meta(update, false).unwrap();
            assert_eq!(
                meta.partial_end_vv.keys().copied().collect::<Vec<_>>(),
                [peer]
            );
            rebuilt.import(update).unwrap();
        }
        assert_eq!(rebuilt.text(), body);
        left.close().await;
        right.close().await;
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.DOCS-R05 — a refused update ends its session with `resync` and appends
    // nothing; the same UI may resume as its own peer, never as an open session's or another
    // writer's.
    #[tokio::test]
    async fn a_refused_update_resyncs_its_session_and_only_its_own_peer_resumes() {
        let directory = tempfile::tempdir().unwrap();
        let (mut session, document) = editable_document(directory.path()).await;
        let mut observer = UiSession::open(&mut session.harness, &document).await;
        let mut refused = UiSession::open(&mut session.harness, &document).await;
        let impostor = loro::LoroDoc::new();
        impostor
            .import(&refused.doc.export(loro::ExportMode::Snapshot).unwrap())
            .unwrap();
        impostor.set_peer_id(refused.peer ^ 1).unwrap();
        let forged = type_into(&impostor, 0, true, "forged");
        let error = refused
            .send(&mut session.harness, &document, &hex(32, 0xb1), &forged)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "documents.foreign_peer");
        assert_eq!(refused.next().await["event"], "resync");
        assert!(
            refused.events.next_event().await.is_none(),
            "the refused session ends after resync"
        );
        let late = type_into(&refused.doc, 0, true, "late");
        assert_eq!(
            refused
                .send(&mut session.harness, &document, &hex(32, 0xb2), &late)
                .await
                .unwrap_err()
                .code(),
            "documents.unknown_session"
        );
        assert!(
            edit_appends(&session.scripted).is_empty(),
            "nothing was appended"
        );

        // The same UI resumes as its peer and its unsent bytes are accepted unchanged.
        let resume = |peer: u64| json!({ "documentId": document, "resumePeer": peer.to_string() });
        let resumed = UiSession::open_with(&mut session.harness, resume(refused.peer)).await;
        assert_eq!(resumed.peer, refused.peer);
        resumed
            .send(&mut session.harness, &document, &hex(32, 0xb3), &late)
            .await
            .unwrap();
        let seen = observer.follow_until(|markdown| markdown == "late\n").await;
        assert!(seen.iter().all(|event| event["event"] == "update"));
        let durable = observer.next().await;
        assert_eq!(durable["event"], "status");
        assert_eq!(durable["durable"], json!([hex(32, 0xb3)]));

        // An open session's peer is never handed to another session.
        let taken = UiSession::open_with(&mut session.harness, resume(observer.peer)).await;
        assert_ne!(taken.peer, observer.peer);
        // Nor is a peer another Installation writes with.
        let update = Writer::of(&session.handles, &document).write("late\n\nfrom bob\n");
        let foreign = *loro::LoroDoc::decode_import_blob_meta(&update, false)
            .unwrap()
            .partial_end_vv
            .keys()
            .next()
            .unwrap();
        let remote = DomainRecord::identified_document_edit(
            "bob",
            &document,
            &hex(32, 0xb4),
            &[4; 32],
            update,
        )
        .unwrap();
        assert!(
            session
                .scripted
                .stream()
                .send(record_frame(9, "bob", &remote))
                .await
        );
        observer
            .follow_until(|markdown| markdown.contains("from bob"))
            .await;
        let refused_foreign = UiSession::open_with(&mut session.harness, resume(foreign)).await;
        assert_ne!(refused_foreign.peer, foreign);
        for open in [observer, resumed, taken, refused_foreign] {
            open.close().await;
        }
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.DOCS-R05 — a session that holds the body keeps its unacknowledged
    // edits while the body waits for history delivered out of order (C before B): it gains
    // what the body adopts, is never resynced, and its retried update lands.
    #[tokio::test]
    async fn a_session_keeps_its_edits_while_history_arrives_out_of_order() {
        let directory = tempfile::tempdir().unwrap();
        let (mut session, document) = editable_document(directory.path()).await;
        let mut ui = UiSession::open(&mut session.harness, &document).await;
        assert_eq!(ui.begin["editable"], true);
        let mut writer = Writer::of(&session.handles, &document);
        let b = writer.write("from bob B\n");
        let c = writer.write("from bob B\n\nthen C\n");
        let remote = |seed: u64, update: Vec<u8>| {
            record_frame(
                seed,
                "bob",
                &DomainRecord::identified_document_edit(
                    "bob",
                    &document,
                    &hex(32, seed),
                    &[seed as u8; 32],
                    update,
                )
                .unwrap(),
            )
        };

        // C arrives first: the body waits for B and is not editable.
        assert!(session.scripted.stream().send(remote(31, c)).await);
        let pending = ui.next().await;
        assert_eq!(pending["event"], "status", "{pending}");
        assert_eq!(pending["editable"], false);
        let mine = type_into(&ui.doc, 0, false, "mine ");
        let refused = ui
            .send(&mut session.harness, &document, &hex(32, 0xc1), &mine)
            .await
            .unwrap_err();
        assert_eq!(refused.code(), "documents.document_pending");
        assert!(refused.retryable());

        // B arrives: the session gains B and C, then learns the body is editable, with no resync.
        assert!(session.scripted.stream().send(remote(32, b)).await);
        let seen = ui
            .follow_until(|markdown| markdown.contains("then C"))
            .await;
        assert!(
            seen.iter().all(|event| event["event"] != "resync"),
            "{seen:?}"
        );
        let mut editable = seen
            .iter()
            .any(|event| event["event"] == "status" && event["editable"] == true);
        while !editable {
            let event = ui.next().await;
            assert_ne!(event["event"], "resync", "{event}");
            editable = event["event"] == "status" && event["editable"] == true;
        }

        // The same request, retried with the same bytes, now lands.
        ui.send(&mut session.harness, &document, &hex(32, 0xc1), &mine)
            .await
            .unwrap();
        let body = session.handles.bodies.lock().unwrap()[&document]
            .text()
            .to_owned();
        assert!(body.contains("mine") && body.contains("then C"), "{body:?}");
        assert_eq!(ui.markdown(), body);
        assert_eq!(edit_appends(&session.scripted).len(), 1);
        ui.close().await;
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.DOCS-R05 — an update that races its session's end is refused as
    // `unknown_session` with nothing appended; the UI reconnects as the same peer, recovery
    // proves the request absent, and the same bytes then land.
    #[tokio::test]
    async fn an_update_racing_its_sessions_end_is_absent_and_lands_after_resume() {
        let directory = tempfile::tempdir().unwrap();
        let (mut session, document) = editable_document(directory.path()).await;
        let ui = UiSession::open(&mut session.harness, &document).await;
        let peer = ui.peer;
        let update = type_into(&ui.doc, 0, true, "raced");
        ui.close().await;
        let hub = session.handles.hub.clone();
        wait_until(|| hub.session(peer).is_none(), "the session ended").await;
        let request_id = hex(32, 0xd1);
        let request = json!({
            "documentId": document, "sessionPeer": peer.to_string(),
            "requestId": request_id, "update": STANDARD.encode(&update),
        });
        let error = session
            .harness
            .request("documents.document.update", request.clone())
            .await
            .unwrap_err();
        assert_eq!(error.code(), "documents.unknown_session");
        assert!(edit_appends(&session.scripted).is_empty());

        let resumed = UiSession::open_with(
            &mut session.harness,
            json!({ "documentId": document, "resumePeer": peer.to_string() }),
        )
        .await;
        assert_eq!(resumed.peer, peer);
        let recovered = session
            .harness
            .request(
                "documents.document.recover",
                json!({ "documentId": document, "requestIds": [request_id] }),
            )
            .await
            .unwrap();
        assert_eq!(
            recovered["requestStatuses"][0]["inclusion"], "absent",
            "{recovered}"
        );
        session
            .harness
            .request("documents.document.update", request)
            .await
            .unwrap();
        assert_eq!(edit_appends(&session.scripted).len(), 1);
        assert_eq!(
            session.handles.bodies.lock().unwrap()[&document].text(),
            "raced\n"
        );
        resumed.close().await;
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.DOCS-R05 — adopting an import keeps the bookkeeping that changed after
    // the import was classified: an append that succeeded is not staged again, and an identity
    // that became durable stays durable.
    #[test]
    fn adopting_an_import_keeps_bookkeeping_changed_since_it_was_classified() {
        let directory = tempfile::tempdir().unwrap();
        let store = DocumentStore::open(directory.path().join("documents.sqlite3")).unwrap();
        let (events, _receiver) = broadcast::channel(256);
        let hub = test_hub(store, events);
        hub.apply_edits(&[("doc".into(), TEST_GENESIS.clone())]);
        let id = hex(32, 0x41);
        hub.with_body("doc", |body| {
            let peer = body.allocate_peer();
            let update = body.revision_update(peer, "local\n");
            body.accept_local_update(peer, id.clone(), &update).unwrap();
            body.remember_identity(id.clone(), ALICE.into(), [4; 32], false)
                .unwrap();
        });
        let remote = Writer::new().write("remote\n");
        let (candidate, _) = hub
            .with_body("doc", |body| body.import_candidate(&remote))
            .unwrap();
        // Outside the gate, before the candidate is adopted: the staged update is appended.
        hub.with_body("doc", |body| {
            body.append_succeeded();
            body.remember_identity(id.clone(), ALICE.into(), [4; 32], true)
                .unwrap();
            body.set_projection_pending(&id, true);
        });
        hub.adopt("doc", candidate, None);
        hub.with_body("doc", |body| {
            assert!(
                !body.has_pending(),
                "the appended update is not staged again"
            );
            let known = body.identity(&id).unwrap();
            assert!(known.durable && known.projection_pending);
            assert!(body.text().contains("local") && body.text().contains("remote"));
        });
    }

    // Verifies: TEAM.SYS.DOCS-R05 — a panic while the publication gate was held does not stop
    // later local writes or remote imports.
    #[tokio::test]
    async fn a_poisoned_publication_gate_still_admits_writes_and_imports() {
        let directory = tempfile::tempdir().unwrap();
        let (session, document) = editable_document(directory.path()).await;
        let gate = session.handles.hub.publication_gate.clone();
        let _ = std::thread::spawn(move || {
            let _held = gate.lock().unwrap();
            panic!("a panic under the gate");
        })
        .join();
        assert!(session.handles.hub.publication_gate.is_poisoned());
        person_types(&session.handles, &document, 0, true, "still ").await;
        let update = Writer::of(&session.handles, &document).write("still\n\nand remote\n");
        let remote = DomainRecord::identified_document_edit(
            "bob",
            &document,
            &hex(32, 0x42),
            &[5; 32],
            update,
        )
        .unwrap();
        assert!(
            session
                .scripted
                .stream()
                .send(record_frame(42, "bob", &remote))
                .await
        );
        let bodies = session.handles.bodies.clone();
        let target = document.clone();
        wait_until(
            || {
                bodies.lock().unwrap()[&target]
                    .text()
                    .contains("and remote")
            },
            "the remote edit",
        )
        .await;
        session.harness.shutdown().await.unwrap();
    }

    /// The genesis the `document.create` record this activation appended carried.
    fn genesis_of(scripted: &ScriptedCapability) -> Vec<u8> {
        scripted
            .recorded_calls()
            .iter()
            .filter(|call| call.method == "append" && call.payload["purpose"] == "document.create")
            .find_map(|call| {
                match DomainRecord::decode(
                    &URL_SAFE_NO_PAD
                        .decode(call.payload["canonical"].as_str().unwrap())
                        .unwrap(),
                ) {
                    Ok(DomainRecord::DocumentCreate { genesis, .. }) => Some(genesis),
                    _ => None,
                }
            })
            .expect("the create record")
    }

    // Verifies: AC5 — a comment with a quote is appended, listed with its author, and resolved.
    #[tokio::test]
    async fn comments_are_added_listed_and_resolved() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        let collection = session
            .harness
            .request("documents.collection.create", json!({ "name": "Plans" }))
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let document = session
            .harness
            .request(
                "documents.document.create",
                json!({ "collectionId": collection, "title": "Q4" }),
            )
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let root = session
            .harness
            .request(
                "documents.comment.add",
                json!({ "documentId": document, "quote": "# Q4", "markdown": "Needs dates" }),
            )
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        session
            .harness
            .request(
                "documents.comment.add",
                json!({ "documentId": document, "parentId": root, "markdown": "Added" }),
            )
            .await
            .unwrap();
        let missing = session
            .harness
            .request(
                "documents.comment.add",
                json!({ "documentId": document, "parentId": hex(64, 5), "markdown": "x" }),
            )
            .await
            .unwrap_err();
        assert_eq!(missing.code(), "documents.unknown_comment");
        session
            .harness
            .request("documents.comment.resolve", json!({ "commentId": root }))
            .await
            .unwrap();
        let listed = session
            .harness
            .request("documents.comments", json!({ "documentId": document }))
            .await
            .unwrap();
        let comments = listed["comments"].as_array().unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[1]["quote"], "# Q4");
        assert_eq!(comments[1]["markdown"], "Needs dates");
        assert_eq!(comments[1]["resolved"], true);
        assert_eq!(comments[1]["author"], ALICE);
        assert_eq!(comments[1]["isLocal"], true);
        assert!(
            comments[1]["createdAt"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .is_ok()
        );
        assert_eq!(comments[0]["parentId"], root);
        assert_eq!(comments[0]["resolved"], false);
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: AC5/AC9 — worst-case JSON escaping, quote and Markdown fit; every older
    // OrderKey page is reachable without skipping a parent on another page.
    #[tokio::test]
    async fn comments_cursor_pages_bound_serialized_bytes_and_reach_all_threads() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        let collection = session
            .harness
            .request("documents.collection.create", json!({ "name": "C" }))
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let document = session
            .harness
            .request(
                "documents.document.create",
                json!({ "collectionId": collection, "title": "D" }),
            )
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let parent = hex(64, 1000);
        for seed in 1000..1018 {
            let record = DomainRecord::comment_add_at(
                ALICE,
                &document,
                (seed > 1000).then_some(parent.as_str()),
                Some(&"\u{0001}".repeat(crate::domain::MAX_QUOTE)),
                &"\u{0001}".repeat(crate::domain::MAX_MARKDOWN),
                Some("1730000000000"),
            )
            .unwrap();
            session
                .handles
                .store
                .ingest(
                    &hex(64, seed),
                    &hex(82, seed),
                    ALICE,
                    record.kind(),
                    "sig",
                    &record.encode(),
                    b"sealed",
                    None,
                )
                .unwrap();
        }
        let mut cursor = None;
        let mut ids = std::collections::HashSet::new();
        let mut pages = 0;
        loop {
            let mut request = json!({ "documentId": document });
            if let Some(cursor) = cursor {
                request["cursor"] = json!(cursor);
            }
            let page = session
                .harness
                .request("documents.comments", request)
                .await
                .unwrap();
            assert!(page.to_string().len() < 256 * 1024);
            let comments = page["comments"].as_array().unwrap();
            assert!(!comments.is_empty());
            for comment in comments {
                assert!(ids.insert(comment["id"].as_str().unwrap().to_owned()));
                assert_eq!(comment["createdAt"], "1730000000000");
            }
            pages += 1;
            if !page["hasMore"].as_bool().unwrap() {
                break;
            }
            cursor = Some(page["nextCursor"].as_str().unwrap().to_owned());
        }
        assert!(pages > 1);
        assert_eq!(ids.len(), 18);
        assert!(ids.contains(&parent));
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: AC6 — Person and Agent signing identities are projected from the current
    // Principal roster, and the stream closes with the subscription.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn names_are_forwarded_and_the_stream_closes_with_the_subscription() {
        let directory = tempfile::tempdir().unwrap();
        let names = ScriptedCapability::new().calls(|method, _| match method {
            "current" => Ok(
                json!({ "workspaceId": "a".repeat(32), "cursor": "0", "principals": [
                {"principalId":"b".repeat(64),"kind":"person","stewardPrincipalId":null,
                    "keys":["c".repeat(64)],"name":"Alice","fingerprint":"cccccccccccc"},
                {"principalId":"d".repeat(64),"kind":"agent","stewardPrincipalId":"b".repeat(64),
                    "keys":["e".repeat(64)],"name":"Helper","fingerprint":"eeeeeeeeeeee"}
            ] }),
            ),
            _ => Err(BackendError::new(
                "capability_unavailable",
                false,
                json!({}),
            )),
        });
        let observer: ActivationObserver = Arc::new(Mutex::new(None));
        let mut harness = HarnessBuilder::new(router_observing(Some(observer)).into_backend())
            .package_identity("dev.teamarness.documents")
            .installation(ALICE, "Alice")
            .data_directory(directory.path())
            .capability(RECORD_LOG, scripted_log())
            .capability("teamarness.principals", names.clone())
            .start()
            .await;
        harness.activate().await.unwrap();
        let mut subscription = harness
            .subscribe("documents.names", json!({}))
            .await
            .unwrap();
        let first = subscription.next_event().await.unwrap().unwrap();
        assert_eq!(first["cursor"], "0");
        assert_eq!(
            first["persons"],
            json!([
                {"identity":"c".repeat(64),"personId":"b".repeat(64),
                    "name":"Alice","fingerprint":"cccccccccccc"},
                {"identity":"e".repeat(64),"personId":"d".repeat(64),
                    "name":"Helper","fingerprint":"eeeeeeeeeeee"}
            ])
        );
        assert_eq!(first["parts"], 1);
        let stream = names.stream();
        tokio::time::timeout(Duration::from_secs(2), stream.opened())
            .await
            .unwrap();
        subscription.close().await;
        wait_until(|| !stream.is_open(), "idle principals stream closure").await;
        harness.shutdown().await.unwrap();
    }

    // ------------------------------------------------------------ Port Contract 1.0

    fn port_envelope(port: &str, actor: JsonValue, request: JsonValue) -> JsonValue {
        json!({
            "contract": ports::CONTRACT, "major": 1, "minor": 0,
            "port": port, "actor": actor, "request": request
        })
    }

    fn alice() -> JsonValue {
        json!({"kind":"person","identity":ALICE})
    }

    /// Calls one provided Port as `actor` and unwraps the Port value.
    async fn port_as(
        harness: &mut Harness,
        kind: &str,
        port: &str,
        actor: JsonValue,
        request: JsonValue,
    ) -> Result<JsonValue, BackendError> {
        harness
            .request(
                &format!("teamarness.port.{kind}.v1"),
                port_envelope(port, actor, request),
            )
            .await
            .map(|value| value["$teamarnessPortValue"].clone())
    }

    async fn query(
        harness: &mut Harness,
        port: &str,
        request: JsonValue,
    ) -> Result<JsonValue, BackendError> {
        port_as(harness, "query", port, alice(), request).await
    }

    async fn revise(
        harness: &mut Harness,
        document: &str,
        base: &JsonValue,
        markdown: &str,
    ) -> Result<JsonValue, BackendError> {
        port_as(
            harness,
            "command",
            "document.revise",
            alice(),
            json!({"documentId":document,"baseVersion":base,"markdown":markdown}),
        )
        .await
    }

    fn appends(session: &Session) -> usize {
        session
            .scripted
            .recorded_calls()
            .into_iter()
            .filter(|call| call.method == "append")
            .count()
    }

    /// A qualified activation holding one Collection and one Document; returns their IDs.
    async fn ported_document(session: &mut Session) -> (String, String) {
        qualify_empty_history(session).await;
        let collection = session
            .harness
            .request("documents.collection.create", json!({ "name": "Plans" }))
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let document = session
            .harness
            .request(
                "documents.document.create",
                json!({ "collectionId": collection, "title": "Q4" }),
            )
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        (collection, document)
    }

    /// A person typing into `document` through the one local write path.
    async fn person_types(
        handles: &ActivationHandles,
        document: &str,
        block: usize,
        at_end: bool,
        text: &str,
    ) {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let (peer, update) = {
            let bodies = handles.bodies.lock().unwrap();
            let body = &bodies[document];
            let peer = body.allocate_peer();
            let doc = loro::LoroDoc::new();
            doc.import(&body.snapshot()).unwrap();
            doc.set_peer_id(peer).unwrap();
            (peer, type_into(&doc, block, at_end, text))
        };
        let id = format!("7e{:030x}", NEXT.fetch_add(1, Ordering::Relaxed));
        submit(handles, document, &id, &(peer, update))
            .await
            .unwrap();
    }

    /// `length` lowercase letters from `seed`, unrelated to any other seed's: a worst case for a
    /// character diff.
    fn letters(seed: u64, length: usize) -> String {
        let mut state = seed | 1;
        (0..length)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                char::from(b'a' + (state % 26) as u8)
            })
            .collect()
    }

    /// How long one revise may take when its character diff exhausts the merge's diff budget
    /// (`markdown::DIFF_BUDGET`). Measured: about 0.9 s in release and 10 s in the debug build.
    /// An unbounded diff of the same texts took 146 s in release.
    const LARGE_REVISE_BOUND: Duration = if cfg!(debug_assertions) {
        Duration::from_secs(60)
    } else {
        Duration::from_secs(5)
    };

    // Verifies: TEAM.SYS.DOCS-R12 — a revise that replaces one 150 K-character paragraph with
    // unrelated text returns within a stated bound, as one record, instead of running an
    // unbounded character diff.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_revise_replacing_a_large_paragraph_returns_within_its_bound() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        let (_, document) = ported_document(&mut session).await;
        let read = query(
            &mut session.harness,
            "document.markdown",
            json!({"documentId":document}),
        )
        .await
        .unwrap();
        let first = letters(1, 150_000);
        let revised = revise(
            &mut session.harness,
            &document,
            &read["version"],
            &format!("{first}\n"),
        )
        .await
        .unwrap();
        let before = appends(&session);
        let second = letters(2, 150_000);
        let started = std::time::Instant::now();
        let replaced = revise(
            &mut session.harness,
            &document,
            &revised["version"],
            &format!("{second}\n"),
        )
        .await
        .unwrap();
        let elapsed = started.elapsed();
        assert!(elapsed < LARGE_REVISE_BOUND, "the revise took {elapsed:?}");
        assert_eq!(replaced["changed"], true);
        assert_eq!(appends(&session), before + 1, "one record");
        assert_eq!(
            session.handles.bodies.lock().unwrap()[&document].text(),
            format!("{second}\n")
        );
        session.harness.shutdown().await.unwrap();
    }

    type MergePause = (
        std::sync::mpsc::SyncSender<()>,
        std::sync::mpsc::Receiver<()>,
    );

    /// Merges of these Documents wait, once each, after reporting that they started.
    static MERGE_PAUSES: std::sync::LazyLock<Mutex<HashMap<String, MergePause>>> =
        std::sync::LazyLock::new(Mutex::default);

    /// The pause a test set for the next merge of `document`, if any.
    pub(super) fn merge_pause(document: &str) -> Option<MergePause> {
        recover(&MERGE_PAUSES).remove(document)
    }

    // Verifies: TEAM.SYS.DOCS-R12 — a revise merges without holding the publication gate, the
    // bodies lock or the identity lock, so a session update on another Document completes while
    // the merge is still running.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_running_merge_does_not_block_an_update_on_another_document() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        let (collection, merging) = ported_document(&mut session).await;
        let other = session
            .harness
            .request(
                "documents.document.create",
                json!({ "collectionId": collection, "title": "Other" }),
            )
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let (started, on_start) = std::sync::mpsc::sync_channel(1);
        let (release, on_release) = std::sync::mpsc::sync_channel(1);
        recover(&MERGE_PAUSES).insert(merging.clone(), (started, on_release));
        let handles = session.handles.clone();
        let target = merging.clone();
        let revising = tokio::spawn(async move {
            let base = handles.bodies.lock().unwrap()[&target].frontiers();
            let staged = stage_local_write(
                &handles.log,
                &handles.hub,
                ALICE,
                &target,
                &hex(32, 0xe1),
                LocalWrite::Revision {
                    base: &base,
                    markdown: "merged later\n",
                },
            )
            .await?;
            drop(staged);
            drain_local_writes(&handles.log, &handles.hub, &handles.changes, ALICE, &target).await
        });
        tokio::task::spawn_blocking(move || on_start.recv().unwrap())
            .await
            .unwrap();

        // The merge is running: an update on the other Document goes through now.
        tokio::time::timeout(
            Duration::from_secs(10),
            person_types(&session.handles, &other, 0, true, "typed"),
        )
        .await
        .expect("the update did not wait for the merge");
        assert_eq!(
            session.handles.bodies.lock().unwrap()[&other].text(),
            "typed\n"
        );
        assert!(!revising.is_finished());

        release.send(()).unwrap();
        revising.await.unwrap().unwrap();
        assert_eq!(
            session.handles.bodies.lock().unwrap()[&merging].text(),
            "merged later\n"
        );
        session.harness.shutdown().await.unwrap();
    }

    #[test]
    fn public_port_contract_has_exact_closed_bounded_shapes() {
        let contract = teamarness_package_contract::port_contract::compile_port_contracts(
            include_bytes!("../../ports.json"),
        )
        .unwrap();
        assert!(contract.warnings.is_empty(), "{:?}", contract.warnings);
        assert_eq!(contract.contracts.len(), 1);
        let provided = &contract.contracts[0];
        assert_eq!(provided.identity, ports::CONTRACT);
        assert_eq!((provided.major, provided.minor), (1, 0));
        let names: Vec<_> = provided
            .ports
            .iter()
            .map(|port| (port.name.as_str(), serde_json::to_value(port.kind).unwrap()))
            .collect();
        assert_eq!(
            names,
            [
                ("document.list", json!("query")),
                ("document.markdown", json!("query")),
                ("document.revise", json!("command")),
                ("documents.changed", json!("event")),
            ]
        );
        let revise = provided
            .ports
            .iter()
            .find(|port| port.name == "document.revise")
            .unwrap();
        assert!(
            revise
                .description
                .contains("must not be retried automatically")
        );
        // Every code the provider's revise returns is named for the Consumer.
        for code in [
            "documents.base_unavailable",
            "documents.revision_too_large",
            "documents.schema_violation",
            "documents.unknown_document",
            "documents.pending",
            "documents.unavailable",
            "documents.actor_unauthorized",
        ] {
            assert!(revise.description.contains(code), "{code}");
        }
        let markdown = provided
            .ports
            .iter()
            .find(|port| port.name == "document.markdown")
            .unwrap();
        assert!(markdown.description.contains("MAX_UI_PAYLOAD_BYTES"));
        let valid = |port: &str, direction: &str, payload: JsonValue| {
            contract
                .validate(ports::CONTRACT, port, direction, &payload)
                .is_ok()
        };
        let id = hex(64, 1);
        let version = ports::encode_version(&Frontiers::from_id(loro::ID::new(7, 0))).unwrap();
        assert!(valid("document.list", "request", json!({})));
        assert!(valid("document.list", "request", json!({"limit":64})));
        assert!(!valid("document.list", "request", json!({"limit":65})));
        assert!(!valid(
            "document.list",
            "request",
            json!({"actor":"forged"})
        ));
        assert!(valid(
            "document.revise",
            "request",
            json!({"documentId":id,"baseVersion":version,"markdown":""})
        ));
        for invalid in [
            json!({"documentId":id,"markdown":"x"}),
            json!({"documentId":id,"baseVersion":"short","markdown":"x"}),
            json!({"documentId":id,"baseVersion":version,"markdown":"x".repeat(262_145)}),
        ] {
            assert!(!valid("document.revise", "request", invalid));
        }
        assert!(valid("documents.changed", "event", json!({})));
        assert!(valid(
            "documents.changed",
            "event",
            json!({"documentIds":[id]})
        ));
        assert!(!valid(
            "documents.changed",
            "event",
            json!({"documentIds":vec![hex(64, 2); 65]})
        ));
        assert!(!valid(
            "documents.changed",
            "event",
            json!({"markdown":"secret"})
        ));
    }

    // Verifies: TEAM.SYS.DOCS-R12 — every shared merge scenario holds through the provider, with
    // each person's concurrent edits applied through the local write path before the revise
    // arrives.
    #[tokio::test]
    async fn every_shared_merge_scenario_holds_through_document_revise() {
        let table: JsonValue =
            serde_json::from_str(include_str!("../../fixtures/merge-scenarios.json")).unwrap();
        let scenarios = table["scenarios"].as_array().unwrap();
        assert!(!scenarios.is_empty());
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        let (collection, _) = ported_document(&mut session).await;
        for scenario in scenarios {
            let name = scenario["name"].as_str().unwrap();
            let document = session
                .harness
                .request(
                    "documents.document.create",
                    json!({ "collectionId": collection, "title": "Scenario" }),
                )
                .await
                .unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned();
            let genesis = query(
                &mut session.harness,
                "document.markdown",
                json!({"documentId":document}),
            )
            .await
            .unwrap();
            assert_eq!(genesis["markdown"], "", "{name}");
            let setup = revise(
                &mut session.harness,
                &document,
                &genesis["version"],
                scenario["initial"].as_str().unwrap(),
            )
            .await
            .unwrap();
            assert_eq!(setup["changed"], true, "{name}");
            assert_eq!(setup["normalised"], false, "{name}: `initial` is canonical");
            let initial = setup["version"].clone();
            for edit in scenario["concurrentHumanEdits"].as_array().unwrap() {
                person_types(
                    &session.handles,
                    &document,
                    usize::try_from(edit["block"].as_u64().unwrap()).unwrap(),
                    edit["at"] == "end",
                    edit["text"].as_str().unwrap(),
                )
                .await;
            }
            let base = match scenario["base"].as_str().unwrap() {
                "initial" => initial,
                "current" => query(
                    &mut session.harness,
                    "document.markdown",
                    json!({"documentId":document}),
                )
                .await
                .unwrap()["version"]
                    .clone(),
                other => panic!("{name}: unknown base {other}"),
            };
            let before = appends(&session);
            let revised = revise(
                &mut session.harness,
                &document,
                &base,
                scenario["revision"].as_str().unwrap(),
            )
            .await
            .unwrap();
            let unchanged = scenario["expectedOps"] == 0;
            assert_eq!(revised["changed"], !unchanged, "{name}");
            assert_eq!(
                appends(&session) - before,
                usize::from(!unchanged),
                "{name}: one revise is one record, and a no-op is none"
            );
            let read = query(
                &mut session.harness,
                "document.markdown",
                json!({"documentId":document}),
            )
            .await
            .unwrap();
            assert_eq!(read["markdown"], scenario["expectedMarkdown"], "{name}");
            if !unchanged {
                assert_eq!(read["version"], revised["version"], "{name}");
            }
        }
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.DOCS-R12 — list, read, revise and observe, with Person authority and the
    // typed refusals, each refusal appending nothing.
    #[tokio::test]
    async fn ports_list_read_revise_observe_and_refuse() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        let (collection, document) = ported_document(&mut session).await;
        let mut events = session
            .harness
            .subscribe(
                "teamarness.port.event.v1",
                port_envelope("documents.changed", alice(), json!({})),
            )
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), events.next_event())
                .await
                .is_err(),
            "the event is live-only: no initial event"
        );

        let page = query(&mut session.harness, "document.list", json!({}))
            .await
            .unwrap();
        assert_eq!(
            page["collections"],
            json!([{"id":collection,"name":"Plans","archived":false}])
        );
        assert_eq!(page["documents"][0]["id"], document);
        assert_eq!(page["documents"][0]["collectionId"], collection);
        assert_eq!(page["documents"][0]["title"], "Q4");
        assert_eq!(page["documents"][0]["body"], "available");
        assert!(page.get("next").is_none());
        let read = query(
            &mut session.harness,
            "document.markdown",
            json!({"documentId":document}),
        )
        .await
        .unwrap();
        assert_eq!(
            read,
            json!({"markdown":"","version":page["documents"][0]["version"]})
        );

        // Pages follow Order Key order, and a Collection filter lists only its Documents.
        let first = query(&mut session.harness, "document.list", json!({"limit":1}))
            .await
            .unwrap();
        assert_eq!(first["collections"].as_array().unwrap().len(), 1);
        assert!(first["documents"].as_array().unwrap().is_empty());
        let second = query(
            &mut session.harness,
            "document.list",
            json!({"limit":1,"after":first["next"]}),
        )
        .await
        .unwrap();
        assert_eq!(second["documents"][0]["id"], document);
        assert!(second.get("next").is_none());
        let filtered = query(
            &mut session.harness,
            "document.list",
            json!({"collectionId":collection}),
        )
        .await
        .unwrap();
        assert!(filtered["collections"].as_array().unwrap().is_empty());
        assert_eq!(filtered["documents"].as_array().unwrap().len(), 1);
        for (request, code) in [
            (json!({"after":"nowhere"}), "documents.invalid_cursor"),
            (
                json!({"collectionId":hex(64, 0xdead)}),
                "documents.unknown_collection",
            ),
        ] {
            assert_eq!(
                query(&mut session.harness, "document.list", request)
                    .await
                    .unwrap_err()
                    .code(),
                code
            );
        }

        // Person authority: only the local Installation Person revises.
        let before = appends(&session);
        let stranger = port_as(
            &mut session.harness,
            "command",
            "document.revise",
            json!({"kind":"person","identity":"stranger"}),
            json!({"documentId":document,"baseVersion":read["version"],"markdown":"no\n"}),
        )
        .await
        .unwrap_err();
        assert_eq!(stranger.code(), "documents.actor_unauthorized");
        assert!(
            port_as(
                &mut session.harness,
                "command",
                "document.revise",
                json!({"kind":"agent","identity":ALICE}),
                json!({"documentId":document,"baseVersion":read["version"],"markdown":"no\n"}),
            )
            .await
            .is_err()
        );

        // A base this body never had, and a token in a non-canonical encoding.
        let unknown = ports::encode_version(&Frontiers::from_id(loro::ID::new(0x5eed, 3))).unwrap();
        let mut bytes = URL_SAFE_NO_PAD
            .decode(read["version"].as_str().unwrap())
            .unwrap();
        bytes.extend_from_slice(&bytes.clone());
        for base in [json!(unknown), json!(URL_SAFE_NO_PAD.encode(&bytes))] {
            assert_eq!(
                revise(&mut session.harness, &document, &base, "no\n")
                    .await
                    .unwrap_err()
                    .code(),
                "documents.base_unavailable"
            );
        }
        // A merge that cannot fit one record changes nothing.
        let huge = format!("{}\n", unique_text(170 * 1024));
        assert_eq!(
            revise(&mut session.harness, &document, &read["version"], &huge)
                .await
                .unwrap_err()
                .code(),
            "documents.revision_too_large"
        );
        assert_eq!(appends(&session), before, "no refusal appends a record");

        // One revise is one record, observed live with its Document named.
        let revised = revise(
            &mut session.harness,
            &document,
            &read["version"],
            "* Ship it\n",
        )
        .await
        .unwrap();
        assert_eq!(revised["changed"], true);
        assert_eq!(revised["normalised"], true, "`*` bullets export as `-`");
        assert_eq!(appends(&session), before + 1);
        let event = tokio::time::timeout(Duration::from_secs(2), events.next_event())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            event["$teamarnessPortValue"],
            json!({"documentIds":[document]})
        );
        let read = query(
            &mut session.harness,
            "document.markdown",
            json!({"documentId":document}),
        )
        .await
        .unwrap();
        assert_eq!(read["markdown"], "- Ship it\n");
        assert_eq!(read["version"], revised["version"]);

        // Merging back an unchanged export appends nothing.
        let unchanged = revise(
            &mut session.harness,
            &document,
            &read["version"],
            "- Ship it\n",
        )
        .await
        .unwrap();
        assert_eq!(
            unchanged,
            json!({"changed":false,"normalised":false,"version":read["version"]})
        );
        assert_eq!(appends(&session), before + 1);

        // A listing change carries no Document IDs: the Consumer re-lists.
        session
            .harness
            .request(
                "documents.document.rename",
                json!({"documentId":document,"title":"Q4 plan"}),
            )
            .await
            .unwrap();
        let relisted = async {
            loop {
                let event = events.next_event().await.unwrap().unwrap();
                if event["$teamarnessPortValue"] == json!({}) {
                    return;
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(2), relisted)
            .await
            .expect("a rename invalidates the listing");
        events.close().await;
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.DOCS-R12 — a body that cannot be read now is refused with its typed error.
    #[tokio::test]
    async fn port_reads_refuse_pending_unavailable_and_oversized_bodies() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        let (_, document) = ported_document(&mut session).await;
        let version = query(
            &mut session.harness,
            "document.markdown",
            json!({"documentId":document}),
        )
        .await
        .unwrap()["version"]
            .clone();

        session
            .handles
            .bodies
            .lock()
            .unwrap()
            .get_mut(&document)
            .unwrap()
            .set_unopened(1);
        let pending = query(
            &mut session.harness,
            "document.markdown",
            json!({"documentId":document}),
        )
        .await
        .unwrap_err();
        assert_eq!(pending.code(), "documents.pending");
        assert!(pending.retryable());
        assert_eq!(
            revise(&mut session.harness, &document, &version, "x\n")
                .await
                .unwrap_err()
                .code(),
            "documents.pending"
        );
        let page = query(&mut session.harness, "document.list", json!({}))
            .await
            .unwrap();
        assert_eq!(page["documents"][0]["body"], "pending");
        assert!(page["documents"][0].get("version").is_none());
        session
            .handles
            .bodies
            .lock()
            .unwrap()
            .get_mut(&document)
            .unwrap()
            .set_unopened(0);

        // Valid concurrent edits can grow a body past what one Port value carries.
        let from_alice =
            Writer::of(&session.handles, &document).write(&format!("{}\n", "a".repeat(110 * 1024)));
        let from_bob = Writer::of(&session.handles, &document)
            .write(&format!("{}\n", "\u{0001}😀".repeat(20_000)));
        session
            .handles
            .hub
            .apply_edits(&[(document.clone(), from_alice), (document.clone(), from_bob)]);
        assert_eq!(
            query(
                &mut session.harness,
                "document.markdown",
                json!({"documentId":document}),
            )
            .await
            .unwrap_err()
            .code(),
            "documents.too_large"
        );
        assert_eq!(
            query(
                &mut session.harness,
                "document.markdown",
                json!({"documentId":hex(64, 0xdead)}),
            )
            .await
            .unwrap_err()
            .code(),
            "documents.unknown_document"
        );
        session.harness.shutdown().await.unwrap();
    }

    /// What a person typing into `doc` as its peer sends: `text` inserted at the start or end
    /// of the first text run of top-level block `block`, as one update.
    fn type_into(doc: &loro::LoroDoc, block: usize, at_end: bool, text: &str) -> Vec<u8> {
        use loro::{Container, ValueOrContainer};
        let before = doc.oplog_vv();
        let Some(ValueOrContainer::Container(Container::List(blocks))) =
            doc.get_map("doc").get("children")
        else {
            panic!("the body has a children list");
        };
        let Some(ValueOrContainer::Container(Container::Map(node))) = blocks.get(block) else {
            panic!("the body has block {block}");
        };
        let Some(ValueOrContainer::Container(Container::List(children))) = node.get("children")
        else {
            panic!("block {block} has children");
        };
        // An empty paragraph has no text run yet; typing creates one, as the binding does.
        let run = match children.get(0) {
            Some(ValueOrContainer::Container(Container::Text(run))) => run,
            None => children.insert_container(0, loro::LoroText::new()).unwrap(),
            Some(_) => panic!("block {block} starts with a text run"),
        };
        let position = if at_end { run.len_unicode() } else { 0 };
        run.insert(position, text).unwrap();
        doc.commit();
        doc.export(loro::ExportMode::updates(&before)).unwrap()
    }

    /// One editor session as a UI holds it: the subscription, the Loro doc loaded from its
    /// snapshot, and its peer.
    struct UiSession {
        events: teamarness_package_sdk::testing::HarnessSubscription,
        doc: loro::LoroDoc,
        peer: u64,
        begin: JsonValue,
    }

    impl UiSession {
        async fn open(harness: &mut Harness, document: &str) -> Self {
            Self::open_with(harness, json!({ "documentId": document })).await
        }

        async fn open_with(harness: &mut Harness, request: JsonValue) -> Self {
            let mut events = harness
                .subscribe("documents.document.session", request)
                .await
                .unwrap();
            let begin = next_session_event(&mut events).await;
            assert_eq!(begin["event"], "begin", "{begin}");
            let mut snapshot = Vec::new();
            for index in 0..begin["chunks"].as_u64().unwrap() {
                let chunk = next_session_event(&mut events).await;
                assert_eq!(chunk["event"], "chunk");
                assert_eq!(chunk["chunkIndex"], index);
                snapshot.extend(STANDARD.decode(chunk["chunk"].as_str().unwrap()).unwrap());
            }
            assert_eq!(next_session_event(&mut events).await["event"], "end");
            let doc = loro::LoroDoc::new();
            if !snapshot.is_empty() {
                doc.import(&snapshot).unwrap();
            }
            let peer = begin["sessionPeer"].as_str().unwrap().parse().unwrap();
            doc.set_peer_id(peer).unwrap();
            Self {
                events,
                doc,
                peer,
                begin,
            }
        }

        fn markdown(&self) -> String {
            crate::markdown::export(&self.doc).unwrap_or_default()
        }

        async fn next(&mut self) -> JsonValue {
            next_session_event(&mut self.events).await
        }

        /// Follows the session, importing every relayed update, until `ready` holds for its
        /// Markdown; returns every event it saw.
        async fn follow_until(&mut self, ready: impl Fn(&str) -> bool) -> Vec<JsonValue> {
            let mut seen = Vec::new();
            while !ready(&self.markdown()) {
                let event = self.next().await;
                if event["event"] == "update" {
                    let update = STANDARD.decode(event["update"].as_str().unwrap()).unwrap();
                    self.doc.import(&update).unwrap();
                }
                seen.push(event);
            }
            seen
        }

        /// Sends `update` as this session's request `request_id`.
        async fn send(
            &self,
            harness: &mut Harness,
            document: &str,
            request_id: &str,
            update: &[u8],
        ) -> Result<JsonValue, BackendError> {
            harness
                .request(
                    "documents.document.update",
                    json!({
                        "documentId": document, "sessionPeer": self.peer.to_string(),
                        "requestId": request_id, "update": STANDARD.encode(update),
                    }),
                )
                .await
        }

        async fn close(mut self) {
            self.events.close().await;
        }
    }

    async fn next_session_event(
        events: &mut teamarness_package_sdk::testing::HarnessSubscription,
    ) -> JsonValue {
        tokio::time::timeout(Duration::from_secs(5), events.next_event())
            .await
            .expect("a session event in time")
            .expect("the session is open")
            .expect("the session has not failed")
    }

    /// What a UI opening `document` now would show: the opening's state, plus `text`, the
    /// Markdown of its snapshot.
    async fn body_view(harness: &mut Harness, document: &str) -> JsonValue {
        let session = UiSession::open(harness, document).await;
        let mut view = session.begin.clone();
        view["text"] = json!(session.markdown());
        session.close().await;
        view
    }

    fn unique_text(bytes: usize) -> String {
        let mut text = String::new();
        let mut index = 0u64;
        while text.len() < bytes {
            text.push_str(&format!(
                "{:016x} ",
                index.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ));
            index += 1;
        }
        text.truncate(bytes);
        text.trim_end().to_owned()
    }
}
