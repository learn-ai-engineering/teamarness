use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value as JsonValue};
use teamarness_package_sdk::capabilities::record_log::{
    Availability, Cursor, OpenResult, RecordEvent, RecordLog, RecordLogSink, SessionOptions,
    SyncState, SyncWarning, run_session,
};
use teamarness_package_sdk::{
    Backend, BackendError, BackendRequest, CancellationToken, EventSubscription, Package,
    event_channel,
};
use tokio::sync::{Mutex as AsyncMutex, watch};

use crate::domain::{DomainRecord, PAGE_SIZE, decode_id, hex_id};
use crate::store::{ConversationStore, IngestResult};

/// How many distinct synchronization warning codes are remembered at a time.
const MAX_WARNINGS: usize = 8;

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
        object([("causeCode", JsonValue::String(cause.code().to_owned()))]),
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

#[derive(Clone)]
pub struct ConversationsBackend {
    pub(crate) active: Arc<AsyncMutex<Option<Running>>>,
    lifecycle: Arc<AsyncMutex<()>>,
    changes: watch::Sender<u64>,
}
#[derive(Clone)]
pub(crate) struct Active {
    pub(crate) store: ConversationStore,
    pub(crate) log: RecordLog,
    pub(crate) sync: Arc<Mutex<SyncReport>>,
}
pub(crate) struct Running {
    pub(crate) active: Active,
    pub(crate) cancellation: CancellationToken,
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
            running.cancellation.cancel();
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
        let data = r.initialization.data_directory().path();
        let store = ConversationStore::open(data.join("conversations.sqlite3"))
            .map_err(|_| storage_failed())?;
        let log = RecordLog::from_initialization(&r.initialization, r.capabilities.clone())
            .map_err(|cause| capability_error("record-log", cause))?;

        // Open control records (channel creates / hides) to rehydrate display data.
        let control_records = store.control_records().map_err(|_| storage_failed())?;
        for batch in control_records.chunks(32) {
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

        // Open and retry deferred records (e.g. messages before their channel).
        let deferred = store.deferred_records().map_err(|_| storage_failed())?;
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

        let sync = Arc::new(Mutex::new(SyncReport::catching_up()));
        let active = Active {
            store: store.clone(),
            log: log.clone(),
            sync: sync.clone(),
        };
        let sink = Arc::new(ConversationsSink {
            store,
            changes: self.changes.clone(),
            sync,
        });
        let cancellation = CancellationToken::new();
        let task = tokio::spawn(run_session(
            log,
            sink,
            cancellation.clone(),
            SessionOptions::default(),
        ));
        *self.active.lock().await = Some(Running {
            active,
            cancellation,
            task,
        });
        Ok(object([]))
    }

    async fn snapshot(&self) -> Result<JsonValue, BackendError> {
        let a = self.active().await?;
        let availability = a.report().availability;
        let channels = a.store.channels().map_err(|_| storage_failed())?;
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
            .map_err(|_| storage_failed())?
        {
            vec![JsonValue::String(
                "Some synchronized records could not be applied.".into(),
            )]
        } else {
            Vec::new()
        };
        Ok(object([
            ("channels", JsonValue::Array(visible)),
            (
                "availability",
                JsonValue::String(availability.as_str().to_owned()),
            ),
            ("warnings", JsonValue::Array(warnings)),
        ]))
    }

    async fn append(&self, purpose: &str, domain: DomainRecord) -> Result<JsonValue, BackendError> {
        let a = self.active().await?;
        let canonical = domain.encode();
        let appended = a
            .log
            .append(purpose, &canonical)
            .await
            .map_err(|cause| capability_error(purpose, cause))?;
        let outcome = a
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
            .map_err(|_| storage_failed())?;
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
            .map_err(|_| storage_failed())?;
        if rows.is_empty() {
            return Ok(object([("messages", JsonValue::Array(Vec::new()))]));
        }
        let has_more = rows.len() == page_limit;
        let sealed: Vec<Vec<u8>> = rows.iter().map(|x| x.4.clone()).collect();
        let results = a
            .log
            .open(&sealed)
            .await
            .map_err(|cause| capability_error("open-message-page", cause))?;
        let local_identity = r.initialization.installation().identity.as_str();
        let last_order = rows.last().map(|r| r.1.clone());
        let mut messages = Vec::new();
        for ((id, _order, author, _rid, _sealed, _ordinal), result) in
            rows.into_iter().zip(results.iter())
        {
            let OpenResult::Ready(record) = result else {
                continue;
            };
            if let Ok(DomainRecord::MessagePost { markdown, .. }) =
                DomainRecord::decode(&record.canonical)
            {
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

impl Active {
    /// The synchronization report the UI is shown.
    pub(crate) fn report(&self) -> SyncReport {
        self.sync
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
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
                    let author = r.initialization.installation().identity.as_str();
                    let d = DomainRecord::channel_create(author, slug)
                        .map_err(|_| error("conversations.invalid_slug", false))?;
                    self.append("channel.create", d).await
                }
                "conversations.channel.hide" => {
                    if r.payload.get("confirm").and_then(JsonValue::as_bool) != Some(true) {
                        return Err(error("conversations.confirmation_required", false));
                    }
                    let cid = decode_id(&str_field(&r.payload, "channelId")?)
                        .ok_or_else(|| error("conversations.invalid_request", false))?;
                    let author = r.initialization.installation().identity.as_str();
                    let d = DomainRecord::channel_hide(cid, author)
                        .map_err(|_| error("conversations.invalid_request", false))?;
                    self.append("channel.hide", d).await
                }
                "conversations.message.post" => {
                    let cid = decode_id(&str_field(&r.payload, "channelId")?)
                        .ok_or_else(|| error("conversations.invalid_request", false))?;
                    let active = self.active().await?;
                    if !active
                        .store
                        .channel_is_active(&hex_id(cid))
                        .map_err(|_| storage_failed())?
                    {
                        return Err(error("conversations.channel_hidden", false));
                    }
                    let md = str_field(&r.payload, "markdown")?;
                    let author = r.initialization.installation().identity.as_str();
                    let d = DomainRecord::message_post(cid, author, md)
                        .map_err(|_| error("conversations.invalid_markdown", false))?;
                    self.append("message.post", d).await
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
                            .map_err(|_| storage_failed())?;
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
    use teamarness_package_sdk::testing::{Harness, HarnessBuilder, ScriptedCapability};

    use super::*;
    use crate::domain::{GENERAL_ID, hex_id};

    const RECORD_LOG: &str = "teamarness.synchronization.immutable-record-log";
    const CHANNEL: [u8; 16] = [1; 16];

    fn hex(length: usize, seed: u64) -> String {
        format!("{:0width$x}", seed, width = length)
    }

    /// A capability that opens nothing and refuses anything it was not told about.
    fn scripted_log() -> ScriptedCapability {
        ScriptedCapability::new().calls(|method, payload| match method {
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
            _ => Err(BackendError::new(
                "capability_unavailable",
                false,
                json!({}),
            )),
        })
    }

    fn record_frame(seed: u64, purpose: &str, canonical: &[u8]) -> JsonValue {
        json!({
            "kind": "record",
            "cursor": hex(50, seed),
            "recordId": hex(64, seed),
            "orderKey": hex(82, seed),
            "author": "alice",
            "purpose": purpose,
            "signature": "c2ln",
            "canonical": URL_SAFE_NO_PAD.encode(canonical),
            "sealedRecord": "c2VhbGVk",
        })
    }

    struct Session {
        harness: Harness,
        backend: ConversationsBackend,
        scripted: ScriptedCapability,
    }

    async fn start(directory: &std::path::Path, scripted: ScriptedCapability) -> Session {
        let backend = ConversationsBackend::default();
        let mut harness = HarnessBuilder::new(Arc::new(backend.clone()) as Arc<dyn Backend>)
            .package_identity("dev.teamarness.conversations")
            .installation("alice", "Alice")
            .data_directory(directory)
            .capability(RECORD_LOG, scripted.clone())
            .start()
            .await;
        harness.activate().await.unwrap();
        Session {
            harness,
            backend,
            scripted,
        }
    }

    impl Session {
        async fn store(&self) -> ConversationStore {
            self.backend.active().await.unwrap().store
        }

        async fn availability(&self) -> Availability {
            self.backend.active().await.unwrap().report().availability
        }

        async fn warnings(&self) -> Vec<String> {
            self.backend.active().await.unwrap().report().warnings
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
        let store = session.store().await;
        wait_for_opens(&session.scripted, 1).await;

        let channel = DomainRecord::channel_create_with_id(CHANNEL, "alice", "team").unwrap();
        let first = DomainRecord::message_post(CHANNEL, "alice", "one").unwrap();
        let second = DomainRecord::message_post(CHANNEL, "alice", "two").unwrap();
        let sender = session.scripted.stream();
        assert!(
            sender
                .send(record_frame(1, "channel.create", &channel.encode()))
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
                .send(record_frame(2, "message.post", &first.encode()))
                .await
        );
        wait_for_opens(&session.scripted, 2).await;
        let sender = session.scripted.stream();
        assert!(
            sender
                .send(record_frame(2, "message.post", &first.encode()))
                .await
        );
        assert!(
            sender
                .send(record_frame(3, "message.post", &second.encode()))
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ingesting_a_record_does_not_mark_the_session_current() {
        let directory = tempfile::tempdir().unwrap();
        let session = start(directory.path(), scripted_log()).await;
        let store = session.store().await;
        wait_for_opens(&session.scripted, 1).await;

        let channel = DomainRecord::channel_create_with_id(CHANNEL, "alice", "team").unwrap();
        assert!(
            session
                .scripted
                .stream()
                .send(record_frame(1, "channel.create", &channel.encode()))
                .await
        );
        wait_until(
            || store.record_count().unwrap_or(0) == 1,
            "the ingested record",
        )
        .await;

        assert_eq!(
            session.availability().await,
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
            let reported = session.backend.clone();
            wait_until(
                || futures_now(&reported).is_some_and(|availability| availability == expected),
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
            || futures_now(&session.backend) == Some(Availability::Offline),
            "the refused state event",
        )
        .await;
        assert!(
            session
                .warnings()
                .await
                .contains(&"package.invalid_capability_response".to_owned()),
            "a retired availability is never mapped to a known one"
        );
        session.harness.shutdown().await.unwrap();
    }

    /// Reads the availability without awaiting, for a polling predicate.
    fn futures_now(backend: &ConversationsBackend) -> Option<Availability> {
        let running = backend.active.try_lock().ok()?;
        let active = running.as_ref()?;
        Some(active.active.report().availability)
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
            || futures_warnings(&session.backend).is_some_and(|warnings| warnings.len() == 8),
            "the bounded warning list",
        )
        .await;
        assert_eq!(session.warnings().await.len(), 8);
        session.harness.shutdown().await.unwrap();
    }

    fn futures_warnings(backend: &ConversationsBackend) -> Option<Vec<String>> {
        let running = backend.active.try_lock().ok()?;
        let active = running.as_ref()?;
        Some(active.active.report().warnings)
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
            || futures_now(&session.backend) == Some(Availability::Offline),
            "the offline availability",
        )
        .await;
        assert!(
            session
                .warnings()
                .await
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
            let channel = DomainRecord::channel_create_with_id(CHANNEL, "alice", "team").unwrap();
            store
                .ingest(
                    &hex(64, 1),
                    &hex(82, 1),
                    "alice",
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
        let store = session.store().await;
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
        let channel = DomainRecord::channel_create_with_id(CHANNEL, "alice", "team").unwrap();
        assert!(
            session
                .scripted
                .stream()
                .send(record_frame(1, "channel.create", &channel.encode()))
                .await
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(store.record_count().unwrap(), before);
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn deactivate_cancels_and_awaits_the_session_task() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        wait_for_opens(&session.scripted, 1).await;

        session.harness.dispose().await.unwrap();
        assert!(session.backend.active.lock().await.is_none());
        // A second dispose is harmless.
        session.harness.dispose().await.unwrap();
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn general_channel_is_visible_before_any_record() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        let snapshot = session
            .harness
            .request("conversations.snapshot", json!({}))
            .await
            .unwrap();
        assert_eq!(snapshot["availability"], "catching_up");
        let channels = snapshot["channels"].as_array().unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0]["id"], hex_id(GENERAL_ID));
        session.harness.shutdown().await.unwrap();
    }
}
