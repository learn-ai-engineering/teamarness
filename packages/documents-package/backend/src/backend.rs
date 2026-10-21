use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value as JsonValue, json};
use teamarness_package_sdk::capabilities::person_names::PersonNames;
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
use crate::store::{DocumentStore, IngestResult, Ingested};

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
                let _ = self.events.send(event);
            }
        }
    }

    fn text_event(&self, document: &str) -> JsonValue {
        self.with_body(document, |body| {
            json!({
                "documentId": document, "revision": body.revision().to_string(),
                "text": body.text(), "editable": body.editable(),
            })
        })
    }
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
    #[allow(dead_code)]
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
            Ok(context.state().hub.text_event(&request.document_id))
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
            let (revision, delta, editable) = hub.with_body(&document, |body| {
                if !body.editable() {
                    return Err(error("documents.document_pending", true));
                }
                if body.revision() != base {
                    return Err(BackendError::new(
                        "documents.stale_revision",
                        false,
                        json!({ "revision": body.revision().to_string() }),
                    ));
                }
                match body.apply(&request.ops) {
                    Ok(delta) => Ok((body.revision(), delta, body.editable())),
                    Err(BodyError::TooLarge) => Err(error("documents.document_too_large", false)),
                    Err(BodyError::OutOfRange) => Err(invalid_request()),
                    Err(BodyError::Loro) => Err(error("documents.edit_failed", true)),
                }
            })?;
            let _ = hub.events.send(json!({
                "documentId": document, "revision": revision.to_string(),
                "origin": "local", "delta": delta, "editable": editable,
            }));
            hub.flush.notify_one();
            Ok(json!({ "ok": true, "revision": revision.to_string() }))
        },
    )
    .request(
        "documents.comments",
        |context: Context<DocumentsActive>, request: DocumentRequest| async move {
            require_document(&context, &request.document_id).await?;
            let state = context.state().clone();
            let document = request.document_id;
            context
                .blocking(move |_| state.comments_value(&document))
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
            let record = DomainRecord::comment_add(
                &author,
                &request.document_id,
                request.parent_id.as_deref(),
                request.quote.as_deref(),
                &request.markdown,
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
                let initial = hub.text_event(&document);
                let mut revision = revision_of(&initial);
                if sender.send(initial).await.is_err() {
                    return;
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
                            if next <= revision {
                                continue;
                            }
                            revision = next;
                            if sender.send(event).await.is_err() {
                                return;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            // Resynchronize with the full text rather than guess what was missed.
                            let full = hub.text_event(&document);
                            revision = revision_of(&full);
                            if sender.send(full).await.is_err() {
                                return;
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
    hub.with_body(document, |body| body.set_unopened(unopened));
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
    for batch in rows.chunks(32) {
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
    /// The Comments of one Document, bounded to the newest 256 so the reply fits the UI bridge.
    fn comments_value(&self, document: &str) -> Result<JsonValue, BackendError> {
        const MAX_COMMENTS: usize = 256;
        let all = self
            .store
            .comments(document)
            .map_err(|_| storage_failed())?;
        let truncated = all.len() > MAX_COMMENTS;
        let start = all.len().saturating_sub(MAX_COMMENTS);
        let comments: Vec<JsonValue> = all[start..]
            .iter()
            .map(|comment| {
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
                value
            })
            .collect();
        let mut value = json!({ "comments": comments });
        if truncated {
            value["truncated"] = json!(true);
        }
        Ok(value)
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
    for batch in control.chunks(32) {
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
    for batch in deferred.chunks(32) {
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
                let flushed = hub.with_body(&document, |body| body.flush(Instant::now(), false));
                let Some(flushed) = flushed else {
                    continue;
                };
                for update in flushed.updates {
                    if let Err(cause) =
                        append_edit(&log, &hub, &changes, &author, &document, update).await
                    {
                        eprintln!(
                            "documents edit append failure: document={document} code={} retryable={}",
                            cause.code(),
                            cause.retryable()
                        );
                    }
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
    let ingested: Ingested = tokio::task::spawn_blocking(move || {
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
    .await
    .map_err(|_| error("documents.storage_failed", true))??;
    // The local body already holds this update; only edits a deferred create unblocked matter.
    let others: Vec<(String, Vec<u8>)> = ingested
        .edits
        .into_iter()
        .filter(|(target, _)| target != document)
        .collect();
    hub.apply_edits(&others);
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

        // Both edits coalesce into one appended `document.edit` record after the window.
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
        wait_until(|| edit_appends(&scripted) == 1, "one coalesced edit append").await;
        tokio::time::sleep(COALESCE_WINDOW * 2).await;
        assert_eq!(
            edit_appends(&session.scripted),
            1,
            "no second record for an appended edit"
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
        assert_eq!(comments[0]["quote"], "# Q4");
        assert_eq!(comments[0]["markdown"], "Needs dates");
        assert_eq!(comments[0]["resolved"], true);
        assert_eq!(comments[0]["author"], ALICE);
        assert_eq!(comments[0]["isLocal"], true);
        assert_eq!(comments[1]["parentId"], root);
        assert_eq!(comments[1]["resolved"], false);
        session.harness.shutdown().await.unwrap();
    }
}
