use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{Value as JsonValue, json};
use teamarness_package_sdk::capabilities::record_log::{
    Availability, Cursor, OpenResult, RecordEvent, RecordLog, RecordLogSink, SessionOptions,
    SyncState, SyncWarning, run_session,
};
use teamarness_package_sdk::{
    BackendError, CancellationToken, Context, Empty, Router, event_channel,
};
use tokio::sync::watch;

use crate::domain::{DomainRecord, PAGE_SIZE, decode_id, hex_id};
use crate::store::{ConversationStore, IngestResult, MessageRow};

/// How many distinct synchronization warning codes are remembered at a time.
const MAX_WARNINGS: usize = 8;

fn error(code: &str, retryable: bool) -> BackendError {
    BackendError::new(code, retryable, json!({}))
}
fn storage_failed() -> BackendError {
    error("conversations.storage_failed", true)
}

fn capability_error(operation: &str, cause: BackendError) -> BackendError {
    eprintln!(
        "conversations capability failure: operation={operation} code={} retryable={}",
        cause.code(),
        cause.retryable()
    );
    BackendError::new(
        "conversations.capability_unavailable",
        cause.retryable(),
        json!({ "causeCode": cause.code() }),
    )
}
/// What the Package tells the UI about synchronization.
///
/// Availability changes only when the host sends a state event or the session reports a failure;
/// ingesting a record never implies it.
#[derive(Debug, Clone)]
pub(crate) struct SyncReport {
    pub(crate) availability: Availability,
    /// Distinct warning codes the host reported, bounded so a noisy session cannot grow it.
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

/// Everything one activation of Conversations owns.
///
/// Every store call runs inside `Context::blocking`, so SQLite never holds an async worker.
pub struct ConversationsActive {
    store: ConversationStore,
    log: RecordLog,
    sync: Arc<Mutex<SyncReport>>,
    changes: watch::Sender<u64>,
    /// The Installation this Package Instance belongs to, which decides who may hide a Channel.
    installation: String,
    session: Mutex<Option<Session>>,
}

/// What one activation built, for a test that must look inside.
///
/// The shipped router only publishes them; a test is what reads them.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone)]
pub(crate) struct ActivationHandles {
    pub(crate) store: ConversationStore,
    pub(crate) sync: Arc<Mutex<SyncReport>>,
}

pub(crate) type ActivationObserver = Arc<Mutex<Option<ActivationHandles>>>;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateRequest {
    slug: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HideRequest {
    channel_id: String,
    confirm: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PostRequest {
    channel_id: String,
    markdown: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PageRequest {
    channel_id: String,
    #[serde(default)]
    before: Option<String>,
    #[serde(default)]
    limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChangesRequest {
    channel_id: String,
    epoch: String,
    after: String,
    #[serde(default)]
    limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResolveRequest {
    channel_id: String,
    ids: Vec<String>,
}

/// The Conversations Backend.
#[must_use]
pub fn router() -> Router<ConversationsActive> {
    router_observing(None)
}

/// The Backend, publishing what each activation built.
///
/// Only a test passes an observer; the shipped router passes `None`.
pub(crate) fn router_observing(
    observer: Option<ActivationObserver>,
) -> Router<ConversationsActive> {
    Router::new(move |activation| {
        let observer = observer.clone();
        let initialization = activation.initialization().clone();
        let capabilities = activation.capabilities().clone();
        let cancellation = activation.cancellation().clone();
        async move {
            let directory = initialization.data_directory().path().to_owned();
            let store = teamarness_package_sdk::blocking(&cancellation, move |_| {
                ConversationStore::open(directory.join("conversations.sqlite3"))
                    .map_err(|_| storage_failed())
            })
            .await?;
            let log = RecordLog::from_initialization(&initialization, capabilities)
                .map_err(|cause| capability_error("record-log", cause))?;
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
            Ok(ConversationsActive {
                store,
                log,
                sync,
                changes,
                installation: initialization.installation().identity.clone(),
                session: Mutex::new(Some(session)),
            })
        }
    })
    .on_suspend(|state: Arc<ConversationsActive>| async move {
        stop_session(&state).await;
        Ok(())
    })
    .on_resume(|state: Arc<ConversationsActive>| async move {
        // A suspended Instance stopped its session; recovering it starts a new one from the
        // cursor the sink committed.
        stop_session(&state).await;
        let session = start_session(&state.store, &state.log, &state.sync, &state.changes);
        *state
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(session);
        Ok(())
    })
    .on_dispose(|state: Arc<ConversationsActive>| async move {
        stop_session(&state).await;
        Ok(())
    })
    .request(
        "conversations.snapshot",
        |context: Context<ConversationsActive>, _: Empty| async move {
            let state = context.state().clone();
            context.blocking(move |_| state.snapshot_value()).await
        },
    )
    .request(
        "conversations.channel.create",
        |context: Context<ConversationsActive>, request: CreateRequest| async move {
            let author = context.state().installation.clone();
            let record = DomainRecord::channel_create(&author, request.slug)
                .map_err(|_| error("conversations.invalid_slug", false))?;
            append(&context, "channel.create", record).await
        },
    )
    .request(
        "conversations.channel.hide",
        |context: Context<ConversationsActive>, request: HideRequest| async move {
            if !request.confirm {
                return Err(error("conversations.confirmation_required", false));
            }
            let channel = decode_id(&request.channel_id)
                .ok_or_else(|| error("conversations.invalid_request", false))?;
            let author = context.state().installation.clone();
            let record = DomainRecord::channel_hide(channel, &author)
                .map_err(|_| error("conversations.invalid_request", false))?;
            append(&context, "channel.hide", record).await
        },
    )
    .request(
        "conversations.message.post",
        |context: Context<ConversationsActive>, request: PostRequest| async move {
            let channel = decode_id(&request.channel_id)
                .ok_or_else(|| error("conversations.invalid_request", false))?;
            let state = context.state().clone();
            let active = context
                .blocking(move |_| {
                    state
                        .store
                        .channel_is_active(&hex_id(channel))
                        .map_err(|_| storage_failed())
                })
                .await?;
            if !active {
                return Err(error("conversations.channel_hidden", false));
            }
            let author = context.state().installation.clone();
            let record = DomainRecord::message_post(channel, &author, request.markdown)
                .map_err(|_| error("conversations.invalid_markdown", false))?;
            append(&context, "message.post", record).await
        },
    )
    .request(
        "conversations.channel.open",
        |context: Context<ConversationsActive>, request: PageRequest| async move {
            let channel = request.channel_id.clone();
            let page = page(&context, request).await?;
            let state = context.state().clone();
            context
                .blocking(move |_| {
                    state
                        .store
                        .mark_read(&channel)
                        .map_err(|_| storage_failed())
                })
                .await?;
            Ok(page)
        },
    )
    .request(
        "conversations.channel.page",
        |context: Context<ConversationsActive>, request: PageRequest| async move {
            page(&context, request).await
        },
    )
    .request(
        "conversations.channel.changes",
        |context: Context<ConversationsActive>, request: ChangesRequest| async move {
            changes(&context, request).await
        },
    )
    .request(
        "conversations.channel.resolve",
        |context: Context<ConversationsActive>, request: ResolveRequest| async move {
            resolve(&context, request).await
        },
    )
    .subscription(
        "conversations.changes",
        |context: Context<ConversationsActive>, _: Empty| async move {
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

impl ConversationsActive {
    /// The synchronization report the UI is shown.
    pub(crate) fn report(&self) -> SyncReport {
        self.sync
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The Channel list, availability and warnings the UI renders.
    ///
    /// TEAM.SYS.CONV-R06: Hide is offered only to the Installation that created the Channel, and
    /// never for the general Channel. TEAM.SYS.CONV-R13: each Channel carries its unread count.
    fn snapshot_value(&self) -> Result<JsonValue, BackendError> {
        let report = self.report();
        let channels = self.store.channels().map_err(|_| storage_failed())?;
        let visible: Vec<JsonValue> = channels
            .into_iter()
            .filter(|channel| !channel.hidden)
            .map(|channel| {
                let can_hide = channel.creator == self.installation && channel.slug != "general";
                json!({
                    "id": channel.id,
                    "slug": channel.slug,
                    "canHide": can_hide,
                    "unreadCount": channel.unread_count,
                })
            })
            .collect();
        let warnings = if self
            .store
            .has_projection_warnings()
            .map_err(|_| storage_failed())?
        {
            vec![JsonValue::String(
                "Some synchronized records could not be applied.".into(),
            )]
        } else {
            Vec::new()
        };
        Ok(json!({
            "channels": visible,
            "availability": report.availability.as_str(),
            "warnings": warnings,
        }))
    }
}

/// Appends one domain record and projects it locally.
async fn append(
    context: &Context<ConversationsActive>,
    purpose: &'static str,
    record: DomainRecord,
) -> Result<JsonValue, BackendError> {
    let canonical = record.encode();
    let appended = context
        .state()
        .log
        .append(purpose, &canonical)
        .await
        .map_err(|cause| capability_error(purpose, cause))?;
    let state = context.state().clone();
    let outcome = context
        .blocking(move |_| {
            state
                .store
                .ingest(
                    appended.record_id.as_str(),
                    appended.order_key.as_str(),
                    &appended.author,
                    purpose,
                    &appended.signature,
                    &canonical,
                    &appended.sealed_record,
                    None,
                )
                .map_err(|_| storage_failed())
        })
        .await?;
    match outcome {
        IngestResult::Applied | IngestResult::Duplicate => {}
        IngestResult::Deferred(_) => return Err(error("conversations.deferred", true)),
        IngestResult::Quarantined(_) => return Err(error("conversations.rejected", false)),
    }
    context
        .state()
        .changes
        .send_modify(|revision| *revision = revision.saturating_add(1));
    Ok(json!({ "ok": true }))
}

/// Reads one page of a Channel's Messages, opening their sealed records through the capability.
async fn page(
    context: &Context<ConversationsActive>,
    request: PageRequest,
) -> Result<JsonValue, BackendError> {
    if decode_id(&request.channel_id).is_none() {
        return Err(error("conversations.invalid_request", false));
    }
    let limit = request
        .limit
        .map_or(PAGE_SIZE, |limit| (limit as usize).min(PAGE_SIZE));
    let state = context.state().clone();
    let channel = request.channel_id.clone();
    let before = request.before.clone();
    let ((epoch, revision), rows) = context
        .blocking(move |_| {
            state
                .store
                .message_rows_with_head(&channel, before.as_deref(), limit)
                .map_err(|_| storage_failed())
        })
        .await?;
    let full_page = rows.len() == limit;
    let mut messages = Vec::new();
    let mut pending = Vec::new();
    let mut last_order = None;
    let mut shortened = false;
    for row in rows {
        let order = row.1.clone();
        let (ready, waiting) = render_rows(context, vec![row]).await?;
        if !fits_history_reply(&messages, &pending, &ready, &waiting) {
            if last_order.is_none() {
                return Err(error("conversations.response_limit", true));
            }
            shortened = true;
            break;
        }
        messages.extend(ready);
        pending.extend(waiting);
        last_order = Some(order);
    }
    let mut page = json!({ "messages": messages, "pending": pending, "epoch": epoch, "revision": revision.to_string() });
    if (full_page || shortened)
        && let Some(next) = last_order
        && let JsonValue::Object(ref mut fields) = page
    {
        fields.insert("nextBefore".into(), JsonValue::String(next));
    }
    Ok(page)
}

// Leave room for the envelope, cursor, and worst-case JSON escaping in the 256 KiB UI bridge.
const HISTORY_REPLY_BUDGET: usize = 216 * 1024;

fn fits_history_reply(
    messages: &[JsonValue],
    pending: &[JsonValue],
    next_messages: &[JsonValue],
    next_pending: &[JsonValue],
) -> bool {
    let size = messages
        .iter()
        .chain(next_messages)
        .chain(pending)
        .chain(next_pending)
        .map(|item| serde_json::to_vec(item).map_or(usize::MAX, |bytes| bytes.len() + 1))
        .fold(0usize, usize::saturating_add);
    size <= HISTORY_REPLY_BUDGET
}

/// Returns every raw projected row as either ready content or a retryable/permanent slot.
async fn render_rows(
    context: &Context<ConversationsActive>,
    rows: Vec<MessageRow>,
) -> Result<(Vec<JsonValue>, Vec<JsonValue>), BackendError> {
    if rows.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let sealed: Vec<Vec<u8>> = rows.iter().map(|row| row.4.clone()).collect();
    let results = context
        .state()
        .log
        .open(&sealed)
        .await
        .map_err(|cause| capability_error("open-message-page", cause))?;
    if rows.len() != results.len() {
        return Err(error("conversations.invalid_open_result", true));
    }
    let mut messages = Vec::new();
    let mut pending = Vec::new();
    for ((id, position, author, _, _, _), result) in rows.into_iter().zip(results) {
        if let OpenResult::Ready(ref opened) = result
            && let Ok(DomainRecord::MessagePost { markdown, .. }) =
                DomainRecord::decode(&opened.canonical)
        {
            messages.push(json!({ "id": id, "position": position, "author": author,
                "isLocal": author == context.state().installation, "markdown": markdown }));
        } else {
            let status = if matches!(result, OpenResult::WrongScope | OpenResult::Corrupt) {
                "unavailable"
            } else {
                "pending"
            };
            pending.push(json!({ "id": id, "position": position, "status": status }));
        }
    }
    Ok((messages, pending))
}

async fn changes(
    context: &Context<ConversationsActive>,
    request: ChangesRequest,
) -> Result<JsonValue, BackendError> {
    if decode_id(&request.channel_id).is_none() {
        return Err(error("conversations.invalid_request", false));
    }
    let after: u64 = request
        .after
        .parse()
        .map_err(|_| error("conversations.invalid_request", false))?;
    let limit = request
        .limit
        .map_or(PAGE_SIZE, |value| (value as usize).min(PAGE_SIZE));
    let state = context.state().clone();
    let channel = request.channel_id;
    let epoch = request.epoch;
    let batch = context
        .blocking(move |_| {
            state
                .store
                .history_changes(&channel, &epoch, after, limit)
                .map_err(|_| storage_failed())
        })
        .await?;
    let Some(batch) = batch else {
        return Ok(
            json!({ "events": [], "continuation": request.after, "highWater": request.after, "resyncRequired": true }),
        );
    };
    let mut events = Vec::with_capacity(batch.changes.len());
    let mut continuation = batch.continuation;
    for event in batch.changes {
        let event_revision = event.revision;
        let mut response = json!({ "kind": event.kind, "id": event.message_id,
            "position": event.order_key, "revision": event.revision.to_string() });
        if event.kind == "insert" {
            let state = context.state().clone();
            let channel_id = event.channel_id;
            let message_id = event.message_id;
            let row = context
                .blocking(move |_| {
                    state
                        .store
                        .message_row_by_id(&channel_id, &message_id)
                        .map_err(|_| storage_failed())
                })
                .await?;
            if let Some(row) = row {
                let (messages, pending) = render_rows(context, vec![row]).await?;
                response["status"] = json!(if messages.is_empty() {
                    pending[0]["status"].as_str().unwrap_or("pending")
                } else {
                    "ready"
                });
                if let Some(message) = messages.into_iter().next() {
                    response["message"] = message;
                }
            } else {
                response["status"] = json!("unavailable");
            }
        }
        if !fits_history_reply(&events, &[], std::slice::from_ref(&response), &[]) {
            if events.is_empty() {
                return Err(error("conversations.response_limit", true));
            }
            break;
        }
        events.push(response);
        continuation = event_revision;
    }
    Ok(
        json!({ "events": events, "continuation": continuation.to_string(),
        "highWater": batch.high_water.to_string() }),
    )
}

async fn resolve(
    context: &Context<ConversationsActive>,
    request: ResolveRequest,
) -> Result<JsonValue, BackendError> {
    if decode_id(&request.channel_id).is_none() || request.ids.is_empty() || request.ids.len() > 32
    {
        return Err(error("conversations.invalid_request", false));
    }
    let state = context.state().clone();
    let channel = request.channel_id;
    let ids = request.ids;
    let (rows, missing) = context
        .blocking(move |_| {
            let mut rows = Vec::new();
            let mut missing = Vec::new();
            for id in ids {
                match state
                    .store
                    .message_row_by_id(&channel, &id)
                    .map_err(|_| storage_failed())?
                {
                    Some(row) => rows.push(row),
                    None => missing.push(id),
                }
            }
            Ok::<_, BackendError>((rows, missing))
        })
        .await?;
    let mut messages = Vec::new();
    let mut pending = Vec::new();
    let mut truncated = false;
    for row in rows {
        let id = row.0.clone();
        let position = row.1.clone();
        if truncated {
            pending.push(json!({ "id": id, "position": position, "status": "pending" }));
            continue;
        }
        let (ready, waiting) = render_rows(context, vec![row]).await?;
        if !fits_history_reply(&messages, &pending, &ready, &waiting) {
            truncated = true;
            pending.push(json!({ "id": id, "position": position, "status": "pending" }));
        } else {
            messages.extend(ready);
            pending.extend(waiting);
        }
    }
    Ok(json!({ "messages": messages, "pending": pending, "missing": missing }))
}

/// Re-opens the sealed records the projection needs to render, and retries deferred ones.
async fn rehydrate(
    store: &ConversationStore,
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
        for result in results {
            if let OpenResult::Ready(record) = result {
                let _ = store.remember_opened(&record.canonical);
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
    store: &ConversationStore,
    log: &RecordLog,
    sync: &Arc<Mutex<SyncReport>>,
    changes: &watch::Sender<u64>,
) -> Session {
    let sink = Arc::new(ConversationsSink {
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
async fn stop_session(state: &ConversationsActive) {
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

// ---------------------------------------------------------------------------
// record log sink
// ---------------------------------------------------------------------------

/// Where the SDK's record log session commits what it receives.
///
/// Every method runs on the blocking pool, and the session credits a delivery only after `ingest`
/// has returned, so a record whose ingest fails is delivered again rather than skipped.
pub(crate) struct ConversationsSink {
    store: ConversationStore,
    changes: watch::Sender<u64>,
    sync: Arc<Mutex<SyncReport>>,
}

impl ConversationsSink {
    fn with_report(&self, change: impl FnOnce(&mut SyncReport)) {
        let mut report = self
            .sync
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        change(&mut report);
    }
}

impl RecordLogSink for ConversationsSink {
    fn committed_cursor(&self) -> Result<Option<Cursor>, BackendError> {
        let stored = self.store.cursor().map_err(|_| storage_failed())?;
        // A stored value the contract's grammar refuses is treated as no cursor: the session
        // replays from the beginning, which ingestion deduplicates by record identity.
        Ok(stored.and_then(|value| Cursor::parse(value).ok()))
    }

    fn ingest(&self, record: &RecordEvent) -> Result<(), BackendError> {
        self.store
            .ingest(
                record.record_id.as_str(),
                record.order_key.as_str(),
                &record.author,
                &record.purpose,
                &record.signature,
                &record.canonical,
                &record.sealed_record,
                Some(record.cursor.as_str()),
            )
            .map_err(|_| storage_failed())?;
        self.changes.send_modify(|x| *x = x.saturating_add(1));
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
            "conversations session failure: code={} retryable={}",
            error.code(),
            error.retryable()
        );
        if error.retryable() {
            // The session is retrying; the host, not the Package, decides availability.
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
mod session_tests {
    use std::time::Duration;

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use serde_json::json;
    use teamarness_package_sdk::testing::{
        Harness, HarnessBuilder, ScriptedCapability, assert_router_matches_contract,
    };

    use super::*;
    use crate::domain::{GENERAL_ID, hex_id};

    const RECORD_LOG: &str = "teamarness.synchronization.immutable-record-log";
    const CHANNEL: [u8; 16] = [1; 16];
    const ALICE: &str = "alice";

    fn hex(length: usize, seed: u64) -> String {
        format!("{:0width$x}", seed, width = length)
    }

    fn empty_payload() -> JsonValue {
        json!({})
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

    fn record_frame(seed: u64, author: &str, purpose: &str, canonical: &[u8]) -> JsonValue {
        json!({
            "kind": "record",
            "cursor": hex(50, seed),
            "recordId": hex(64, seed),
            "orderKey": hex(82, seed),
            "author": author,
            "purpose": purpose,
            "signature": "c2ln",
            "canonical": URL_SAFE_NO_PAD.encode(canonical),
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
                .package_identity("dev.teamarness.conversations")
                .installation(ALICE, "Alice")
                .data_directory(directory)
                .capability(RECORD_LOG, scripted.clone())
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

    impl Session {
        fn store(&self) -> ConversationStore {
            self.handles.store.clone()
        }

        fn report(&self) -> SyncReport {
            self.handles
                .sync
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
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

    async fn wait_for_opens(scripted: &ScriptedCapability, count: usize) {
        for _ in 0..600 {
            if scripted.recorded_opens().len() >= count {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!(
            "the Package opened {} streams, expected {count}",
            scripted.recorded_opens().len()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn failed_ingest_is_redelivered_and_not_skipped() {
        let directory = tempfile::tempdir().unwrap();
        let session = start(directory.path(), scripted_log()).await;
        let store = session.store();
        wait_for_opens(&session.scripted, 1).await;

        let channel = DomainRecord::channel_create_with_id(CHANNEL, ALICE, "team").unwrap();
        let first = DomainRecord::message_post(CHANNEL, ALICE, "one").unwrap();
        let second = DomainRecord::message_post(CHANNEL, ALICE, "two").unwrap();
        let sender = session.scripted.stream();
        assert!(
            sender
                .send(record_frame(1, ALICE, "channel.create", &channel.encode()))
                .await
        );
        wait_until(
            || store.record_count().unwrap_or(0) == 1,
            "the channel record",
        )
        .await;

        // The next ingest fails once; the session must deliver the same record again rather
        // than crediting it away.
        store.fail_next_ingest_for_test();
        assert!(
            sender
                .send(record_frame(2, ALICE, "message.post", &first.encode()))
                .await
        );
        wait_for_opens(&session.scripted, 2).await;
        let sender = session.scripted.stream();
        assert!(
            sender
                .send(record_frame(2, ALICE, "message.post", &first.encode()))
                .await
        );
        assert!(
            sender
                .send(record_frame(3, ALICE, "message.post", &second.encode()))
                .await
        );
        wait_until(
            || store.record_count().unwrap_or(0) == 3,
            "both messages after the retry",
        )
        .await;

        assert_eq!(
            store
                .message_rows(&hex_id(CHANNEL), None, PAGE_SIZE)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(store.cursor().unwrap(), Some(hex(50, 3)));
        session.harness.shutdown().await.unwrap();
    }

    #[test]
    fn history_response_budget_limits_worst_case_escaped_markdown() {
        let row = json!({ "id": "a".repeat(64), "position": "0".repeat(82),
            "markdown": "\u{0000}".repeat(32 * 1024), "author": "a".repeat(64), "isLocal": false });
        assert!(fits_history_reply(
            &[],
            &[],
            std::slice::from_ref(&row),
            &[]
        ));
        assert!(!fits_history_reply(
            std::slice::from_ref(&row),
            &[],
            std::slice::from_ref(&row),
            &[]
        ));
        assert!(
            serde_json::to_vec(&json!({ "messages": [row] }))
                .unwrap()
                .len()
                < 256 * 1024
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn large_ready_messages_page_with_a_raw_cursor_under_the_ui_limit() {
        let directory = tempfile::tempdir().unwrap();
        let message =
            DomainRecord::message_post(GENERAL_ID, ALICE, "\u{0000}".repeat(32 * 1024)).unwrap();
        let canonical = URL_SAFE_NO_PAD.encode(message.encode());
        let scripted = ScriptedCapability::new().calls(move |method, payload| match method {
            "open" => {
                let count = payload["sealedRecords"].as_array().map_or(0, Vec::len);
                Ok(json!({ "results": (0..count).map(|_| json!({
                    "status": "ready", "recordId": hex(64, 1), "orderKey": hex(82, 1),
                    "author": ALICE, "purpose": "message.post", "signature": "c2ln",
                    "canonical": canonical,
                })).collect::<Vec<_>>() }))
            }
            _ => Err(BackendError::new(
                "capability_unavailable",
                false,
                json!({}),
            )),
        });
        let mut session = start(directory.path(), scripted).await;
        for n in 1..=2 {
            let domain = DomainRecord::message_post(GENERAL_ID, ALICE, "stored").unwrap();
            session
                .store()
                .ingest(
                    &hex(64, n),
                    &hex(82, n),
                    ALICE,
                    domain.kind(),
                    "c2ln",
                    &domain.encode(),
                    b"sealed",
                    None,
                )
                .unwrap();
        }
        let first = session
            .harness
            .request(
                "conversations.channel.open",
                json!({ "channelId": hex_id(GENERAL_ID), "limit": 32 }),
            )
            .await
            .unwrap();
        assert_eq!(first["messages"].as_array().unwrap().len(), 1);
        let cursor = first["nextBefore"].as_str().unwrap();
        let second = session
            .harness
            .request(
                "conversations.channel.page",
                json!({ "channelId": hex_id(GENERAL_ID), "before": cursor }),
            )
            .await
            .unwrap();
        assert_eq!(second["messages"].as_array().unwrap().len(), 1);
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn change_feed_replays_projected_message_with_bounded_continuation() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        let head = session
            .harness
            .request(
                "conversations.channel.open",
                json!({ "channelId": hex_id(GENERAL_ID) }),
            )
            .await
            .unwrap();
        let epoch = head["epoch"].as_str().expect("page has projection epoch");
        let revision = head["revision"]
            .as_str()
            .expect("page has projection revision");
        let message = DomainRecord::message_post(GENERAL_ID, ALICE, "private").unwrap();
        session
            .store()
            .ingest(
                "late",
                "01",
                ALICE,
                message.kind(),
                "s",
                &message.encode(),
                b"sealed",
                None,
            )
            .unwrap();
        let changes = session
            .harness
            .request(
                "conversations.channel.changes",
                json!({
                    "channelId": hex_id(GENERAL_ID), "epoch": epoch, "after": revision, "limit": 1,
                }),
            )
            .await
            .unwrap();
        assert_eq!(changes["events"][0]["id"], hex_id(message.id()));
        assert_eq!(changes["events"][0]["kind"], "insert");
        assert_eq!(changes["events"][0]["status"], "pending");
        assert_eq!(changes["continuation"], "1");
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ingesting_a_record_does_not_mark_the_session_current() {
        let directory = tempfile::tempdir().unwrap();
        let session = start(directory.path(), scripted_log()).await;
        let store = session.store();
        wait_for_opens(&session.scripted, 1).await;

        let channel = DomainRecord::channel_create_with_id(CHANNEL, ALICE, "team").unwrap();
        assert!(
            session
                .scripted
                .stream()
                .send(record_frame(1, ALICE, "channel.create", &channel.encode()))
                .await
        );
        wait_until(
            || store.record_count().unwrap_or(0) == 1,
            "the ingested record",
        )
        .await;

        assert_eq!(
            session.report().availability,
            Availability::CatchingUp,
            "ingesting a record does not mark the session current"
        );
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.CONV-R14
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn state_events_drive_sync_status_for_every_availability() {
        let directory = tempfile::tempdir().unwrap();
        let session = start(directory.path(), scripted_log()).await;
        wait_for_opens(&session.scripted, 1).await;
        let sender = session.scripted.stream();

        for expected in Availability::all() {
            assert!(
                sender
                    .send(json!({
                        "kind": "state",
                        "availability": expected.as_str(),
                        "pendingRecordCount": "0",
                    }))
                    .await
            );
            wait_until(
                || session.report().availability == expected,
                expected.as_str(),
            )
            .await;
        }

        // The retired value is refused by the typed decode rather than mapped to something
        // else: the session reports an invalid capability response and the Package goes offline.
        assert!(
            sender
                .send(json!({
                    "kind": "state",
                    "availability": "degraded",
                    "pendingRecordCount": "0",
                }))
                .await
        );
        wait_until(
            || session.report().availability == Availability::Offline,
            "the refused state event",
        )
        .await;
        assert!(
            session
                .report()
                .warnings
                .contains(&"package.invalid_capability_response".to_owned()),
            "a retired availability is never mapped to a known one"
        );
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn warnings_are_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let session = start(directory.path(), scripted_log()).await;
        wait_for_opens(&session.scripted, 1).await;
        let sender = session.scripted.stream();

        for index in 0..12 {
            assert!(
                sender
                    .send(json!({ "kind": "warning", "code": format!("warning_{index}") }))
                    .await
            );
        }
        wait_until(
            || session.report().warnings.len() == MAX_WARNINGS,
            "the bounded warning list",
        )
        .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(session.report().warnings.len(), MAX_WARNINGS);
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn subscribe_failure_sets_offline_status() {
        let directory = tempfile::tempdir().unwrap();
        let session = start(directory.path(), scripted_log()).await;
        wait_for_opens(&session.scripted, 1).await;

        session
            .scripted
            .stream()
            .close("capability_unavailable")
            .await;
        wait_until(
            || session.report().availability == Availability::Offline,
            "the offline availability",
        )
        .await;
        assert!(
            session
                .report()
                .warnings
                .contains(&"capability_unavailable".to_owned())
        );
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cursor_invalid_close_resets_the_cursor_and_resubscribes_once() {
        let directory = tempfile::tempdir().unwrap();
        // Seed a committed cursor before the Package opens the same database.
        {
            let store =
                ConversationStore::open(directory.path().join("conversations.sqlite3")).unwrap();
            let channel = DomainRecord::channel_create_with_id(CHANNEL, ALICE, "team").unwrap();
            store
                .ingest(
                    &hex(64, 1),
                    &hex(82, 1),
                    ALICE,
                    "channel.create",
                    "c2ln",
                    &channel.encode(),
                    b"sealed",
                    Some(&hex(50, 1)),
                )
                .unwrap();
            assert_eq!(store.cursor().unwrap(), Some(hex(50, 1)));
        }

        let session = start(directory.path(), scripted_log()).await;
        let store = session.store();
        wait_for_opens(&session.scripted, 1).await;
        assert_eq!(
            session.scripted.recorded_opens()[0]["afterCursor"].as_str(),
            Some(hex(50, 1).as_str())
        );

        session.scripted.stream().close("cursor_invalid").await;
        wait_for_opens(&session.scripted, 2).await;

        assert_eq!(store.cursor().unwrap(), None);
        assert_eq!(
            session.scripted.recorded_opens()[1]["afterCursor"],
            JsonValue::Null
        );
        // A replayed record with a known identity does not duplicate.
        let before = store.record_count().unwrap();
        let channel = DomainRecord::channel_create_with_id(CHANNEL, ALICE, "team").unwrap();
        assert!(
            session
                .scripted
                .stream()
                .send(record_frame(1, ALICE, "channel.create", &channel.encode()))
                .await
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(store.record_count().unwrap(), before);
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn suspend_stops_the_session_and_recover_starts_a_new_one() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        wait_for_opens(&session.scripted, 1).await;

        session.harness.suspend().await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(session.scripted.recorded_opens().len(), 1);

        session.harness.recover().await.unwrap();
        wait_for_opens(&session.scripted, 2).await;
        session.harness.dispose().await.unwrap();
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.CONV-R06
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn snapshot_offers_hide_only_for_channels_created_by_this_installation() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        wait_for_opens(&session.scripted, 1).await;

        session
            .harness
            .request("conversations.channel.create", json!({ "slug": "mine" }))
            .await
            .unwrap();
        let remote = DomainRecord::channel_create_with_id([2; 16], "bob", "theirs").unwrap();
        assert!(
            session
                .scripted
                .stream()
                .send(record_frame(7, "bob", "channel.create", &remote.encode()))
                .await
        );
        wait_until(
            || session.store().record_count().unwrap_or(0) == 2,
            "both Channels",
        )
        .await;

        let snapshot = session
            .harness
            .request("conversations.snapshot", empty_payload())
            .await
            .unwrap();
        let mut seen = Vec::new();
        for channel in snapshot["channels"].as_array().unwrap() {
            seen.push((
                channel["slug"].as_str().unwrap().to_owned(),
                channel["canHide"].as_bool().unwrap(),
            ));
        }
        seen.sort();
        assert_eq!(
            seen,
            vec![
                ("general".to_owned(), false),
                ("mine".to_owned(), true),
                ("theirs".to_owned(), false),
            ]
        );
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.CONV-R13
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn snapshot_reports_unread_count_per_channel() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        wait_for_opens(&session.scripted, 1).await;
        let store = session.store();

        let channel = DomainRecord::channel_create_with_id(CHANNEL, "bob", "team").unwrap();
        let sender = session.scripted.stream();
        assert!(
            sender
                .send(record_frame(1, "bob", "channel.create", &channel.encode()))
                .await
        );
        for (index, text) in ["one", "two", "three"].into_iter().enumerate() {
            let message = DomainRecord::message_post(CHANNEL, "bob", text).unwrap();
            assert!(
                sender
                    .send(record_frame(
                        10 + index as u64,
                        "bob",
                        "message.post",
                        &message.encode()
                    ))
                    .await
            );
        }
        wait_until(|| store.record_count().unwrap_or(0) == 4, "every record").await;

        let snapshot = session
            .harness
            .request("conversations.snapshot", empty_payload())
            .await
            .unwrap();
        let counts = unread_counts(&snapshot);
        assert_eq!(counts.get("team"), Some(&3));
        assert_eq!(counts.get("general"), Some(&0));

        session
            .harness
            .request(
                "conversations.channel.open",
                json!({ "channelId": hex_id(CHANNEL) }),
            )
            .await
            .unwrap();
        let snapshot = session
            .harness
            .request("conversations.snapshot", empty_payload())
            .await
            .unwrap();
        assert_eq!(unread_counts(&snapshot).get("team"), Some(&0));
        session.harness.shutdown().await.unwrap();
    }

    fn unread_counts(snapshot: &JsonValue) -> std::collections::BTreeMap<String, u64> {
        snapshot["channels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|channel| {
                (
                    channel["slug"].as_str().unwrap().to_owned(),
                    channel["unreadCount"].as_u64().unwrap(),
                )
            })
            .collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn invalid_payload_uses_the_sdk_code() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        let error = session
            .harness
            .request(
                "conversations.channel.create",
                json!({ "slug": "mine", "extra": 1 }),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code(), "package.invalid_payload");

        let unknown = session
            .harness
            .request("conversations.delete", empty_payload())
            .await
            .unwrap_err();
        assert_eq!(unknown.code(), "package.unknown_operation");
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn general_channel_is_visible_before_any_record() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        let snapshot = session
            .harness
            .request("conversations.snapshot", empty_payload())
            .await
            .unwrap();
        assert_eq!(snapshot["availability"], "catching_up");
        let channels = snapshot["channels"].as_array().unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0]["id"], hex_id(GENERAL_ID));
        assert_eq!(channels[0]["canHide"], false);
        session.harness.shutdown().await.unwrap();
    }

    #[test]
    fn router_matches_the_ui_contract() {
        assert_router_matches_contract(
            &router().operations(),
            include_str!("../../ui-contract.json"),
        );
    }
}
