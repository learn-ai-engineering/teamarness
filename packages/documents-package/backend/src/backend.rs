use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value as JsonValue, json};
use teamarness_package_sdk::capabilities::person_names::{PersonNameSnapshot, PersonNames};
use teamarness_package_sdk::capabilities::record_log::{
    Availability, Cursor, OpenResult, RecordEvent, RecordLog, RecordLogSink, SessionOptions,
    SyncState, SyncWarning, run_session,
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
    /// This Installation's Loro peer id, stable across restarts and distinct across Installations.
    peer: u64,
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
            let published = self.with_body(document, |body| match body.import(update) {
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
        self.with_body(document, |body| {
            full_text_events(document, body.revision(), body.text(), body.editable())
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
    let base =
        json!({ "documentId": document, "revision": revision.to_string(), "editable": editable });
    let mut small = base.clone();
    small["text"] = json!(text);
    if small.to_string().len() <= TEXT_EVENT_BUDGET {
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
    base_revision: String,
    ops: Vec<TextOp>,
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
                peer: peer_id(&installation),
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
            let (revision, delta, editable, is_retry) = hub.with_body(&document, |body| {
                if !body.editable() {
                    return Err(error("documents.document_pending", true));
                }
                if let Some(revision) = body.retried_revision(base, &request.ops) {
                    return Ok((revision, Vec::new(), body.editable(), true));
                }
                if body.revision() != base {
                    return Err(BackendError::new(
                        "documents.stale_revision",
                        false,
                        json!({ "revision": body.revision().to_string() }),
                    ));
                }
                match body.apply(&request.ops) {
                    Ok(delta) => {
                        body.remember_request(base, &request.ops);
                        Ok((body.revision(), delta, body.editable(), false))
                    }
                    Err(BodyError::TooLarge) => Err(error("documents.document_too_large", false)),
                    Err(BodyError::OutOfRange) => Err(invalid_request()),
                    Err(BodyError::Loro) => Err(error("documents.edit_failed", true)),
                }
            })?;
            if !is_retry {
                hub.flush.notify_one();
                let event = json!({
                    "documentId": document, "revision": revision.to_string(),
                    "origin": "local", "delta": delta, "editable": editable,
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
            Ok(json!({ "ok": true, "revision": revision.to_string() }))
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
        for result in results {
            match result {
                OpenResult::Ready(record) => {
                    if let Ok(DomainRecord::DocumentEdit {
                        document, update, ..
                    }) = DomainRecord::decode(&record.canonical)
                    {
                        hub.apply_edits(&[(document, update)]);
                    }
                }
                OpenResult::WaitingForHistoryAccess => unopened += 1,
                OpenResult::WrongScope | OpenResult::Corrupt => {}
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
        IngestResult::Quarantined(_) => return Err(error("documents.rejected", false)),
    }
    Ok(json!({ "ok": true, "id": id }))
}

/// Re-opens the sealed metadata records the listing needs, and retries deferred ones.
async fn rehydrate(
    store: &DocumentStore,
    log: &RecordLog,
    hub: &BodyHub,
    cancellation: &CancellationToken,
) -> Result<(), BackendError> {
    let reader = store.clone();
    let control = teamarness_package_sdk::blocking(cancellation, move |_| {
        reader.control_records().map_err(|_| storage_failed())
    })
    .await?;
    for batch in open_batches(&control) {
        let sealed: Vec<Vec<u8>> = batch.iter().map(|row| row.sealed_record.clone()).collect();
        let results = log
            .open(&sealed)
            .await
            .map_err(|cause| capability_error("open-control-records", cause))?;
        for (row, result) in batch.iter().zip(results) {
            if let OpenResult::Ready(record) = result {
                let _ = store.remember_opened(&row.record_id, &row.order_key, &record.canonical);
            }
        }
    }
    let reader = store.clone();
    let deferred = teamarness_package_sdk::blocking(cancellation, move |_| {
        reader.deferred_records().map_err(|_| storage_failed())
    })
    .await?;
    for batch in open_batches(&deferred) {
        let sealed: Vec<Vec<u8>> = batch.iter().map(|row| row.sealed_record.clone()).collect();
        let results = log
            .open(&sealed)
            .await
            .map_err(|cause| capability_error("open-deferred-records", cause))?;
        for (row, result) in batch.iter().zip(results.iter()) {
            if let OpenResult::Ready(record) = result
                && let Ok(ingested) = store.retry_deferred(&row.record_id, &record.canonical)
            {
                hub.apply_edits(&ingested.edits);
            }
        }
    }
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
        let unopened = import_edit_rows(log, hub, &rows).await?;
        hub.with_body(&document, |body| body.set_unopened(unopened));
    }
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
                .filter(|(_, body)| body.has_pending())
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
    loop {
        let next = hub.with_body(document, |body| {
            body.flush(Instant::now(), force)
                .and_then(|flushed| flushed.updates.into_iter().next())
        });
        let Some(update) = next else {
            return Ok(());
        };
        append(update).await?;
        hub.with_body(document, DocumentBody::append_succeeded);
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
    let record = DomainRecord::document_edit(author, document, update).map_err(domain_error)?;
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
            let others: Vec<(String, Vec<u8>)> = ingested
                .edits
                .into_iter()
                .filter(|(target, _)| target != document)
                .collect();
            hub.apply_edits(&others);
        }
        Ok(Err(cause)) => eprintln!(
            "documents edit projection failure after append: {}",
            cause.code()
        ),
        Err(cause) => eprintln!("documents edit projection task failed after append: {cause}"),
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
    });
    let cancellation = CancellationToken::new();
    let task = tokio::spawn(run_session(
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
        self.hub.apply_edits(&ingested.edits);
        Ok(())
    }

    fn reset_cursor(&self) -> Result<(), BackendError> {
        self.store.reset_cursor().map_err(|_| storage_failed())
    }

    fn on_state(&self, state: &SyncState) {
        self.with_report(|report| report.availability = state.availability);
        // History access can arrive while availability remains `current`; every session state
        // observation gives pending editors another opportunity to open sealed edits.
        self.changes
            .send_modify(|revision| *revision = revision.saturating_add(1));
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
            peer: 3,
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
            let sealed = vec![target; 32 * 1024];
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
                    if bytes[0] == 5 && gained.load(Ordering::SeqCst) {
                        json!({"status":"ready", "recordId":hex(64, 37), "orderKey":hex(82, 37),
                            "author":"bob", "purpose":"document.edit", "signature":"c2ln", "canonical":canonical})
                    } else { json!({"status":"waiting_for_history_access"}) }
                }).collect::<Vec<_>>() }))
            }
            _ => Err(error("unavailable", false)),
        });
        let mut session = start(directory.path(), scripted.clone()).await;
        assert_eq!(
            session.handles.bodies.lock().unwrap()[&document].unopened(),
            17
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
        let request =
            json!({"documentId": document, "baseRevision":"0", "ops":[{"insert":"draft"}]});
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
        let second = json!({"documentId": document, "baseRevision":"1", "ops":[{"retain":5},{"insert":"!"}]});
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
                json!({"documentId": document, "baseRevision":"0", "ops":[{"insert":"durable"}]}),
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
        let restarted = start(directory.path(), scripted).await;
        assert_eq!(
            restarted.handles.bodies.lock().unwrap()[&document].text(),
            "durable"
        );
        assert!(!restarted.handles.bodies.lock().unwrap()[&document].has_pending());
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
            peer: 1,
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
            peer: 1,
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
                json!({ "documentId": document, "baseRevision": "0", "ops": [{ "insert": "# Plan" }] }),
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
                json!({ "documentId": document, "baseRevision": "0", "ops": [{ "insert": "x" }] }),
            )
            .await
            .unwrap_err();
        assert_eq!(stale.code(), "documents.stale_revision");
        session
            .harness
            .request(
                "documents.document.edit",
                json!({ "documentId": document, "baseRevision": "1", "ops": [{ "retain": 6 }, { "insert": "\n" }] }),
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
        let pushed = tokio::time::timeout(Duration::from_secs(2), text.next_event())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(pushed["origin"], "remote");
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
