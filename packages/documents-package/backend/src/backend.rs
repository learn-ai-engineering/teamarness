use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value as JsonValue, json};
use sha2::{Digest, Sha256};
use teamarness_package_sdk::capabilities::person_names::{PersonNameSnapshot, PersonNames};
use teamarness_package_sdk::capabilities::record_log::{
    Availability, Cursor, OpenResult, RecordEvent, RecordLog, RecordLogSink, ReplayEnd,
    SessionOptions, SyncState, SyncWarning, run_session_from_start,
};
use teamarness_package_sdk::{
    BackendError, CancellationToken, Context, Empty, Router, event_channel,
};
use tokio::sync::{Notify, broadcast, watch};

use crate::body::{BodyError, COALESCE_WINDOW, DocumentBody, TextOp};
use crate::domain::{DomainError, DomainRecord};
use crate::store::{DocumentStore, IngestResult};

/// How many distinct synchronization warning codes are remembered at a time.
const MAX_WARNINGS: usize = 8;

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
        DomainError::InvalidUpdate => error("documents.invalid_update", false),
        DomainError::InvalidMarkdown => error("documents.invalid_markdown", false),
        DomainError::InvalidQuote => error("documents.invalid_quote", false),
        _ => invalid_request(),
    }
}

/// Every Document body this activation holds, keyed by Document identity.
type Bodies = Arc<Mutex<HashMap<String, DocumentBody>>>;

/// The shared pieces that import remote edits and publish text events, used by the request
/// handlers, the record-log sink, and the flusher alike.
#[derive(Clone)]
struct BodyHub {
    store: DocumentStore,
    bodies: Bodies,
    /// Text events for every Document; subscriptions filter by `documentId`.
    events: broadcast::Sender<JsonValue>,
    /// Wakes the flusher when a local edit is pending.
    flush: Arc<Notify>,
    /// Serializes append attempts with requests without holding the body mutex across I/O.
    append_locks: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    waiting_metadata: Arc<Mutex<HashSet<String>>>,
    metadata_lock: Arc<tokio::sync::Mutex<()>>,
    /// This Installation's Loro peer id, stable across restarts and distinct across Installations.
    peer: u64,
    qualified: Arc<AtomicBool>,
    identity_index_complete: Arc<AtomicBool>,
    identity_lookup_lock: Arc<tokio::sync::Mutex<()>>,
}

impl BodyHub {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, DocumentBody>> {
        self.bodies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Runs `with` on the body of `document`, creating it on first use.
    fn with_body<T>(&self, document: &str, with: impl FnOnce(&mut DocumentBody) -> T) -> T {
        let mut bodies = self.lock();
        let body = bodies
            .entry(document.to_owned())
            .or_insert_with(|| DocumentBody::new(self.peer));
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
        let waiting = self.waiting_metadata.lock().unwrap().clone();
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
                .or_insert_with(|| DocumentBody::new(self.peer));
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
        let metadata_ready = self.waiting_metadata.lock().unwrap().is_empty();
        self.identity_index_complete
            .store(bodies_ready && metadata_ready, Ordering::Release);
    }

    fn append_lock(&self, document: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.append_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(document.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// Imports body updates a store ingest made applicable and publishes their deltas.
    fn apply_edits(&self, edits: &[(String, Vec<u8>)]) {
        for (document, update) in edits {
            if self.store.identity_conflicted(document).unwrap_or(true) {
                continue;
            }
            let published = self.with_body(document, |body| match body.import(update) {
                Ok(Some(_)) if !body.editable() => None,
                Ok(Some(delta)) => Some(json!({
                    "documentId": document, "revision": body.revision().to_string(),
                    "origin": "remote", "delta": delta, "editable": body.editable(),
                })),
                Ok(None) => None,
                Err(_) => {
                    eprintln!("documents body import failure: document={document}");
                    None
                }
            });
            if let Some(event) = published {
                if event.to_string().len() > TEXT_EVENT_BUDGET {
                    for full in self.text_events(document) {
                        let _ = self.events.send(full);
                    }
                } else {
                    let _ = self.events.send(event);
                }
            }
        }
    }

    fn text_events(&self, document: &str) -> Vec<JsonValue> {
        match self.store.identity_conflicted(document) {
            Ok(true) => {
                let revision = self.with_body(document, |body| body.revision());
                return vec![
                    json!({"documentId":document, "revision":revision.to_string(),
                    "editable":false, "text":"", "reason":"identity_conflict"}),
                ];
            }
            Err(_) => {
                let revision = self.with_body(document, |body| body.revision());
                return vec![
                    json!({"documentId":document, "revision":revision.to_string(),
                    "editable":false, "text":""}),
                ];
            }
            Ok(false) => {}
        }
        self.with_body(document, |body| {
            full_text_events(
                document,
                body.revision(),
                if body.editable() { body.text() } else { "" },
                body.editable(),
            )
        })
    }

    // The request frame is bounded even when the subscription must deliver the text in parts.
    fn open_response(&self, document: &str) -> JsonValue {
        let first = self.text_events(document).remove(0);
        if first["transfer"] == "begin" {
            json!({ "documentId": document, "revision": first["revision"], "editable": first["editable"] })
        } else {
            first
        }
    }
}

// Reserve plenty of room for the Package UI envelope and JSON escaping of control characters.
const TEXT_EVENT_BUDGET: usize = 96 * 1024;
const TEXT_CHUNK_BYTES: usize = 24 * 1024;

fn full_text_events(document: &str, revision: u64, text: &str, editable: bool) -> Vec<JsonValue> {
    full_text_events_bounded(document, revision, text, editable, TEXT_EVENT_BUDGET)
}

fn full_text_events_bounded(
    document: &str,
    revision: u64,
    text: &str,
    editable: bool,
    budget: usize,
) -> Vec<JsonValue> {
    let base =
        json!({ "documentId": document, "revision": revision.to_string(), "editable": editable });
    let mut small = base.clone();
    small["text"] = json!(text);
    if small.to_string().len() <= budget {
        return vec![small];
    }
    let mut chunks = Vec::new();
    let mut offset = 0;
    while offset < text.len() {
        let mut end = (offset + TEXT_CHUNK_BYTES).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(&text[offset..end]);
        offset = end;
    }
    let mut events = vec![
        json!({ "documentId": document, "revision": revision.to_string(),
        "editable": editable, "transfer": "begin", "chunks": chunks.len() }),
    ];
    for (index, chunk) in chunks.into_iter().enumerate() {
        events.push(
            json!({ "documentId": document, "revision": revision.to_string(),
            "editable": editable, "transfer": "chunk", "chunkIndex": index, "chunk": chunk }),
        );
    }
    events.push(
        json!({ "documentId": document, "revision": revision.to_string(),
        "editable": editable, "transfer": "end" }),
    );
    events
}

/// Derives a Loro peer id from the Installation identity: the first eight bytes of a
/// domain-separated SHA-256, so it is stable across restarts and distinct across Installations.
fn peer_id(installation: &str) -> u64 {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"teamarness.documents.loro-peer\x00");
    hasher.update(installation.as_bytes());
    let digest = hasher.finalize();
    u64::from_be_bytes(digest[..8].try_into().expect("eight bytes"))
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
    names: PersonNames,
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
struct EditRequest {
    document_id: String,
    request_id: String,
    base_revision: String,
    ops: Vec<TextOp>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecoverRequest {
    document_id: String,
    transfer_id: String,
    request_ids: Vec<String>,
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
                replay_store.begin_replay_qualification().map_err(|_| storage_failed())
            }).await?;
            let log = RecordLog::from_initialization(&initialization, capabilities.clone())
                .map_err(|cause| capability_error("record-log", cause))?;
            let names = PersonNames::from_initialization(&initialization, capabilities)
                .map_err(|cause| capability_error("person-names", cause))?;
            let installation = initialization.installation().identity.clone();
            let (events, _) = broadcast::channel(256);
            let hub = BodyHub {
                store: store.clone(),
                bodies: Arc::new(Mutex::new(HashMap::new())),
                events,
                flush: Arc::new(Notify::new()),
                append_locks: Arc::new(Mutex::new(HashMap::new())),
                waiting_metadata: Arc::new(Mutex::new(HashSet::new())),
                metadata_lock: Arc::new(tokio::sync::Mutex::new(())),
                peer: peer_id(&installation),
                qualified: Arc::new(AtomicBool::new(false)),
                identity_index_complete: Arc::new(AtomicBool::new(false)),
                identity_lookup_lock: Arc::new(tokio::sync::Mutex::new(())),
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
            .await.map_err(|_| storage_failed())?.map_err(|_| storage_failed())?;
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
            let record =
                DomainRecord::document_create(&author, &request.collection_id, &request.title)
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
            let hub = &context.state().hub;
            for event in hub.text_events(&request.document_id) {
                let _ = hub.events.send(event);
            }
            Ok(hub.open_response(&request.document_id))
        },
    )
    .request(
        "documents.document.recover",
        |context: Context<DocumentsActive>, request: RecoverRequest| async move {
            require_document(&context, &request.document_id).await?;
            if request.transfer_id.len() != 32 || !request.transfer_id.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                || request.request_ids.len() > 8 || request.request_ids.iter().any(|id| id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))) {
                return Err(invalid_request());
            }
            let hub = &context.state().hub;
            if context.state().store.identity_conflicted(&request.document_id).map_err(|_| storage_failed())? {
                let mut event = hub.text_events(&request.document_id).remove(0);
                event["transferId"] = json!(request.transfer_id);
                event["requestStatuses"] = json!(request.request_ids.iter().map(|id| json!({
                    "requestId":id,"inclusion":"unresolved","durability":"unresolved"
                })).collect::<Vec<_>>());
                let revision = event["revision"].clone();
                let _ = hub.events.send(event);
                return Ok(json!({"documentId":request.document_id,"revision":revision,"transferId":request.transfer_id}));
            }
            let author = &context.state().installation;
            let looked_up = lookup_identities(&context.state().log, &context.state().store, hub, &request.request_ids).await;
            let mut foreign_ids = HashSet::new();
            if let Ok(found) = &looked_up {
                for (id, (document, owner, digest)) in found {
                    if document == &request.document_id && owner == author {
                        hub.remember_identity(document, id, owner, *digest, true);
                    } else {
                        foreign_ids.insert(id.as_str());
                    }
                }
            }
            // All fields, including ID status, are one body-lock snapshot.
            let (revision, text, editable, statuses) = hub.with_body(&request.document_id, |body| {
                let statuses: Vec<JsonValue> = request.request_ids.iter().map(|id| {
                    match body.identity(id).filter(|known| known.author == *author) {
                        Some(known) => json!({"requestId":id,"inclusion":if body.editable() { "included" } else { "unresolved" },"durability": if known.durable { "durable" } else { "pending" }}),
                        None if looked_up.is_ok() && !foreign_ids.contains(id.as_str()) && hub.qualified.load(Ordering::Acquire) && body.editable() => json!({"requestId":id,"inclusion":"absent","durability":"unresolved"}),
                        None => json!({"requestId":id,"inclusion":"unresolved","durability":"unresolved"}),
                    }
                }).collect();
                (body.revision(), if body.editable() { body.text().to_owned() } else { String::new() }, body.editable(), statuses)
            });
            let mut events = full_text_events_bounded(&request.document_id, revision, &text, editable, TEXT_EVENT_BUDGET - 2048);
            for (index, event) in events.iter_mut().enumerate() {
                event["transferId"] = json!(request.transfer_id);
                if index == 0 { event["requestStatuses"] = json!(statuses); }
            }
            for event in events { let _ = hub.events.send(event); }
            Ok(json!({"documentId":request.document_id,"revision":revision.to_string(),"transferId":request.transfer_id}))
        },
    )
    .request(
        "documents.document.edit",
        |context: Context<DocumentsActive>, request: EditRequest| async move {
            require_document(&context, &request.document_id).await?;
            let base: u64 = request
                .base_revision
                .parse()
                .map_err(|_| invalid_request())?;
            let hub = context.state().hub.clone();
            let document = request.document_id.clone();
            let append_lock = hub.append_lock(&document);
            let _append_guard = append_lock.lock().await;
            if context.state().store.identity_conflicted(&document).map_err(|_| storage_failed())? {
                return Err(error("documents.identity_conflict", false));
            }
            if request.request_id.len() != 32 || !request.request_id.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) {
                return Err(invalid_request());
            }
            let author = context.state().installation.clone();
            let digest: [u8; 32] = Sha256::digest(serde_json::to_vec(&(base, &request.ops)).map_err(|_| invalid_request())?).into();
            let _identity_guard = hub.identity_lookup_lock.lock().await;
            let found = lookup_identities(&context.state().log, &context.state().store, &hub, std::slice::from_ref(&request.request_id)).await?;
            if let Some((old_document, old_author, old_digest)) = found.get(&request.request_id) {
                if old_document != &document || old_author != &author || old_digest != &digest {
                    return Err(invalid_request());
                }
                hub.remember_identity(&document, &request.request_id, &author, digest, true);
            }
            // A new ID is absent only after the fenced, gap-free from-start replay and
            // successful opening of every relevant prior edit.
            if found.is_empty()
                && !hub.lock().values().any(|body| body.identity(&request.request_id).is_some())
                && !hub.qualified.load(Ordering::Acquire) {
                return Err(error("documents.document_pending", true));
            }
            // One ID cannot name a request on another Document or Installation.
            if hub.lock().iter().any(|(id, body)| id != &document && body.identity(&request.request_id).is_some()) {
                return Err(invalid_request());
            }
            let (revision, delta, editable, is_retry) = hub.with_body(&document, |body| {
                if !body.editable() {
                    return Err(error("documents.document_pending", true));
                }
                if let Some(known) = body.identity(&request.request_id) {
                    if known.author != author || known.digest != digest {
                        return Err(invalid_request());
                    }
                    return Ok((known.revision, Vec::new(), body.editable(), true));
                }
                if body.revision() != base {
                    return Err(BackendError::new(
                        "documents.stale_revision",
                        false,
                        json!({ "revision": body.revision().to_string() }),
                    ));
                }
                let before = body.text().to_owned();
                match body.apply_identified(&request.ops, request.request_id.clone()) {
                    Ok(_) => {
                        body.remember_request(base, &request.ops);
                        body.remember_identity(request.request_id.clone(), author.clone(), digest, false).map_err(|_| invalid_request())?;
                        Ok((body.revision(), crate::body::diff(&before, body.text()), body.editable(), false))
                    }
                    Err(BodyError::TooLarge) => Err(error("documents.document_too_large", false)),
                    Err(BodyError::RecordTooLarge) => Err(error("documents.edit_request_too_large", false)),
                    Err(BodyError::OutOfRange) => Err(invalid_request()),
                    Err(BodyError::Loro) => Err(error("documents.edit_failed", true)),
                }
            })?;
            if !is_retry {
                hub.flush.notify_one();
                let event = json!({
                    "documentId": document, "revision": revision.to_string(),
                    "origin": "local", "requestId": request.request_id, "delta": delta, "editable": editable,
                });
                if event.to_string().len() > TEXT_EVENT_BUDGET {
                    for full in hub.text_events(&document) {
                        let _ = hub.events.send(full);
                    }
                } else {
                    let _ = hub.events.send(event);
                }
            }
            // Keep the draft visible during the coalescing window, but never acknowledge it
            // until every export in this request has reached the Core record log.
            if !is_retry {
                tokio::time::sleep(COALESCE_WINDOW).await;
            }
            drain_document(&hub, &document, true, |update| {
                append_edit(
                    &context.state().log,
                    &hub,
                    &context.state().changes,
                    &context.state().installation,
                    &document,
                    update,
                )
            })
            .await?;
            Ok(json!({ "ok": true, "revision": revision.to_string(), "requestId": request.request_id }))
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
        "documents.document.text",
        |context: Context<DocumentsActive>, request: DocumentRequest| async move {
            require_document(&context, &request.document_id).await?;
            let hub = context.state().hub.clone();
            let document = request.document_id;
            let (sender, subscription) = event_channel(64);
            let mut events = hub.events.subscribe();
            tokio::spawn(async move {
                // The initial full text is taken after subscribing, so no delta between the two
                // is lost; a delta older than the initial revision is skipped below.
                let initial = hub.text_events(&document);
                let mut revision = revision_of(&initial[0]);
                for event in initial {
                    if sender.send(event).await.is_err() {
                        return;
                    }
                }
                loop {
                    let event = tokio::select! {
                        event = events.recv() => event,
                        () = sender.closed() => return,
                    };
                    match event {
                        Ok(event) => {
                            if event["documentId"].as_str() != Some(document.as_str()) {
                                continue;
                            }
                            let next = revision_of(&event);
                            if next < revision
                                || (next == revision
                                    && event.get("transfer").is_none()
                                    && event.get("text").is_none())
                            {
                                continue;
                            }
                            if next > revision.saturating_add(1) {
                                let full = hub.text_events(&document);
                                revision = revision_of(&full[0]);
                                for part in full {
                                    if sender.send(part).await.is_err() {
                                        return;
                                    }
                                }
                                continue;
                            }
                            revision = next;
                            if sender.send(event).await.is_err() {
                                return;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            // Resynchronize with the full text rather than guess what was missed.
                            let full = hub.text_events(&document);
                            revision = revision_of(&full[0]);
                            for part in full {
                                if sender.send(part).await.is_err() {
                                    return;
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => return,
                    }
                }
            });
            Ok(subscription)
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

/// Splits a roster into events of at most 64 Persons, which fit the bridge limits with room.
fn name_snapshots(snapshot: PersonNameSnapshot) -> Vec<JsonValue> {
    const PERSONS_PER_EVENT: usize = 64;
    let persons: Vec<_> = snapshot
        .persons
        .into_iter()
        .map(|person| {
            json!({
                "identity": person.identity, "personId": person.person_id,
                "name": person.name, "fingerprint": person.fingerprint,
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

fn revision_of(event: &JsonValue) -> u64 {
    event["revision"]
        .as_str()
        .and_then(|revision| revision.parse().ok())
        .unwrap_or(0)
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

/// Read only the exact indexed sealed row to confirm a local retry's immutable digest.
/// A missing row remains unresolved until the qualified replay and all opens succeed.
async fn lookup_identities(
    log: &RecordLog,
    store: &DocumentStore,
    hub: &BodyHub,
    ids: &[String],
) -> Result<HashMap<String, (String, String, [u8; 32])>, BackendError> {
    let mut found = HashMap::new();
    for id in ids {
        if let Some(cached) = hub.lock().iter().find_map(|(document, body)| {
            body.identity(id)
                .map(|known| (document.clone(), known.author.clone(), known.digest))
        }) {
            found.insert(id.clone(), cached);
            continue;
        }
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
        found.insert(id.clone(), (document, author, payload_digest));
    }
    Ok(found)
}

/// Re-opens the edit records of one Document whose sealed bytes could not be opened before,
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
        .blocking(move |_| store.edit_records(Some(&id)).map_err(|_| storage_failed()))
        .await?;
    let unopened = import_edit_rows(&context.state().log, &hub, &rows).await?;
    let changed = hub.with_body(document, |body| {
        let changed = body.unopened() != unopened;
        body.set_unopened(unopened);
        changed
    });
    if changed {
        hub.refresh_identity_index_complete();
        for event in hub.text_events(document) {
            let _ = hub.events.send(event);
        }
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
                        Ok(DomainRecord::DocumentEdit {
                            document, update, ..
                        }) => hub.apply_edits(&[(document, update)]),
                        Ok(DomainRecord::IdentifiedDocumentEdit {
                            author,
                            document,
                            request_id,
                            payload_digest,
                            update,
                        }) => {
                            match hub.store.index_opened_edit(row, &record.canonical) {
                                Ok(false) => {
                                    // A newly opened collision invalidates a previously visible body.
                                    for affected in hub
                                        .store
                                        .conflicted_documents()
                                        .map_err(|_| storage_failed())?
                                    {
                                        for event in hub.text_events(&affected) {
                                            let _ = hub.events.send(event);
                                        }
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
                            hub.apply_edits(&[(document.clone(), update)]);
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
                .store
                .ingest_with_commit(
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
    context.state().hub.apply_edits(&ingested.edits);
    match ingested.result {
        IngestResult::Applied | IngestResult::Duplicate => {}
        IngestResult::Deferred(_) => return Err(error("documents.deferred", true)),
        IngestResult::Quarantined(_) | IngestResult::IdentityConflict(_) => {
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
    if waiting_only && hub.waiting_metadata.lock().unwrap().is_empty() {
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
            let waiting = hub.waiting_metadata.lock().unwrap();
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
                        hub.waiting_metadata
                            .lock()
                            .unwrap()
                            .insert(row.record_id.clone());
                    }
                    OpenResult::Ready(record) => {
                        if deferred && row.purpose == DomainRecord::EDIT_KIND {
                            if record.record_id.as_str() != row.record_id
                                || record.order_key.as_str() != row.order_key
                                || record.author != row.author
                                || record.purpose != row.purpose
                                || record.signature != row.signature
                            {
                                hub.waiting_metadata
                                    .lock()
                                    .unwrap()
                                    .insert(row.record_id.clone());
                                continue;
                            }
                            if !store
                                .index_opened_edit(row, &record.canonical)
                                .map_err(|_| storage_failed())?
                            {
                                hub.waiting_metadata.lock().unwrap().remove(&row.record_id);
                                for affected in
                                    store.conflicted_documents().map_err(|_| storage_failed())?
                                {
                                    for event in hub.text_events(&affected) {
                                        let _ = hub.events.send(event);
                                    }
                                }
                                continue;
                            }
                        }
                        if deferred {
                            let ingested = store
                                .retry_deferred(&row.record_id, &record.canonical)
                                .map_err(|_| storage_failed())?;
                            hub.apply_edits(&ingested.edits);
                        } else {
                            store
                                .remember_opened(&row.record_id, &row.order_key, &record.canonical)
                                .map_err(|_| storage_failed())?;
                        }
                        hub.waiting_metadata.lock().unwrap().remove(&row.record_id);
                    }
                    OpenResult::WrongScope | OpenResult::Corrupt => {
                        let mut waiting = hub.waiting_metadata.lock().unwrap();
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
    // Rebuild every body from its edit records; a Document whose records cannot be opened yet
    // stays pending until `documents.document.open` retries it.
    let reader = store.clone();
    let edits = teamarness_package_sdk::blocking(cancellation, move |_| {
        reader.edit_records(None).map_err(|_| storage_failed())
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

/// Appends due local edits as `document.edit` records.
///
/// One task per activation wakes on a local edit and then every 100 ms while anything is
/// pending, so a burst of keystrokes becomes one record once the coalescing window passes.
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
            let pending: Vec<String> = hub
                .lock()
                .iter()
                .filter(|(id, body)| {
                    body.has_pending() && !hub.store.identity_conflicted(id).unwrap_or(true)
                })
                .map(|(id, _)| id.clone())
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
                if let Err(cause) = drain_document(&hub, &document, false, |update| {
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
                () = tokio::time::sleep(COALESCE_WINDOW.min(Duration::from_millis(100))) => {}
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
    force: bool,
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
        let next = hub.with_body(document, |body| {
            body.flush(Instant::now(), force)
                .and_then(|flushed| flushed.updates.into_iter().next())
        });
        let Some(update) = next else {
            if appended {
                let saved_revision = hub.with_body(document, |body| body.last_request_revision());
                if let Some(saved_revision) = saved_revision {
                    let mut events = hub.text_events(document);
                    if let Some(last) = events.last_mut() {
                        last["savedRevision"] = json!(saved_revision.to_string());
                        if let Some(id) = completed_id {
                            last["requestId"] = json!(id);
                        }
                    }
                    for event in events {
                        let _ = hub.events.send(event);
                    }
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
    let record = if let Some((id, digest)) = identity.as_ref() {
        DomainRecord::identified_document_edit(author, document, id, digest, update)
    } else {
        DomainRecord::document_edit(author, document, update)
    }
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
    let appended = log
        .append(purpose, &canonical)
        .await
        .map_err(|cause| capability_error(purpose, cause))?;
    let store = hub.store.clone();
    let changes = changes.clone();
    let ingested = tokio::task::spawn_blocking(move || {
        store
            .ingest_with_commit(
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
                return Err(error("documents.request_id_conflict", false));
            }
            if hub.store.identity_conflicted(document).unwrap_or(true) {
                return Err(error("documents.identity_conflict", false));
            }
            if let Some((id, digest)) = &identity {
                hub.remember_identity(document, id, author, *digest, true);
            }
            let others: Vec<(String, Vec<u8>)> = ingested
                .edits
                .into_iter()
                .filter(|(target, _)| target != document)
                .collect();
            hub.apply_edits(&others);
        }
        Ok(Err(cause)) => {
            if let Some((id, digest)) = &identity {
                hub.remember_identity(document, id, author, *digest, true);
            }
            eprintln!(
                "documents edit projection failure after append: {}",
                cause.code()
            );
        }
        Err(cause) => {
            if let Some((id, digest)) = &identity {
                hub.remember_identity(document, id, author, *digest, true);
            }
            eprintln!("documents edit projection task failed after append: {cause}");
        }
    }
    Ok(())
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
            drain_document(&state.hub, &document, true, |update| {
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
        if matches!(
            DomainRecord::decode(&record.canonical),
            Ok(DomainRecord::IdentifiedDocumentEdit { .. })
        ) && !self.hub.identity_index_complete.load(Ordering::Acquire)
        {
            return Err(error("documents.document_pending", true));
        }
        let ingested = self
            .store
            .ingest_with_commit(
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
                for event in self.hub.text_events(&affected) {
                    let _ = self.hub.events.send(event);
                }
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
        self.hub.apply_edits(&ingested.edits);
        if matches!(
            ingested.result,
            IngestResult::Applied | IngestResult::Duplicate
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
        }
        Ok(())
    }

    fn reset_cursor(&self) -> Result<(), BackendError> {
        self.store.reset_cursor().map_err(|_| storage_failed())
    }

    fn on_qualified_replay(&self, _boundary: &ReplayEnd) {
        let complete = self.store.replayed_previous_projection().unwrap_or(false)
            && !self.store.has_deferred_records().unwrap_or(true)
            && !self
                .store
                .has_unresolved_projection_warnings()
                .unwrap_or(true)
            && self.hub.identity_index_complete.load(Ordering::Acquire);
        let bodies = self.hub.lock();
        self.hub.qualified.store(
            complete
                && self.hub.waiting_metadata.lock().unwrap().is_empty()
                && bodies.values().all(DocumentBody::editable),
            Ordering::Release,
        );
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
        if self.hub.waiting_metadata.lock().unwrap().is_empty() {
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
                    for event in hub.text_events(&document) {
                        let _ = hub.events.send(event);
                    }
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

    #[test]
    fn full_text_and_oversized_delta_frames_stay_below_bridge_limit() {
        let text = "\u{0000}😀".repeat(60_000);
        let events = full_text_events(&hex(64, 1), 42, &text, true);
        assert!(events.len() > 3);
        assert_eq!(events[0]["transfer"], "begin");
        assert_eq!(events.last().unwrap()["transfer"], "end");
        assert_eq!(
            events
                .iter()
                .filter_map(|event| event["chunk"].as_str())
                .collect::<String>(),
            text
        );
        assert!(
            events
                .iter()
                .all(|event| event.to_string().len() < 256 * 1024)
        );
    }

    #[test]
    fn oversized_import_delta_falls_back_to_bounded_full_text_events() {
        let directory = tempfile::tempdir().unwrap();
        let store = DocumentStore::open(directory.path().join("documents.sqlite3")).unwrap();
        let (events, mut receiver) = broadcast::channel(256);
        let hub = BodyHub {
            store,
            bodies: Arc::new(Mutex::new(HashMap::new())),
            events,
            flush: Arc::new(Notify::new()),
            append_locks: Arc::new(Mutex::new(HashMap::new())),
            waiting_metadata: Arc::new(Mutex::new(HashSet::new())),
            metadata_lock: Arc::new(tokio::sync::Mutex::new(())),
            peer: 3,
            qualified: Arc::new(AtomicBool::new(false)),
            identity_index_complete: Arc::new(AtomicBool::new(false)),
            identity_lookup_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        let mut alice = DocumentBody::new(1);
        let mut bob = DocumentBody::new(2);
        alice
            .apply(&[TextOp::Insert {
                insert: "a".repeat(110 * 1024),
            }])
            .unwrap();
        bob.apply(&[TextOp::Insert {
            insert: "\u{0000}😀".repeat(20_000),
        }])
        .unwrap();
        for update in alice.flush(Instant::now(), true).unwrap().updates {
            hub.apply_edits(&[("doc".into(), update)]);
            while receiver.try_recv().is_ok() {}
        }
        for update in bob.flush(Instant::now(), true).unwrap().updates {
            hub.apply_edits(&[("doc".into(), update)]);
        }
        let merged = hub.with_body("doc", |body| body.text().to_owned());
        assert!(merged.len() > crate::body::MAX_TEXT_BYTES);
        let published: Vec<_> = std::iter::from_fn(|| receiver.try_recv().ok()).collect();
        assert!(
            published
                .iter()
                .any(|event: &JsonValue| event["transfer"] == "begin")
        );
        assert!(
            published
                .iter()
                .all(|event| event.to_string().len() < 256 * 1024)
        );
        let response = hub.open_response("doc");
        assert!(response.get("text").is_none());
        assert!(response.to_string().len() < 256 * 1024);
        let full = hub.text_events("doc");
        assert!(
            full.iter()
                .all(|event| event.to_string().len() < 256 * 1024)
        );
        assert_eq!(
            full.iter()
                .filter_map(|event| event["chunk"].as_str())
                .collect::<String>(),
            merged
        );
    }

    #[test]
    fn pending_body_never_publishes_partial_ready_edits() {
        let directory = tempfile::tempdir().unwrap();
        let store = DocumentStore::open(directory.path().join("documents.sqlite3")).unwrap();
        let (events, mut receiver) = broadcast::channel(16);
        let hub = BodyHub {
            store,
            bodies: Arc::new(Mutex::new(HashMap::new())),
            events,
            flush: Arc::new(Notify::new()),
            append_locks: Arc::new(Mutex::new(HashMap::new())),
            waiting_metadata: Arc::new(Mutex::new(HashSet::new())),
            metadata_lock: Arc::new(tokio::sync::Mutex::new(())),
            peer: 3,
            qualified: Arc::new(AtomicBool::new(false)),
            identity_index_complete: Arc::new(AtomicBool::new(false)),
            identity_lookup_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        let mut source = DocumentBody::new(1);
        source
            .apply(&[TextOp::Insert {
                insert: "complete".into(),
            }])
            .unwrap();
        let update = source
            .flush(Instant::now(), true)
            .unwrap()
            .updates
            .remove(0);
        hub.with_body("doc", |body| body.set_unopened(2));
        hub.apply_edits(&[("doc".into(), update.clone())]);
        assert_eq!(hub.open_response("doc")["text"], "");
        assert!(
            receiver.try_recv().is_err(),
            "no partial delta is broadcast"
        );
        hub.with_body("doc", |body| body.set_unopened(0));
        assert_eq!(hub.open_response("doc")["text"], "complete");
        assert_eq!(hub.open_response("doc")["editable"], true);
    }

    #[test]
    fn router_matches_the_ui_contract() {
        assert_router_matches_contract(
            &router().operations(),
            include_str!("../../ui-contract.json"),
        );
    }

    /// Loro is linked and its text container round-trips an edit; the body model builds on it
    /// in a later phase.
    #[test]
    fn loro_text_round_trips() {
        let doc = loro::LoroDoc::new();
        let text = doc.get_text("body");
        text.insert(0, "# Plan").unwrap();
        doc.commit();
        assert_eq!(text.to_string(), "# Plan");
    }

    use std::time::Duration;

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use teamarness_package_sdk::testing::{Harness, HarnessBuilder, ScriptedCapability};

    const RECORD_LOG: &str = "teamarness.synchronization.immutable-record-log";
    const ALICE: &str = "alice";

    fn hex(length: usize, seed: u64) -> String {
        format!("{seed:0length$x}")
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
                .capability("teamarness.person-names", ScriptedCapability::new())
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
        for (seed, peer, text) in [(4, 91, "one"), (5, 92, "two")] {
            let mut body = DocumentBody::new(peer);
            body.apply(&[TextOp::Insert {
                insert: text.into(),
            }])
            .unwrap();
            let update = body.flush(Instant::now(), true).unwrap().updates.remove(0);
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
                    DomainRecord::document_create("bob", &hex(64, 1), "Notes").unwrap(),
                ),
                (
                    3,
                    DomainRecord::document_create("bob", &hex(64, 1), "Other").unwrap(),
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
            let hidden = session
                .harness
                .request("documents.document.open", json!({"documentId":document}))
                .await
                .unwrap();
            assert_eq!(hidden["reason"], "identity_conflict");
            assert_eq!(hidden["text"], "");
            session.handles.qualified.store(true, Ordering::Release);
            let mut subscription = session
                .harness
                .subscribe("documents.document.text", json!({"documentId":document}))
                .await
                .unwrap();
            let transfer = hex(32, 88);
            session
                .harness
                .request(
                    "documents.document.recover",
                    json!({"documentId":document,
                "transferId":transfer,"requestIds":[hex(32, 89)]}),
                )
                .await
                .unwrap();
            let recovered = tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    let event = subscription.next_event().await.unwrap().unwrap();
                    if event["transferId"] == transfer {
                        break event;
                    }
                }
            })
            .await
            .unwrap();
            assert_eq!(recovered["reason"], "identity_conflict");
            assert_eq!(recovered["requestStatuses"][0]["inclusion"], "unresolved");
            assert_eq!(recovered["text"], "");
            let rejected = session
                .harness
                .request(
                    "documents.document.edit",
                    json!({"documentId":document,
                "baseRevision":"0","requestId":hex(32, 89),"ops":[{"insert":"bad"}]}),
                )
                .await
                .unwrap_err();
            assert_eq!(rejected.code(), "documents.identity_conflict");
            let mut unrelated_body = DocumentBody::new(93);
            unrelated_body
                .apply(&[TextOp::Insert {
                    insert: "live".into(),
                }])
                .unwrap();
            let unrelated_update = unrelated_body
                .flush(Instant::now(), true)
                .unwrap()
                .updates
                .remove(0);
            let unrelated = DomainRecord::document_edit("bob", &other, unrelated_update).unwrap();
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
                "live"
            );
            assert!(!store.identity_conflicted(&other).unwrap());
            session.harness.shutdown().await.unwrap();
        }
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
                    &DomainRecord::document_create("bob", &hex(64, 1), "Notes").unwrap()
                ))
                .await
        );
        let mut source = DocumentBody::new(91);
        source
            .apply(&[TextOp::Insert {
                insert: "first".into(),
            }])
            .unwrap();
        let first_update = source
            .flush(Instant::now(), true)
            .unwrap()
            .updates
            .remove(0);
        source.append_succeeded();
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
        let mut attacker = DocumentBody::new(92);
        attacker
            .apply(&[TextOp::Insert {
                insert: "bad".into(),
            }])
            .unwrap();
        let bad = attacker
            .flush(Instant::now(), true)
            .unwrap()
            .updates
            .remove(0);
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
        source
            .apply(&[
                TextOp::Retain { retain: 5 },
                TextOp::Insert {
                    insert: "later".into(),
                },
            ])
            .unwrap();
        let next = source
            .flush(Instant::now(), true)
            .unwrap()
            .updates
            .remove(0);
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
            "first",
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

    #[tokio::test]
    async fn unopenable_deferred_identity_hides_applied_body_until_collision_is_known() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("documents.sqlite3");
        let document = hex(64, 2);
        let id = hex(32, 9);
        let mut body = DocumentBody::new(91);
        body.apply(&[TextOp::Insert {
            insert: "private".into(),
        }])
        .unwrap();
        let update = body.flush(Instant::now(), true).unwrap().updates.remove(0);
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
                    DomainRecord::document_create("bob", &hex(64, 1), "Notes").unwrap(),
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
        connection.execute_batch("DROP TABLE edit_identities; DROP TABLE document_identity_conflicts; PRAGMA user_version=4;").unwrap();
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
        let pending = session
            .harness
            .request("documents.document.open", json!({"documentId":document}))
            .await
            .unwrap();
        assert_eq!(pending["editable"], false);
        assert_eq!(pending["text"], "");
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
        let hidden = session
            .harness
            .request("documents.document.open", json!({"documentId":document}))
            .await
            .unwrap();
        assert_eq!(hidden["reason"], "identity_conflict");
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn migrated_deferred_identity_is_indexed_before_remote_collision() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("documents.sqlite3");
        let absent_document = hex(64, 9);
        let request_id = hex(32, 13);
        let mut source = DocumentBody::new(91);
        source
            .apply(&[TextOp::Insert {
                insert: "safe".into(),
            }])
            .unwrap();
        let update = source
            .flush(Instant::now(), true)
            .unwrap()
            .updates
            .remove(0);
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
            let doc = DomainRecord::document_create("bob", &hex(64, 1), "Notes").unwrap();
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
            .execute_batch("DROP TABLE edit_identities; DROP TABLE document_identity_conflicts; PRAGMA user_version=4;")
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
            "safe"
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
                    &DomainRecord::document_create("bob", &hex(64, 1), "Notes").unwrap()
                ))
                .await
        );
        let store = session.handles.store.clone();
        wait_until(
            || store.document_exists(&document).unwrap(),
            "document arrives",
        )
        .await;
        let mut text = session
            .harness
            .subscribe("documents.document.text", json!({"documentId":document}))
            .await
            .unwrap();
        let _ = text.next_event().await.unwrap();
        let before = hex(32, 90);
        session
            .harness
            .request(
                "documents.document.recover",
                json!({"documentId":document,"transferId":before,"requestIds":[id]}),
            )
            .await
            .unwrap();
        let unresolved = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let event = text.next_event().await.unwrap().unwrap();
                if event["transferId"] == before {
                    break event;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(unresolved["requestStatuses"][0]["inclusion"], "unresolved");
        let mut source = DocumentBody::new(94);
        source
            .apply(&[TextOp::Insert {
                insert: "replayed".into(),
            }])
            .unwrap();
        let update = source
            .flush(Instant::now(), true)
            .unwrap()
            .updates
            .remove(0);
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
        let next_transfer = hex(32, 91);
        session.harness.request("documents.document.recover",
            json!({"documentId":document,"transferId":next_transfer,"requestIds":[id,hex(32,89)]})).await.unwrap();
        let received = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let event = text.next_event().await.unwrap().unwrap();
                if event["transferId"] == next_transfer {
                    break event;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(received["requestStatuses"][0]["inclusion"], "included");
        assert_eq!(received["requestStatuses"][1]["inclusion"], "absent");
        assert_eq!(store.edit_identity(&id).unwrap().unwrap().0, hex(64, 3));
        text.close().await;
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
        let title = DomainRecord::document_create("bob", &hex(64, 1), "Notes").unwrap();
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
        let document = DomainRecord::document_create("bob", &hex(64, 1), "Notes").unwrap();
        let comment = DomainRecord::comment_add("bob", &hex(64, 2), None, None, "Hello").unwrap();
        let deferred = DomainRecord::document_create("bob", &hex(64, 99), "Later").unwrap();
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
        let mut body = DocumentBody::new(90);
        body.apply(&[TextOp::Insert {
            insert: "recovered".into(),
        }])
        .unwrap();
        let update = body.flush(Instant::now(), true).unwrap().updates.remove(0);
        let edit = DomainRecord::document_edit("bob", &document, update).unwrap();
        let store = DocumentStore::open(directory.path().join("documents.sqlite3")).unwrap();
        let mut seed = 1;
        let mut save = |record: DomainRecord, target: u8| {
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
            DomainRecord::document_create("bob", &collection, "Notes").unwrap(),
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
        for _ in 0..17 {
            save(edit.clone(), 5);
        }
        let access = Arc::new(AtomicBool::new(false));
        let gained = access.clone();
        let canonical = URL_SAFE_NO_PAD.encode(edit.encode());
        let scripted = ScriptedCapability::new().calls(move |method, payload| match method {
            "open" => {
                let sealed = payload["sealedRecords"].as_array().unwrap();
                let total: usize = sealed.iter().map(|value| URL_SAFE_NO_PAD.decode(value.as_str().unwrap()).unwrap().len()).sum();
                assert!(sealed.len() <= 32 && total <= 512 * 1024);
                Ok(json!({"results": sealed.iter().map(|value| {
                    let bytes = URL_SAFE_NO_PAD.decode(value.as_str().unwrap()).unwrap();
                    if bytes[0] == 5 && (gained.load(Ordering::SeqCst) || bytes[1] == 37) {
                        json!({"status":"ready", "recordId":hex(64, u64::from(bytes[1])), "orderKey":hex(82, u64::from(bytes[1])),
                            "author":"bob", "purpose":"document.edit", "signature":"sig", "canonical":canonical})
                    } else { json!({"status":"waiting_for_history_access"}) }
                }).collect::<Vec<_>>() }))
            }
            _ => Err(error("unavailable", false)),
        });
        let mut session = start(directory.path(), scripted.clone()).await;
        assert_eq!(
            session.handles.bodies.lock().unwrap()[&document].unopened(),
            16
        );
        let mut subscription = session
            .harness
            .subscribe("documents.document.text", json!({"documentId":document}))
            .await
            .unwrap();
        assert_eq!(
            subscription.next_event().await.unwrap().unwrap()["editable"],
            false
        );
        let pending = session
            .harness
            .request("documents.document.open", json!({"documentId":document}))
            .await
            .unwrap();
        assert_eq!(pending["editable"], false);
        assert_eq!(
            pending["text"], "",
            "one ready edit must not reveal partial body"
        );
        access.store(true, Ordering::SeqCst);
        let ready = session
            .harness
            .request("documents.document.open", json!({"documentId":document}))
            .await
            .unwrap();
        assert_eq!(ready["editable"], true);
        assert_eq!(ready["text"], "recovered");
        assert_eq!(
            session.handles.bodies.lock().unwrap()[&document].unopened(),
            0
        );
        let mut published = false;
        for _ in 0..4 {
            if subscription.next_event().await.unwrap().unwrap()["editable"] == true {
                published = true;
                break;
            }
        }
        assert!(
            published,
            "the existing subscription sees the final editable state"
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

    #[tokio::test]
    async fn edit_failure_is_retryable_and_retries_without_duplicate_operations() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let fail = Arc::new(AtomicBool::new(true));
        let failure = fail.clone();
        let appended = Arc::new(std::sync::atomic::AtomicU64::new(100));
        let scripted = ScriptedCapability::new().calls(move |method, payload| {
            if method == "append" && payload["purpose"] == "document.edit"
                && failure.load(Ordering::SeqCst) {
                return Err(error("unavailable", true));
            }
            match method {
                "open" => Ok(json!({ "results": (0..payload["sealedRecords"].as_array().map_or(0, Vec::len))
                    .map(|_| json!({ "status": "waiting_for_history_access" })).collect::<Vec<_>>() })),
                "append" => {
                    let seed = appended.fetch_add(1, Ordering::SeqCst);
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
        let mut text = session
            .harness
            .subscribe("documents.document.text", json!({"documentId":document}))
            .await
            .unwrap();
        let _ = text.next_event().await.unwrap().unwrap();
        let request = json!({"documentId": document, "requestId":"11111111111111111111111111111111", "baseRevision":"0", "ops":[{"insert":"draft"}]});
        let result = session
            .harness
            .request("documents.document.edit", request.clone())
            .await;
        assert_eq!(
            result.unwrap_err().code(),
            "documents.capability_unavailable"
        );
        assert!(session.handles.bodies.lock().unwrap()[&document].has_pending());
        fail.store(false, Ordering::SeqCst);
        let handles = session.handles.clone();
        let waiting_document = document.clone();
        wait_until(
            || !handles.bodies.lock().unwrap()[&waiting_document].has_pending(),
            "background append retry",
        )
        .await;
        let saved = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let event = text.next_event().await.unwrap().unwrap();
                if event.get("savedRevision").is_some() {
                    break event;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(saved["savedRevision"], "1");
        assert_eq!(saved["text"], "draft");
        let response = session
            .harness
            .request("documents.document.edit", request)
            .await
            .unwrap();
        assert_eq!(response["revision"], "1");
        assert!(!session.handles.bodies.lock().unwrap()[&document].has_pending());
        assert_eq!(
            session.handles.bodies.lock().unwrap()[&document].text(),
            "draft"
        );
        fail.store(true, Ordering::SeqCst);
        let second = json!({"documentId": document, "requestId":"22222222222222222222222222222222", "baseRevision":"1", "ops":[{"retain":5},{"insert":"!"}]});
        assert!(
            session
                .harness
                .request("documents.document.edit", second)
                .await
                .is_err()
        );
        assert!(session.handles.bodies.lock().unwrap()[&document].has_pending());
        fail.store(false, Ordering::SeqCst);
        session.harness.dispose().await.unwrap();
        assert!(
            !session.handles.bodies.lock().unwrap()[&document].has_pending(),
            "graceful dispose drains outstanding edits"
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
        let response = first
            .harness
            .request(
                "documents.document.edit",
                json!({"documentId": document, "requestId":"33333333333333333333333333333333", "baseRevision":"0", "ops":[{"insert":"durable"}]}),
            )
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
            "durable"
        );
        assert!(!restarted.handles.bodies.lock().unwrap()[&document].has_pending());
        assert!(
            restarted.handles.bodies.lock().unwrap()[&document]
                .identity("33333333333333333333333333333333")
                .is_none(),
            "the old ID was evicted from the bounded cache"
        );
        let old_edit = json!({"documentId":document,"requestId":"33333333333333333333333333333333","baseRevision":"0","ops":[{"insert":"durable"}]});
        let old_count = records.lock().unwrap().len();
        fail_lookup.store(true, Ordering::SeqCst);
        assert_eq!(
            restarted
                .harness
                .request("documents.document.edit", old_edit.clone())
                .await
                .unwrap_err()
                .code(),
            "documents.capability_unavailable"
        );
        let mut text = restarted
            .harness
            .subscribe("documents.document.text", json!({"documentId":document}))
            .await
            .unwrap();
        let _ = text.next_event().await.unwrap();
        let transfer = "abababababababababababababababab";
        let missing = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
        let query = json!({"documentId":document,"transferId":transfer,"requestIds":["33333333333333333333333333333333",missing]});
        restarted
            .harness
            .request("documents.document.recover", query.clone())
            .await
            .unwrap();
        let first = tokio::time::timeout(Duration::from_secs(2), text.next_event())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(first["transferId"], transfer);
        assert_eq!(
            first["requestStatuses"][0]["inclusion"], "unresolved",
            "failed sealed lookup is not an absence proof"
        );
        assert_eq!(first["requestStatuses"][1]["inclusion"], "unresolved");
        fail_lookup.store(false, Ordering::SeqCst);
        let repeated = restarted
            .harness
            .request("documents.document.edit", old_edit.clone())
            .await
            .unwrap();
        assert_eq!(repeated["revision"], "1");
        assert_eq!(
            records.lock().unwrap().len(),
            old_count,
            "evicted retry must not append again"
        );
        let conflict = json!({"documentId":document,"requestId":"33333333333333333333333333333333","baseRevision":"0","ops":[{"insert":"other"}]});
        assert_eq!(
            restarted
                .harness
                .request("documents.document.edit", conflict)
                .await
                .unwrap_err()
                .code(),
            "documents.invalid_request"
        );
        restarted
            .harness
            .request("documents.document.recover", query.clone())
            .await
            .unwrap();
        let found = text.next_event().await.unwrap().unwrap();
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
                restarted
                    .harness
                    .request("documents.document.recover", query.clone())
                    .await
                    .unwrap();
                let event = text.next_event().await.unwrap().unwrap();
                if event["requestStatuses"][1]["inclusion"] == "absent" {
                    break event;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(absent["requestStatuses"][0]["durability"], "durable");
        text.close().await;
        restarted.harness.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn partial_append_failure_retries_only_unacknowledged_exports() {
        let directory = tempfile::tempdir().unwrap();
        let store = DocumentStore::open(directory.path().join("documents.sqlite3")).unwrap();
        let (events, _) = broadcast::channel(8);
        let hub = BodyHub {
            store,
            bodies: Arc::new(Mutex::new(HashMap::new())),
            events,
            flush: Arc::new(Notify::new()),
            append_locks: Arc::new(Mutex::new(HashMap::new())),
            waiting_metadata: Arc::new(Mutex::new(HashSet::new())),
            metadata_lock: Arc::new(tokio::sync::Mutex::new(())),
            peer: 1,
            qualified: Arc::new(AtomicBool::new(false)),
            identity_index_complete: Arc::new(AtomicBool::new(false)),
            identity_lookup_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        let text = "x".repeat(150 * 1024);
        hub.with_body("doc", |body| {
            body.apply(&[TextOp::Insert {
                insert: text.clone(),
            }])
            .unwrap();
        });
        let exports = hub.with_body("doc", |body| {
            body.flush(Instant::now(), true).unwrap().updates
        });
        assert!(exports.len() >= 2);
        let mut appended = Vec::new();
        let failure = drain_document(&hub, "doc", true, |update| {
            if appended.is_empty() {
                appended.push(update);
                std::future::ready(Ok(()))
            } else {
                std::future::ready(Err(error("documents.capability_unavailable", true)))
            }
        })
        .await;
        assert!(
            failure.is_err(),
            "the request cannot acknowledge a partial append"
        );
        assert!(hub.with_body("doc", |body| body.has_pending()));
        drain_document(&hub, "doc", true, |update| {
            appended.push(update);
            std::future::ready(Ok(()))
        })
        .await
        .unwrap();
        assert_eq!(
            appended, exports,
            "a retry must not duplicate an accepted export"
        );
        assert!(!hub.with_body("doc", |body| body.has_pending()));
        let mut restarted = DocumentBody::new(2);
        for update in &appended {
            restarted.import(update).unwrap();
        }
        assert_eq!(restarted.text(), text);
    }

    #[tokio::test]
    async fn failed_append_keeps_export_for_retry_and_shutdown_drain() {
        let directory = tempfile::tempdir().unwrap();
        let store = DocumentStore::open(directory.path().join("documents.sqlite3")).unwrap();
        let (events, _) = broadcast::channel(8);
        let hub = BodyHub {
            store,
            bodies: Arc::new(Mutex::new(HashMap::new())),
            events,
            flush: Arc::new(Notify::new()),
            append_locks: Arc::new(Mutex::new(HashMap::new())),
            waiting_metadata: Arc::new(Mutex::new(HashSet::new())),
            metadata_lock: Arc::new(tokio::sync::Mutex::new(())),
            peer: 1,
            qualified: Arc::new(AtomicBool::new(false)),
            identity_index_complete: Arc::new(AtomicBool::new(false)),
            identity_lookup_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        hub.with_body("doc", |body| {
            body.apply(&[TextOp::Insert {
                insert: "saved".into(),
            }])
            .unwrap();
        });
        let first = hub.with_body("doc", |body| {
            body.flush(Instant::now(), true).unwrap().updates[0].clone()
        });
        let failure = drain_document(&hub, "doc", true, |_update| async {
            Err(error("documents.capability_unavailable", true))
        })
        .await;
        assert!(failure.is_err());
        assert!(hub.with_body("doc", |body| body.has_pending()));
        let mut persisted = Vec::new();
        drain_document(&hub, "doc", true, |update| {
            persisted.push(update);
            std::future::ready(Ok(()))
        })
        .await
        .unwrap();
        assert_eq!(persisted, vec![first]);
        assert!(!hub.with_body("doc", |body| body.has_pending()));
        let mut restarted = DocumentBody::new(1);
        restarted.import(&persisted[0]).unwrap();
        assert_eq!(restarted.text(), "saved");
    }

    // Verifies: AC2, AC3 — local edits are echoed, coalesced into one appended record, and a
    // remote edit record is imported and pushed to the open text subscription.
    #[tokio::test]
    async fn edits_are_echoed_appended_and_remote_edits_pushed() {
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
        let opened = session
            .harness
            .request("documents.document.open", json!({ "documentId": document }))
            .await
            .unwrap();
        assert_eq!(opened["text"], "");
        assert_eq!(opened["revision"], "0");
        assert_eq!(opened["editable"], true);
        let mut text = session
            .harness
            .subscribe("documents.document.text", json!({ "documentId": document }))
            .await
            .unwrap();
        let initial = text.next_event().await.unwrap().unwrap();
        assert_eq!(initial["text"], "");

        let edited = session
            .harness
            .request(
                "documents.document.edit",
                json!({ "documentId": document, "requestId":"44444444444444444444444444444444", "baseRevision": "0", "ops": [{ "insert": "# Plan" }] }),
            )
            .await
            .unwrap();
        assert_eq!(edited["revision"], "1");
        let echo = text.next_event().await.unwrap().unwrap();
        assert_eq!(echo["origin"], "local");
        assert_eq!(echo["delta"], json!([{ "insert": "# Plan" }]));
        let stale = session
            .harness
            .request(
                "documents.document.edit",
                json!({ "documentId": document, "requestId":"55555555555555555555555555555555", "baseRevision": "0", "ops": [{ "insert": "x" }] }),
            )
            .await
            .unwrap_err();
        assert_eq!(stale.code(), "documents.stale_revision");
        session
            .harness
            .request(
                "documents.document.edit",
                json!({ "documentId": document, "requestId":"66666666666666666666666666666666", "baseRevision": "1", "ops": [{ "retain": 6 }, { "insert": "\n" }] }),
            )
            .await
            .unwrap();
        let _ = text.next_event().await.unwrap().unwrap();

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
        tokio::time::sleep(COALESCE_WINDOW * 2).await;
        assert_eq!(
            edit_appends(&session.scripted),
            2,
            "no repeated record for an appended edit"
        );
        {
            let bodies = session.handles.bodies.lock().unwrap();
            assert_eq!(bodies[&document].text(), "# Plan\n");
            assert!(!bodies[&document].has_pending());
        }

        // A remote peer's edit arrives as a record and is pushed as a remote delta.
        let mut bob = DocumentBody::new(77);
        bob.apply(&[TextOp::Insert {
            insert: "bob: ".into(),
        }])
        .unwrap();
        let update = bob.flush(Instant::now(), true).unwrap().updates.remove(0);
        let remote = DomainRecord::document_edit("bob", &document, update).unwrap();
        let stream = session.scripted.stream();
        tokio::time::timeout(Duration::from_secs(2), stream.opened())
            .await
            .unwrap();
        assert!(stream.send(record_frame(9, "bob", &remote)).await);
        let pushed = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let event = text.next_event().await.unwrap().unwrap();
                if event["origin"] == "remote" {
                    break event;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(pushed["revision"], "3");
        let reopened = session
            .harness
            .request("documents.document.open", json!({ "documentId": document }))
            .await
            .unwrap();
        let merged = reopened["text"].as_str().unwrap();
        assert!(
            merged.contains("# Plan\n") && merged.contains("bob: "),
            "{merged:?}"
        );
        text.close().await;
        session.harness.shutdown().await.unwrap();
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

    // Verifies: AC6 — the names roster is forwarded in bounded parts and the stream closes
    // with the subscription.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn names_are_forwarded_and_the_stream_closes_with_the_subscription() {
        let directory = tempfile::tempdir().unwrap();
        let names = ScriptedCapability::new().calls(|method, _| match method {
            "current" => Ok(json!({ "workspaceId": "a".repeat(32), "cursor": "0", "persons": [] })),
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
            .capability("teamarness.person-names", names.clone())
            .start()
            .await;
        harness.activate().await.unwrap();
        let mut subscription = harness
            .subscribe("documents.names", json!({}))
            .await
            .unwrap();
        let first = subscription.next_event().await.unwrap().unwrap();
        assert_eq!(first["cursor"], "0");
        assert_eq!(first["persons"], json!([]));
        assert_eq!(first["parts"], 1);
        let stream = names.stream();
        tokio::time::timeout(Duration::from_secs(2), stream.opened())
            .await
            .unwrap();
        subscription.close().await;
        wait_until(|| !stream.is_open(), "idle person-names stream closure").await;
        harness.shutdown().await.unwrap();
    }
}
