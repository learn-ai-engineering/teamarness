use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Map, Value as JsonValue};
use teamarness_package_sdk::{
    Backend, BackendError, BackendRequest, EventSubscription, Package, event_channel,
};
use tokio::sync::{Mutex as AsyncMutex, watch};

use crate::domain::{DomainRecord, PAGE_SIZE, decode_id, hex_id};
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
        let active = Active {
            store: store.clone(),
            record_grant: sync.clone(),
        };
        let cursor = store
            .cursor()
            .map_err(|_| error("conversations.storage_failed", true))?;
        let mut sub = r
            .capabilities
            .subscribe(
                &sync,
                object([
                    ("method", JsonValue::String("subscribe".into())),
                    (
                        "afterCursor",
                        cursor.map_or(JsonValue::Null, JsonValue::String),
                    ),
                ]),
            )
            .await
            .map_err(|cause| capability_error("subscribe", cause))?;
        for batch in store
            .control_records()
            .map_err(|_| error("conversations.storage_failed", true))?
            .chunks(32)
        {
            let sealed = batch
                .iter()
                .map(|row| JsonValue::String(b64(&row.sealed_record)))
                .collect();
            let opened = r
                .capabilities
                .call(
                    &sync,
                    "open",
                    object([("sealedRecords", JsonValue::Array(sealed))]),
                )
                .await
                .map_err(|cause| capability_error("open-control-records", cause))?;
            if let Some(results) = opened.get("results").and_then(JsonValue::as_array) {
                for result in results {
                    if let Some(canonical) = result.get("canonical").and_then(JsonValue::as_str) {
                        let bytes = from_b64(canonical)?;
                        let _ = store.remember_opened(&bytes);
                    }
                }
            }
        }
        for batch in store
            .deferred_records()
            .map_err(|_| error("conversations.storage_failed", true))?
            .chunks(32)
        {
            let sealed = batch
                .iter()
                .map(|row| JsonValue::String(b64(&row.sealed_record)))
                .collect();
            let opened = r
                .capabilities
                .call(
                    &sync,
                    "open",
                    object([("sealedRecords", JsonValue::Array(sealed))]),
                )
                .await
                .map_err(|cause| capability_error("open-deferred-records", cause))?;
            if let Some(results) = opened.get("results").and_then(JsonValue::as_array) {
                for (row, result) in batch.iter().zip(results) {
                    if let Some(canonical) = result.get("canonical").and_then(JsonValue::as_str) {
                        let canonical = from_b64(canonical)?;
                        store
                            .retry_deferred(&row.record_id, &canonical)
                            .map_err(|_| error("conversations.storage_failed", true))?;
                    }
                }
            }
        }
        let backend = self.clone();
        let state = active.clone();
        let (cancel, mut cancelled) = watch::channel(false);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = cancelled.changed() => {
                        if result.is_err() || *cancelled.borrow() {
                            break;
                        }
                    }
                    event = sub.recv() => match event {
                        Some(Ok(event)) => match ingest_event(&state.store, &event) {
                            Ok(_) => backend.changes.send_modify(|revision| *revision = revision.saturating_add(1)),
                            Err(error) => eprintln!(
                                "conversations record ingestion failure: code={} retryable={}",
                                error.code(),
                                error.retryable()
                            ),
                        },
                        Some(Err(error)) => {
                            eprintln!(
                                "conversations record subscription failure: code={} retryable={}",
                                error.code(),
                                error.retryable()
                            );
                            break;
                        }
                        None => {
                            eprintln!("conversations record subscription closed");
                            break;
                        }
                    }
                }
            }
            sub.close().await;
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
            ("availability", JsonValue::String("current".into())),
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
        if has_more && let Some(next) = last_order && let JsonValue::Object(ref mut o) = page {
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
            if r.operation != "conversations.changes" && r.operation != "changes" {
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
fn ingest_event(store: &ConversationStore, v: &JsonValue) -> Result<IngestResult, BackendError> {
    let id = str_field(v, "recordId")?;
    let order = str_field(v, "orderKey")?;
    let author = str_field(v, "author")?;
    let purpose = str_field(v, "purpose")?;
    let sig = str_field(v, "signature")?;
    let canonical = from_b64(&str_field(v, "canonical")?)?;
    let sealed = from_b64(&str_field(v, "sealedRecord")?)?;
    let cursor = v.get("cursor").and_then(JsonValue::as_str);
    store
        .ingest(
            &id, &order, &author, &purpose, &sig, &canonical, &sealed, cursor,
        )
        .map_err(|_| error("conversations.storage_failed", true))
}
