use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Map, Value as JsonValue};
use teamarness_package_sdk::{
    Backend, BackendError, BackendRequest, CapabilityClient, EventSubscription, Package,
    event_channel,
};
use tokio::sync::{Mutex as AsyncMutex, watch};

use crate::domain::{DomainRecord, PAGE_SIZE, decode_id, hex_id};
use crate::immutable_log::{
    CapabilityLogAdapter, ImmutableRecordLog, LogError, LogEvent, LogSubscription,
    OpenResultStatus, SyncStatus,
};
use crate::store::{ConversationStore, IngestResult};

fn object<const N: usize>(entries: [(&str, JsonValue); N]) -> JsonValue {
    let mut o = Map::new();
    for (k, v) in entries {
        o.insert(k.into(), v);
    }
    JsonValue::Object(o)
}
fn str_field(v: &JsonValue, k: &str) -> Result<String, BackendError> {
    v.get(k)
        .and_then(JsonValue::as_str)
        .map(str::to_owned)
        .ok_or_else(|| error("conversations.invalid_request", false))
}
fn error(code: &str, retryable: bool) -> BackendError {
    BackendError::new(code, retryable, object([]))
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
        object([("causeCode", JsonValue::String(cause.code().to_owned()))]),
    )
}
fn log_error(operation: &str, cause: LogError) -> BackendError {
    let (code, retryable) = match &cause {
        LogError::Transport(c) => (c.clone(), true),
        LogError::InvalidResponse => ("conversations.invalid_record".into(), false),
        LogError::StreamClosed => ("conversations.stream_closed".into(), true),
    };
    eprintln!("conversations log failure: operation={operation} code={code} retryable={retryable}");
    BackendError::new(
        "conversations.capability_unavailable",
        retryable,
        object([("causeCode", JsonValue::String(code))]),
    )
}
fn grant(init: &teamarness_package_sdk::Initialization, id: &str) -> Result<String, BackendError> {
    init.capabilities()
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|v| v.get("identity").and_then(JsonValue::as_str) == Some(id))
                .and_then(|v| v.get("grant"))
                .and_then(JsonValue::as_str)
                .map(str::to_owned)
        })
        .ok_or_else(|| error("conversations.missing_capability", false))
}
fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}
fn from_b64(v: &str) -> Result<Vec<u8>, BackendError> {
    URL_SAFE_NO_PAD
        .decode(v)
        .map_err(|_| error("conversations.invalid_record", false))
}

#[derive(Clone)]
pub struct ConversationsBackend {
    pub(crate) active: Arc<AsyncMutex<Option<Running>>>,
    lifecycle: Arc<AsyncMutex<()>>,
    changes: watch::Sender<u64>,
}
#[derive(Clone)]
pub(crate) struct Active {
    pub(crate) store: ConversationStore,
    pub(crate) record_grant: String,
    pub(crate) sync_status: Arc<AsyncMutex<SyncStatus>>,
}
pub(crate) struct Running {
    pub(crate) active: Active,
    pub(crate) cancel: watch::Sender<bool>,
    pub(crate) task: tokio::task::JoinHandle<()>,
}
impl Default for ConversationsBackend {
    fn default() -> Self {
        let (tx, _) = watch::channel(0);
        Self {
            active: Arc::new(AsyncMutex::new(None)),
            lifecycle: Arc::new(AsyncMutex::new(())),
            changes: tx,
        }
    }
}
impl ConversationsBackend {
    pub async fn serve() -> Result<(), teamarness_package_sdk::SdkError> {
        Package::builder(Arc::new(Self::default())).serve().await
    }

    async fn active(&self) -> Result<Active, BackendError> {
        self.active
            .lock()
            .await
            .as_ref()
            .map(|running| running.active.clone())
            .ok_or_else(|| error("conversations.not_active", true))
    }

    async fn stop_active(&self) {
        if let Some(running) = self.active.lock().await.take() {
            let _ = running.cancel.send(true);
            let _ = running.task.await;
        }
    }

    pub(crate) async fn deactivate(&self) -> Result<JsonValue, BackendError> {
        let _lifecycle = self.lifecycle.lock().await;
        self.stop_active().await;
        Ok(object([]))
    }

    async fn activate(&self, r: &BackendRequest) -> Result<JsonValue, BackendError> {
        let _lifecycle = self.lifecycle.lock().await;
        self.stop_active().await;
        let data = r
            .initialization
            .data_directory()
            .get("path")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| error("conversations.invalid_initialization", false))?;
        let store = ConversationStore::open(Path::new(data).join("conversations.sqlite3"))
            .map_err(|_| error("conversations.storage_failed", true))?;
        let sync = grant(
            &r.initialization,
            "teamarness.synchronization.immutable-record-log",
        )?;
        let log = CapabilityLogAdapter::new(r.capabilities.clone(), sync.clone());

        // Open control records (channel creates / hides) to rehydrate display data.
        let control_records = store
            .control_records()
            .map_err(|_| error("conversations.storage_failed", true))?;
        for batch in control_records.chunks(32) {
            let sealed: Vec<Vec<u8>> = batch.iter().map(|row| row.sealed_record.clone()).collect();
            let results = log
                .open(sealed)
                .await
                .map_err(|cause| log_error("open-control-records", cause))?;
            for result in results {
                if let OpenResultStatus::Ready { canonical } = &result.status {
                    let _ = store.remember_opened(canonical);
                }
            }
        }

        // Open and retry deferred records (e.g. messages before their channel).
        let deferred = store
            .deferred_records()
            .map_err(|_| error("conversations.storage_failed", true))?;
        for batch in deferred.chunks(32) {
            let records: Vec<_> = batch.to_vec();
            let sealed: Vec<Vec<u8>> = records
                .iter()
                .map(|row| row.sealed_record.clone())
                .collect();
            let results = log
                .open(sealed)
                .await
                .map_err(|cause| log_error("open-deferred-records", cause))?;
            for (row, result) in records.iter().zip(results.iter()) {
                if let OpenResultStatus::Ready { canonical } = &result.status {
                    let _ = store.retry_deferred(&row.record_id, canonical);
                }
            }
        }

        let sync_status = Arc::new(AsyncMutex::new(SyncStatus::CatchingUp));
        let active = Active {
            store: store.clone(),
            record_grant: sync.clone(),
            sync_status: sync_status.clone(),
        };

        let backend = self.clone();
        let capabilities = r.capabilities.clone();
        let (cancel, cancelled) = watch::channel(false);
        let active_clone = active.clone();
        let task = tokio::spawn(async move {
            run_session(backend, active_clone, capabilities, sync, cancelled).await;
        });
        *self.active.lock().await = Some(Running {
            active,
            cancel,
            task,
        });
        Ok(object([]))
    }

    async fn snapshot(&self) -> Result<JsonValue, BackendError> {
        let a = self.active().await?;
        let availability = a.sync_status.lock().await.availability().to_owned();
        let channels = a
            .store
            .channels()
            .map_err(|_| error("conversations.storage_failed", true))?;
        let visible = channels
            .into_iter()
            .filter(|c| !c.hidden)
            .map(|c| {
                object([
                    ("id", JsonValue::String(c.id)),
                    ("slug", JsonValue::String(c.slug)),
                    ("canHide", JsonValue::Bool(c.creator != "system")),
                ])
            })
            .collect();
        let warnings = if a
            .store
            .has_projection_warnings()
            .map_err(|_| error("conversations.storage_failed", true))?
        {
            vec![JsonValue::String(
                "Some synchronized records could not be applied.".into(),
            )]
        } else {
            Vec::new()
        };
        Ok(object([
            ("channels", JsonValue::Array(visible)),
            ("availability", JsonValue::String(availability)),
            ("warnings", JsonValue::Array(warnings)),
        ]))
    }

    async fn append(
        &self,
        r: &BackendRequest,
        purpose: &str,
        domain: DomainRecord,
    ) -> Result<JsonValue, BackendError> {
        let a = self.active().await?;
        let canonical = domain.encode();
        let response = r
            .capabilities
            .call(
                &a.record_grant,
                "append",
                object([
                    ("purpose", JsonValue::String(purpose.into())),
                    ("canonical", JsonValue::String(b64(&canonical))),
                ]),
            )
            .await
            .map_err(|cause| capability_error(purpose, cause))?;
        let record_id = str_field(&response, "recordId")?;
        let order = str_field(&response, "orderKey")?;
        let author = str_field(&response, "author")?;
        let signature = str_field(&response, "signature")?;
        let sealed = from_b64(&str_field(&response, "sealedRecord")?)?;
        let outcome = a
            .store
            .ingest(
                &record_id, &order, &author, purpose, &signature, &canonical, &sealed, None,
            )
            .map_err(|_| error("conversations.storage_failed", true))?;
        match outcome {
            IngestResult::Applied | IngestResult::Duplicate => {}
            IngestResult::Deferred(_) => {
                return Err(error("conversations.deferred", true));
            }
            IngestResult::Quarantined(_) => {
                return Err(error("conversations.rejected", false));
            }
        }
        self.changes.send_modify(|x| *x = x.saturating_add(1));
        Ok(object([("ok", JsonValue::Bool(true))]))
    }

    async fn page(
        &self,
        r: &BackendRequest,
        channel: &str,
        before: Option<&str>,
        limit: usize,
    ) -> Result<JsonValue, BackendError> {
        let a = self.active().await?;
        if decode_id(channel).is_none() {
            return Err(error("conversations.invalid_request", false));
        }
        let page_limit = limit.min(PAGE_SIZE);
        let rows = a
            .store
            .message_rows(channel, before, page_limit)
            .map_err(|_| error("conversations.storage_failed", true))?;
        if rows.is_empty() {
            return Ok(object([("messages", JsonValue::Array(Vec::new()))]));
        }
        let has_more = rows.len() == page_limit;
        let sealed = rows.iter().map(|x| JsonValue::String(b64(&x.4))).collect();
        let opened = r
            .capabilities
            .call(
                &a.record_grant,
                "open",
                object([("sealedRecords", JsonValue::Array(sealed))]),
            )
            .await
            .map_err(|cause| capability_error("open-message-page", cause))?;
        let results = opened
            .get("results")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| error("conversations.invalid_record", false))?;
        let local_identity = r
            .initialization
            .installation()
            .get("identity")
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        let last_order = rows.last().map(|r| r.1.clone());
        let mut messages = Vec::new();
        for ((id, _order, author, _rid, _sealed, _ordinal), value) in rows.into_iter().zip(results)
        {
            let canon = value
                .get("canonical")
                .and_then(JsonValue::as_str)
                .and_then(|s| from_b64(s).ok())
                .and_then(|b| DomainRecord::decode(&b).ok());
            if let Some(DomainRecord::MessagePost { markdown, .. }) = canon {
                messages.push(object([
                    ("id", JsonValue::String(id)),
                    ("author", JsonValue::String(author.clone())),
                    ("isLocal", JsonValue::Bool(author == local_identity)),
                    ("markdown", JsonValue::String(markdown)),
                ]));
            }
        }
        let mut page = object([("messages", JsonValue::Array(messages))]);
        if has_more
            && let Some(next) = last_order
            && let JsonValue::Object(ref mut o) = page
        {
            o.insert("nextBefore".into(), JsonValue::String(next));
        }
        Ok(page)
    }
}
impl Backend for ConversationsBackend {
    fn handle(
        &self,
        r: BackendRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<JsonValue, BackendError>> + Send + '_>>
    {
        Box::pin(async move {
            match r.operation.as_str() {
                "lifecycle.activate" | "lifecycle.recover" => self.activate(&r).await,
                "lifecycle.suspend" | "lifecycle.dispose" => self.deactivate().await,
                "conversations.snapshot" => self.snapshot().await,
                "conversations.channel.create" => {
                    let slug = str_field(&r.payload, "slug")?;
                    let author = r
                        .initialization
                        .installation()
                        .get("identity")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("unknown");
                    let d = DomainRecord::channel_create(author, slug)
                        .map_err(|_| error("conversations.invalid_slug", false))?;
                    self.append(&r, "channel.create", d).await
                }
                "conversations.channel.hide" => {
                    if r.payload.get("confirm").and_then(JsonValue::as_bool) != Some(true) {
                        return Err(error("conversations.confirmation_required", false));
                    }
                    let cid = decode_id(&str_field(&r.payload, "channelId")?)
                        .ok_or_else(|| error("conversations.invalid_request", false))?;
                    let author = r
                        .initialization
                        .installation()
                        .get("identity")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("unknown");
                    let d = DomainRecord::channel_hide(cid, author)
                        .map_err(|_| error("conversations.invalid_request", false))?;
                    self.append(&r, "channel.hide", d).await
                }
                "conversations.message.post" => {
                    let cid = decode_id(&str_field(&r.payload, "channelId")?)
                        .ok_or_else(|| error("conversations.invalid_request", false))?;
                    let active = self.active().await?;
                    if !active
                        .store
                        .channel_is_active(&hex_id(cid))
                        .map_err(|_| error("conversations.storage_failed", true))?
                    {
                        return Err(error("conversations.channel_hidden", false));
                    }
                    let md = str_field(&r.payload, "markdown")?;
                    let author = r
                        .initialization
                        .installation()
                        .get("identity")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("unknown");
                    let d = DomainRecord::message_post(cid, author, md)
                        .map_err(|_| error("conversations.invalid_markdown", false))?;
                    self.append(&r, "message.post", d).await
                }
                "conversations.channel.open" | "conversations.channel.page" => {
                    let cid = str_field(&r.payload, "channelId")?;
                    let before = r.payload.get("before").and_then(JsonValue::as_str);
                    let limit = r
                        .payload
                        .get("limit")
                        .and_then(JsonValue::as_u64)
                        .unwrap_or(PAGE_SIZE as u64) as usize;
                    let out = self.page(&r, &cid, before, limit).await?;
                    if r.operation.ends_with("open") {
                        self.active()
                            .await?
                            .store
                            .mark_read(&cid)
                            .map_err(|_| error("conversations.storage_failed", true))?;
                    }
                    Ok(out)
                }
                _ => Err(error("conversations.unknown_operation", false)),
            }
        })
    }
    fn subscribe(
        &self,
        r: BackendRequest,
    ) -> Pin<
        Box<dyn std::future::Future<Output = Result<EventSubscription, BackendError>> + Send + '_>,
    > {
        Box::pin(async move {
            if r.operation != "conversations.changes" {
                return Err(error("conversations.unknown_subscription", false));
            }
            let (tx, sub) = event_channel(32);
            let me = self.clone();
            let mut rev = self.changes.subscribe();
            tokio::spawn(async move {
                if let Ok(v) = me.snapshot().await
                    && tx.send(v).await.is_err()
                {
                    return;
                }
                while rev.changed().await.is_ok() {
                    if let Ok(v) = me.snapshot().await
                        && tx.send(v).await.is_err()
                    {
                        return;
                    }
                }
            });
            Ok(sub)
        })
    }
}

// ---------------------------------------------------------------------------
// session loop
// ---------------------------------------------------------------------------

const INITIAL_BACKOFF: Duration = Duration::from_millis(100);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Run a cancellation-aware session loop that subscribes from the committed
/// cursor, ingests events, and reconnects with exponential backoff on failure.
async fn run_session(
    backend: ConversationsBackend,
    active: Active,
    capabilities: CapabilityClient,
    grant: String,
    mut cancelled: watch::Receiver<bool>,
) {
    let log = CapabilityLogAdapter::new(capabilities, grant);
    let mut backoff = INITIAL_BACKOFF;

    // Start in catching-up; the first healthy record transitions to current.
    *active.sync_status.lock().await = SyncStatus::CatchingUp;

    'outer: loop {
        // Check cancellation before expensive subscribe.
        if *cancelled.borrow() {
            break;
        }

        let cursor = match active.store.cursor() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("conversations cursor read failure: {e:?}");
                *active.sync_status.lock().await =
                    SyncStatus::Degraded("cursor_read_failed".into());
                break;
            }
        };

        let mut sub = match log.subscribe(cursor).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("conversations subscribe failure: {e}");
                *active.sync_status.lock().await = SyncStatus::Degraded("subscribe_failed".into());
                break;
            }
        };

        // Inner receive loop — processes events from one subscription.
        let outcome = receive_loop(&backend, &active, &mut sub, &mut cancelled).await;
        sub.close().await;

        match outcome {
            ReceiveOutcome::Cancelled => break 'outer,
            ReceiveOutcome::StreamEnded | ReceiveOutcome::TransportError => {
                // Transition to catching-up and reconnect after backoff.
                *active.sync_status.lock().await = SyncStatus::CatchingUp;

                tokio::select! {
                    _ = cancelled.changed() => break 'outer,
                    _ = tokio::time::sleep(backoff) => {},
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

enum ReceiveOutcome {
    Cancelled,
    StreamEnded,
    TransportError,
}

/// Inner loop: process events from one subscription until cancellation,
/// stream close, or a transport error.
async fn receive_loop(
    backend: &ConversationsBackend,
    active: &Active,
    sub: &mut LogSubscription,
    cancelled: &mut watch::Receiver<bool>,
) -> ReceiveOutcome {
    loop {
        tokio::select! {
            result = cancelled.changed() => {
                if result.is_err() || *cancelled.borrow() {
                    return ReceiveOutcome::Cancelled;
                }
            }
            event = sub.recv() => match event {
                Some(Ok(LogEvent::Record {
                    record_id,
                    order_key,
                    author,
                    purpose,
                    signature,
                    canonical,
                    sealed_record,
                    cursor,
                })) => {
                    match active.store.ingest(
                        &record_id, &order_key, &author, &purpose, &signature,
                        &canonical, &sealed_record, cursor.as_deref(),
                    ) {
                        Ok(_) => {
                            backend.changes.send_modify(|x| *x = x.saturating_add(1));
                            // Quiescence fallback: first successful record
                            // ingestion transitions to current when no explicit
                            // state event arrived.
                            let mut status = active.sync_status.lock().await;
                            if *status == SyncStatus::CatchingUp {
                                *status = SyncStatus::Current;
                            }
                        }
                        Err(err) => {
                            eprintln!("conversations record ingestion failure: {err:?}");
                        }
                    }
                }
                Some(Ok(LogEvent::State { availability })) => {
                    if let Some(avail) = availability {
                        let mut status = active.sync_status.lock().await;
                        match avail.as_str() {
                            "current" => *status = SyncStatus::Current,
                            "catching_up" => *status = SyncStatus::CatchingUp,
                            _ => {}
                        }
                    }
                }
                Some(Ok(LogEvent::Warning { code, message })) => {
                    eprintln!(
                        "conversations subscription warning: code={code} message={}",
                        message.as_deref().unwrap_or("")
                    );
                }
                Some(Err(err @ LogError::Transport(_))) => {
                    eprintln!("conversations subscription transport failure: {err}");
                    return ReceiveOutcome::TransportError;
                }
                Some(Err(err)) => {
                    eprintln!("conversations event parse error: {err}");
                    // Continue on parse errors — a single malformed event
                    // does not tear down the subscription.
                }
                None => {
                    eprintln!("conversations record subscription closed");
                    return ReceiveOutcome::StreamEnded;
                }
            }
        }
    }
}
