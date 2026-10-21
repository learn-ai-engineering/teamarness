use std::sync::{Arc, Mutex};

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
use tokio::sync::watch;

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
        _ => invalid_request(),
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
    #[allow(dead_code)]
    names: PersonNames,
    sync: Arc<Mutex<SyncReport>>,
    changes: watch::Sender<u64>,
    installation: String,
    session: Mutex<Option<Session>>,
}

/// What one activation built, for a test that must look inside.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone)]
pub(crate) struct ActivationHandles {
    pub(crate) store: DocumentStore,
    pub(crate) sync: Arc<Mutex<SyncReport>>,
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
            rehydrate(&store, &log, &cancellation).await?;

            let sync = Arc::new(Mutex::new(SyncReport::catching_up()));
            let (changes, _) = watch::channel(0);
            if let Some(observer) = observer {
                *observer
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ActivationHandles {
                    store: store.clone(),
                    sync: sync.clone(),
                });
            }
            let session = start_session(&store, &log, &sync, &changes);
            Ok(DocumentsActive {
                store,
                log,
                names,
                sync,
                changes,
                installation: initialization.installation().identity.clone(),
                session: Mutex::new(Some(session)),
            })
        }
    })
    .on_suspend(|state: Arc<DocumentsActive>| async move {
        stop_session(&state).await;
        Ok(())
    })
    .on_resume(|state: Arc<DocumentsActive>| async move {
        stop_session(&state).await;
        let session = start_session(&state.store, &state.log, &state.sync, &state.changes);
        *state
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(session);
        Ok(())
    })
    .on_dispose(|state: Arc<DocumentsActive>| async move {
        stop_session(&state).await;
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

impl DocumentsActive {
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
    let outcome = context
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
    match outcome {
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
            if let OpenResult::Ready(record) = result {
                let _ = store.retry_deferred(&row.record_id, &record.canonical);
            }
        }
    }
    Ok(())
}

/// Starts the record log session that ingests everything the host delivers.
fn start_session(
    store: &DocumentStore,
    log: &RecordLog,
    sync: &Arc<Mutex<SyncReport>>,
    changes: &watch::Sender<u64>,
) -> Session {
    let sink = Arc::new(DocumentsSink {
        store: store.clone(),
        changes: changes.clone(),
        sync: sync.clone(),
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
        self.store
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
}
