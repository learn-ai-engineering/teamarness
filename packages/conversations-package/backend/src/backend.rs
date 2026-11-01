use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{Value as JsonValue, json};
use teamarness_package_sdk::capabilities::principals::{PrincipalSnapshot, Principals};
use teamarness_package_sdk::capabilities::record_log::{
    Availability, Cursor, OpenResult, RecordEvent, RecordLog, RecordLogSink, SessionOptions,
    SyncState, SyncWarning, run_session,
};
use teamarness_package_sdk::{
    BackendError, CancellationToken, Context, Empty, Router, event_channel, ports::PortActor,
};
use tokio::sync::watch;

use crate::domain::{DomainRecord, PAGE_SIZE, decode_id, hex_id};
use crate::store::{ConversationStore, IngestResult, MessageRow};

/// How many distinct synchronization warning codes are remembered at a time.
const MAX_WARNINGS: usize = 8;
/// How many Channels' unsent drafts the Backend remembers; saving one more forgets the oldest.
const MAX_DRAFTS: usize = 64;

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
    .with_outcome_uncertain(cause.outcome_uncertain())
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
    principals: Principals,
    sync: Arc<Mutex<SyncReport>>,
    changes: watch::Sender<u64>,
    /// The Installation this Package Instance belongs to, which decides who may hide a Channel.
    installation: String,
    session: Mutex<Option<Session>>,
    /// The UI's unsent drafts: held in memory only, so they last while the Backend runs.
    drafts: Mutex<Drafts>,
}

/// Unsent drafts by Channel, oldest saved first, bounded to [`MAX_DRAFTS`] Channels.
///
/// They survive a Package switch but never a Backend restart.
#[derive(Debug, Default)]
struct Drafts(VecDeque<(String, String)>);

impl Drafts {
    fn get(&self, channel_id: &str) -> &str {
        self.0
            .iter()
            .find(|(id, _)| id == channel_id)
            .map_or("", |(_, text)| text.as_str())
    }

    /// Saves `text` as the newest draft, or forgets the Channel's draft when `text` is empty.
    fn set(&mut self, channel_id: String, text: String) {
        self.0.retain(|(id, _)| *id != channel_id);
        if text.is_empty() {
            return;
        }
        self.0.push_back((channel_id, text));
        while self.0.len() > MAX_DRAFTS {
            self.0.pop_front();
        }
    }
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
struct DraftGetRequest {
    channel_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DraftSetRequest {
    channel_id: String,
    text: String,
}

/// A Channel ID as the UI contract writes it: 32 lowercase hex characters.
fn draft_channel(channel_id: &str) -> Result<(), BackendError> {
    decode_id(channel_id)
        .map(|_| ())
        .ok_or_else(|| error("conversations.invalid_request", false))
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
            let principals =
                Principals::from_initialization(&initialization, activation.capabilities().clone())
                    .map_err(|cause| capability_error("principals", cause))?;
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
                principals,
                sync,
                changes,
                installation: initialization.installation().identity.clone(),
                session: Mutex::new(Some(session)),
                drafts: Mutex::new(Drafts::default()),
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
            let author = Author::person(context.state().installation.clone());
            post(&context, author, request).await
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
        "conversations.draft.get",
        |context: Context<ConversationsActive>, request: DraftGetRequest| async move {
            draft_channel(&request.channel_id)?;
            let drafts = context
                .state()
                .drafts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Ok(json!({ "text": drafts.get(&request.channel_id) }))
        },
    )
    .request(
        "conversations.draft.set",
        |context: Context<ConversationsActive>, request: DraftSetRequest| async move {
            draft_channel(&request.channel_id)?;
            if request.text.len() > crate::domain::MAX_MARKDOWN {
                return Err(error("conversations.invalid_request", false));
            }
            context
                .state()
                .drafts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set(request.channel_id, request.text);
            Ok(json!({}))
        },
    )
    .request(
        "conversations.channel.resolve",
        |context: Context<ConversationsActive>, request: ResolveRequest| async move {
            resolve(&context, request).await
        },
    )
    .port_request(
        "dev.teamarness.conversations", "message.post", "command",
        |context: Context<ConversationsActive>, actor: PortActor, request: PostRequest| async move {
            let author = authorize_actor(&context, actor)?;
            post(&context, author, request).await
        },
    )
    .port_request(
        "dev.teamarness.conversations", "message.page", "query",
        |context: Context<ConversationsActive>, _actor: PortActor, request: PageRequest| async move {
            page(&context, request).await
        },
    )
    .port_request(
        "dev.teamarness.conversations", "message.changes", "query",
        |context: Context<ConversationsActive>, _actor: PortActor, request: ChangesRequest| async move {
            changes(&context, request).await
        },
    )
    .port_request(
        "dev.teamarness.conversations", "channel.list", "query",
        |context: Context<ConversationsActive>, _actor: PortActor, _: Empty| async move {
            let state = context.state().clone();
            context.blocking(move |_| state.channel_list_value()).await
        },
    )
    .port_request(
        "dev.teamarness.conversations", "runtime.guide", "query",
        |_context: Context<ConversationsActive>, _actor: PortActor, _: Empty| async move {
            Ok(crate::guide::response(CHANNEL_LIST_LIMIT))
        },
    )
    .port_event(
        "dev.teamarness.conversations", "messages.changed",
        |context: Context<ConversationsActive>, _actor: PortActor| async move {
            let (sender, subscription) = event_channel(1);
            let mut revisions = context.state().changes.subscribe();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        result = revisions.changed() => {
                            if result.is_err() || sender.send(json!({})).await.is_err() { break; }
                        }
                        () = sender.closed() => break,
                    }
                }
            });
            Ok(subscription)
        },
    )
    .subscription(
        "conversations.principals",
        |context: Context<ConversationsActive>, _: Empty| async move {
            let (sender, subscription) = event_channel(4);
            let principals = context.state().principals.clone();
            tokio::spawn(async move {
                loop {
                    let mut cursor = match principals.current().await {
                        Ok(snapshot) => {
                            let cursor = snapshot.cursor;
                            for event in principal_snapshots(snapshot) {
                                if sender.send(event).await.is_err() {
                                    return;
                                }
                            }
                            cursor
                        }
                        Err(_) => {
                            if sender
                                .send(
                                    json!({ "available": false, "cursor": "0", "principals": [] }),
                                )
                                .await
                                .is_err()
                            {
                                return;
                            }
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                            continue;
                        }
                    };
                    if let Ok(mut stream) = principals.subscribe(cursor).await {
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
                                        for event in principal_snapshots(snapshot) {
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
                        .send(json!({ "available": false, "cursor": "0", "principals": [] }))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            });
            Ok(subscription)
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

fn principal_snapshots(snapshot: PrincipalSnapshot) -> Vec<JsonValue> {
    // Capability bounds the roster to 1,024 Principals and each name to 256 bytes.
    // 64 entries fit the bridge budget even with JSON-escaped names.
    const PRINCIPALS_PER_EVENT: usize = 64;
    let principals: Vec<_> = snapshot
        .principals
        .into_iter()
        .map(|principal| {
            json!({
                "identity": principal.keys[0], "principalId": principal.principal_id,
                "kind": match principal.kind {
                    teamarness_package_sdk::capabilities::principals::PrincipalKind::Person => "person",
                    teamarness_package_sdk::capabilities::principals::PrincipalKind::Agent => "agent",
                },
                "stewardPrincipalId": principal.steward_principal_id,
                "name": principal.name, "fingerprint": principal.fingerprint,
            })
        })
        .collect();
    let parts = principals.len().max(1).div_ceil(PRINCIPALS_PER_EVENT);
    principals
        .chunks(PRINCIPALS_PER_EVENT)
        .enumerate()
        .map(|(part, chunk)| {
            json!({
                "available": true, "cursor": snapshot.cursor.to_string(),
                "principals": chunk, "part": part, "parts": parts,
            })
        })
        .chain((principals.is_empty()).then(|| {
            json!({
                "available": true, "cursor": snapshot.cursor.to_string(),
                "principals": [], "part": 0, "parts": 1,
            })
        }))
        .collect()
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

    /// The `channel.list` Port answer: the Channels the UI snapshot shows, as `{id, name}`.
    ///
    /// TEAM.SYS.CONV-R18: same visibility as the snapshot (hidden Channels excluded, creation
    /// order, `#general` first), bounded to the contract's 64 rows.
    fn channel_list_value(&self) -> Result<JsonValue, BackendError> {
        let channels = self.store.channels().map_err(|_| storage_failed())?;
        let visible: Vec<JsonValue> = channels
            .into_iter()
            .filter(|channel| !channel.hidden)
            .take(CHANNEL_LIST_LIMIT)
            .map(|channel| json!({ "id": channel.id, "name": channel.slug }))
            .collect();
        Ok(json!({ "channels": visible }))
    }
}

/// The most Channels one `channel.list` answer carries, which the Port Contract also pins.
const CHANNEL_LIST_LIMIT: usize = 64;

/// Who a Message is written as: the record's declared author and, for an Agent, the Principal
/// Core signs for.
struct Author {
    /// The signing key the record names as its author, which Core must sign with.
    identity: String,
    /// The Agent's Principal ID for a record-log 1.1 append; `None` for the Installation Person.
    principal: Option<String>,
}

impl Author {
    fn person(identity: String) -> Self {
        Self {
            identity,
            principal: None,
        }
    }
}

/// Turns the Host-authored actor into the author a post is written as.
///
/// A Person must be this Installation. An Agent actor is Host-derived from the calling Package
/// Instance's Agent Binding (ADR 0030), so it is accepted as given: the record names the Agent's
/// key and Core signs with that key, or refuses with `principal_not_bound`, when the Package
/// appends with its Principal ID.
fn authorize_actor(
    context: &Context<ConversationsActive>,
    actor: PortActor,
) -> Result<Author, BackendError> {
    match actor {
        PortActor::Person { identity } if identity == context.state().installation => {
            Ok(Author::person(identity))
        }
        PortActor::Agent {
            principal,
            identity,
        } => Ok(Author {
            identity,
            principal: Some(principal),
        }),
        PortActor::Person { .. } => Err(error("conversations.actor_unauthorized", false)),
    }
}

async fn post(
    context: &Context<ConversationsActive>,
    author: Author,
    request: PostRequest,
) -> Result<JsonValue, BackendError> {
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
    // The caller never selects the signer: Core signs for this Installation, or for the bound
    // Agent whose Principal ID the Host-derived actor named.
    let record = DomainRecord::message_post(channel, &author.identity, request.markdown)
        .map_err(|_| error("conversations.invalid_markdown", false))?;
    append_as(context, "message.post", record, author.principal.as_deref()).await
}

/// Appends one domain record signed by this Installation and projects it locally.
async fn append(
    context: &Context<ConversationsActive>,
    purpose: &'static str,
    record: DomainRecord,
) -> Result<JsonValue, BackendError> {
    append_as(context, purpose, record, None).await
}

/// Appends one domain record, signed by the named Agent when `principal` is given, and projects
/// it locally.
async fn append_as(
    context: &Context<ConversationsActive>,
    purpose: &'static str,
    record: DomainRecord,
    principal: Option<&str>,
) -> Result<JsonValue, BackendError> {
    let canonical = record.encode();
    let appended = context
        .state()
        .log
        .append_with_principal(purpose, &canonical, principal)
        .await
        .map_err(|cause| capability_error(purpose, cause))?;
    let state = context.state().clone();
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
        .await
        .map_err(|failure| failure.with_outcome_uncertain(true))?;
    match outcome {
        IngestResult::Applied | IngestResult::Duplicate => {}
        IngestResult::Deferred(_) => {
            return Err(error("conversations.deferred", false).with_outcome_uncertain(true));
        }
        IngestResult::Quarantined(_) => {
            return Err(error("conversations.rejected", false).with_outcome_uncertain(true));
        }
    }
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
    /// A Workspace Agent Alice stewards: its Principal ID and its current signing key.
    const AGENT_PRINCIPAL: &str =
        "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    const AGENT_KEY: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

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
                // Core signs with the Agent's key when the 1.1 `principal` names a bound Agent.
                let author = match payload.get("principal").and_then(JsonValue::as_str) {
                    Some(AGENT_PRINCIPAL) => AGENT_KEY,
                    Some(_) => {
                        return Err(BackendError::new("principal_not_bound", false, json!({})));
                    }
                    None => ALICE,
                };
                Ok(json!({
                    "recordId": hex(64, seed),
                    "orderKey": hex(82, seed),
                    "author": author,
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
                .capability_minor(RECORD_LOG, scripted.clone(), 1)
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

    // Verifies: TEAM.SYS.CONV-R20
    #[tokio::test]
    async fn runtime_guide_is_schema_valid_bounded_and_explains_recovery() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        let response = session
            .harness
            .request(
                "teamarness.port.query.v1",
                json!({
                    "contract":"dev.teamarness.conversations", "major":1, "minor":3,
                    "port":"runtime.guide", "actor":{"kind":"person","identity":ALICE},
                    "request":{}
                }),
            )
            .await
            .expect("the actual public Port router serves runtime.guide");
        let guide = &response["$teamarnessPortValue"];
        let contract = teamarness_package_contract::port_contract::compile_port_contracts(
            include_bytes!("../../ports.json"),
        )
        .unwrap();
        assert!(contract.has_guide("dev.teamarness.conversations"));
        assert!(
            contract
                .validate(
                    "dev.teamarness.conversations",
                    "runtime.guide",
                    "response",
                    guide
                )
                .is_ok()
        );
        let provided = &contract.contracts[0];
        let list = provided
            .ports
            .iter()
            .find(|port| port.name == "channel.list")
            .unwrap();
        assert_eq!(
            list.response.as_ref().unwrap()["properties"]["channels"]["maxItems"],
            CHANNEL_LIST_LIMIT
        );
        assert_eq!(
            contract.definitions["changes"]["properties"]["events"]["maxItems"],
            PAGE_SIZE
        );
        let post = provided
            .ports
            .iter()
            .find(|port| port.name == "message.post")
            .unwrap();
        assert_eq!(
            post.request.as_ref().unwrap()["properties"]["markdown"]["maxLength"],
            crate::domain::MAX_MARKDOWN
        );
        assert!(serde_json::to_vec(guide).unwrap().len() <= 32 * 1024);
        // Include the process envelope in the size qualification, not just the Markdown.
        assert!(serde_json::to_vec(&response).unwrap().len() <= 32 * 1024);
        let markdown = guide["markdown"].as_str().unwrap();
        for advice in [
            format!("at most {CHANNEL_LIST_LIMIT} visible Channels"),
            format!("at most {PAGE_SIZE} raw rows"),
            format!("at most {PAGE_SIZE} changes"),
            format!("{} UTF-8 bytes", crate::domain::MAX_MARKDOWN),
            "Subscribe to messages.changed before the initial message.page".into(),
            "epoch and revision".into(),
            "continuation".into(),
            "highWater".into(),
            "acknowledged prefix".into(),
            "pending".into(),
            "unavailable".into(),
            "resyncRequired".into(),
            "reconstruct".into(),
            "do not mark Channels read".into(),
            "Host-derived actor".into(),
            "conversations.channel_hidden".into(),
            "provider_error".into(),
            "details.provider.code".into(),
            "outcomeUncertain".into(),
            "Never automatically retry an uncertain message.post".into(),
        ] {
            assert!(markdown.contains(&advice), "missing guidance: {advice}");
        }
        for invalid in [
            json!({}),
            json!({"markdown":true}),
            json!({"markdown":"x","extra":0}),
            json!({"markdown":"x".repeat(32769)}),
        ] {
            assert!(
                contract
                    .validate(
                        "dev.teamarness.conversations",
                        "runtime.guide",
                        "response",
                        &invalid
                    )
                    .is_err()
            );
        }
        assert!(
            contract
                .validate(
                    "dev.teamarness.conversations",
                    "runtime.guide",
                    "request",
                    &json!({"extra":0})
                )
                .is_err()
        );
        // The guide distinguishes the schema's character bound from enforced UTF-8 bytes.
        let oversized = json!({"channelId":hex_id(GENERAL_ID), "markdown":"é".repeat(crate::domain::MAX_MARKDOWN / 2 + 1)});
        assert!(
            contract
                .validate(
                    "dev.teamarness.conversations",
                    "message.post",
                    "request",
                    &oversized
                )
                .is_ok()
        );
        let refused = session
            .harness
            .request(
                "teamarness.port.command.v1",
                json!({
                    "contract":"dev.teamarness.conversations", "major":1, "minor":3,
                    "port":"message.post", "actor":{"kind":"person","identity":ALICE},
                    "request":oversized
                }),
            )
            .await
            .unwrap_err();
        assert_eq!(refused.code(), "conversations.invalid_markdown");
        assert!(!refused.outcome_uncertain());
        assert_eq!(session.store().record_count().unwrap(), 0);
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.CONV-R20, TEAM.SYS.PKG-R12
    #[test]
    fn runtime_guide_preserves_the_five_existing_operation_shapes() {
        use std::collections::BTreeSet;
        let mut contract = teamarness_package_contract::port_contract::compile_port_contracts(
            include_bytes!("../../ports.json"),
        )
        .unwrap();
        assert_eq!(
            (contract.contracts[0].major, contract.contracts[0].minor),
            (1, 3)
        );
        assert_eq!(contract.contracts[0].ports.len(), 6);
        // Snapshot of the five operations at 9985fd9: names, kinds, schemas and all
        // reachable definitions. Ignore descriptions and the additive minor only.
        let names = BTreeSet::from([
            "channel.list".into(),
            "message.changes".into(),
            "message.page".into(),
            "message.post".into(),
            "messages.changed".into(),
        ]);
        contract.contracts[0].minor = 0;
        contract.contracts[0].description = "Operation shape snapshot".into();
        for port in &mut contract.contracts[0].ports {
            port.description = "Operation shape snapshot".into();
        }
        let metadata = contract
            .project_metadata("dev.teamarness.conversations", Some(&names))
            .unwrap();
        assert_eq!(
            metadata.sha256,
            "3afcd9a8669e41a6d601d9f0b2f9133ee75e8318cee2c6b80eab854bc96c81be"
        );
    }

    #[test]
    fn public_port_contract_has_exact_closed_bounded_shapes() {
        let contract = teamarness_package_contract::port_contract::compile_port_contracts(
            include_bytes!("../../ports.json"),
        )
        .unwrap();
        assert_eq!(contract.contracts.len(), 1);
        let provided = &contract.contracts[0];
        assert_eq!(provided.identity, "dev.teamarness.conversations");
        assert_eq!((provided.major, provided.minor), (1, 3));
        assert_eq!(provided.ports.len(), 6);
        let list = |direction: &str, value: &JsonValue| {
            contract.validate(
                "dev.teamarness.conversations",
                "channel.list",
                direction,
                value,
            )
        };
        assert!(list("request", &json!({})).is_ok());
        assert!(list("request", &json!({"limit": 1})).is_err());
        let channel = json!({"id": hex_id(GENERAL_ID), "name": "general"});
        assert!(list("response", &json!({"channels": [channel]})).is_ok());
        assert!(list("response", &json!({"channels": []})).is_ok());
        assert!(list("response", &json!({"channels": vec![channel.clone(); 64]})).is_ok());
        assert!(list("response", &json!({"channels": vec![channel.clone(); 65]})).is_err());
        assert!(list("response", &json!({})).is_err());
        assert!(
            list(
                "response",
                &json!({"channels": [{"id": hex_id(GENERAL_ID)}]})
            )
            .is_err()
        );
        assert!(
            list(
                "response",
                &json!({"channels": [{"id": hex_id(GENERAL_ID), "name": "general", "unreadCount": 1}]})
            )
            .is_err()
        );
        assert!(
            contract
                .validate(
                    "dev.teamarness.conversations",
                    "message.post",
                    "request",
                    &json!({"channelId":hex_id(GENERAL_ID),"markdown":"hello"})
                )
                .is_ok()
        );
        for invalid in [
            json!({"channelId":hex_id(GENERAL_ID),"markdown":""}),
            json!({"channelId":hex_id(GENERAL_ID),"markdown":"x","actor":"forged"}),
            json!({"channelId":hex_id(GENERAL_ID),"markdown":"x".repeat(32769)}),
        ] {
            assert!(
                contract
                    .validate(
                        "dev.teamarness.conversations",
                        "message.post",
                        "request",
                        &invalid
                    )
                    .is_err()
            );
        }
        assert!(
            contract
                .validate(
                    "dev.teamarness.conversations",
                    "messages.changed",
                    "event",
                    &json!({})
                )
                .is_ok()
        );
        assert!(
            contract
                .validate(
                    "dev.teamarness.conversations",
                    "messages.changed",
                    "event",
                    &json!({"markdown":"secret"})
                )
                .is_err()
        );
        const { assert!(HISTORY_REPLY_BUDGET < teamarness_package_protocol::MAX_FRAME_BYTES) };
    }

    #[tokio::test]
    async fn ports_share_ui_post_and_read_only_page_and_invalidate_live() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        let envelope = |port: &str, actor: JsonValue, request: JsonValue| {
            json!({
                "contract": "dev.teamarness.conversations", "major": 1, "minor": 0,
                "port": port, "actor": actor, "request": request
            })
        };
        let person = json!({"kind":"person","identity":ALICE});
        let page_request = json!({"channelId":hex_id(GENERAL_ID)});
        let initial = session
            .harness
            .request(
                "teamarness.port.query.v1",
                envelope("message.page", person.clone(), page_request.clone()),
            )
            .await
            .unwrap()["$teamarnessPortValue"]
            .clone();
        let mut events = session
            .harness
            .subscribe(
                "teamarness.port.event.v1",
                envelope("messages.changed", person.clone(), json!({})),
            )
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), events.next_event())
                .await
                .is_err(),
            "subscription must not emit its current watch value"
        );
        let forbidden = session
            .harness
            .request(
                "teamarness.port.command.v1",
                envelope(
                    "message.post",
                    json!({"kind":"person","identity":"stranger"}),
                    json!({"channelId":hex_id(GENERAL_ID),"markdown":"forbidden"}),
                ),
            )
            .await
            .unwrap_err();
        assert_eq!(forbidden.code(), "conversations.actor_unauthorized");
        assert!(
            session
                .harness
                .request(
                    "teamarness.port.command.v1",
                    envelope(
                        "message.post",
                        json!({"kind":"agent","identity":ALICE}),
                        json!({"channelId":hex_id(GENERAL_ID),"markdown":"forbidden"})
                    )
                )
                .await
                .is_err()
        );
        assert_eq!(
            session
                .harness
                .request(
                    "teamarness.port.command.v1",
                    envelope(
                        "message.post",
                        person.clone(),
                        json!({"channelId":hex_id(GENERAL_ID),"markdown":"hello"})
                    )
                )
                .await
                .unwrap(),
            json!({"$teamarnessPortEnvelope":1,"$teamarnessPortValue":{"ok":true}})
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), events.next_event())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            json!({"$teamarnessPortEnvelope":1,"$teamarnessPortValue":{}})
        );
        assert_eq!(
            session.store().read_ordinal(&hex_id(GENERAL_ID)).unwrap(),
            0
        );
        let page = session
            .harness
            .request(
                "teamarness.port.query.v1",
                envelope("message.page", person.clone(), page_request.clone()),
            )
            .await
            .unwrap()["$teamarnessPortValue"]
            .clone();
        assert_eq!(page["pending"].as_array().unwrap().len(), 1);
        assert_ne!(page["revision"], initial["revision"]);
        let ui = session
            .harness
            .request("conversations.channel.page", page_request)
            .await
            .unwrap();
        assert_eq!(page, ui);
        assert_eq!(
            session.store().read_ordinal(&hex_id(GENERAL_ID)).unwrap(),
            0
        );
        session
            .harness
            .request(
                "conversations.channel.open",
                json!({"channelId":hex_id(GENERAL_ID)}),
            )
            .await
            .unwrap();
        assert!(session.store().read_ordinal(&hex_id(GENERAL_ID)).unwrap() > 0);
        events.close().await;
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.CONV-R19
    #[tokio::test]
    async fn message_changes_port_reuses_revision_recovery_without_marking_read() {
        let directory = tempfile::tempdir().unwrap();
        let (entered, _) = std::sync::mpsc::channel();
        let (_, release) = std::sync::mpsc::channel();
        let mut session = start(
            directory.path(),
            paused_history_log(
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
                entered,
                release,
            ),
        )
        .await;
        let store = session.store();
        commit_history_row(&store, 100);
        let (epoch, baseline) = store.history_head().unwrap();
        let late_id = commit_history_row(&store, 2);
        let next_id = commit_history_row(&store, 3);
        let envelope = |request: JsonValue| {
            json!({
                "contract": "dev.teamarness.conversations", "major": 1, "minor": 2,
                "port": "message.changes", "actor": {"kind":"person", "identity": ALICE},
                "request": request,
            })
        };
        let request = json!({"channelId": hex_id(GENERAL_ID), "epoch": epoch,
            "after": baseline.to_string(), "limit": 1});
        let ui = session
            .harness
            .request("conversations.channel.changes", request.clone())
            .await
            .unwrap();
        assert_eq!(ui["events"][0]["id"], late_id);
        assert_eq!(ui["events"][0]["position"], hex(82, 2));
        assert_eq!(ui["events"][0]["message"]["markdown"], "row 2");
        let port = session
            .harness
            .request("teamarness.port.query.v1", envelope(request))
            .await
            .unwrap();
        assert_eq!(port["$teamarnessPortValue"], ui);
        let second = session.harness.request("teamarness.port.query.v1", envelope(json!({
            "channelId": hex_id(GENERAL_ID), "epoch": epoch, "after": ui["continuation"], "limit": 1,
        }))).await.unwrap()["$teamarnessPortValue"].clone();
        assert_eq!(second["events"][0]["id"], next_id);
        assert_eq!(second["continuation"], "3");
        let terminal = session.harness.request("teamarness.port.query.v1", envelope(json!({
            "channelId": hex_id(GENERAL_ID), "epoch": epoch, "after": second["continuation"],
        }))).await.unwrap()["$teamarnessPortValue"].clone();
        assert_eq!(terminal["events"], json!([]));
        assert_eq!(terminal["continuation"], terminal["highWater"]);
        let stale = session
            .harness
            .request(
                "teamarness.port.query.v1",
                envelope(json!({
                    "channelId": hex_id(GENERAL_ID), "epoch": "0".repeat(32), "after": "0",
                })),
            )
            .await
            .unwrap()["$teamarnessPortValue"]
            .clone();
        assert_eq!(stale["resyncRequired"], true);
        assert_eq!(store.read_ordinal(&hex_id(GENERAL_ID)).unwrap(), 0);
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.PKG-R12, TEAM.SYS.CONV-R19
    #[test]
    fn message_changes_port_is_closed_bounded_and_additive_over_1_1() {
        use teamarness_package_contract::port_contract::{
            check_additive_minor_documents, compile_port_contracts,
        };
        let old =
            compile_port_contracts(include_bytes!("../tests/fixtures/ports-1.1.json")).unwrap();
        let new = compile_port_contracts(include_bytes!("../../ports.json")).unwrap();
        assert_eq!(new.contracts[0].minor, 3);
        check_additive_minor_documents(&old, &new, "dev.teamarness.conversations").unwrap();
        for before in &old.contracts[0].ports {
            let after = new.contracts[0]
                .ports
                .iter()
                .find(|port| port.name == before.name)
                .unwrap();
            assert_eq!(before.kind, after.kind);
            assert_eq!(before.request, after.request);
            assert_eq!(before.response, after.response);
            assert_eq!(before.event, after.event);
        }
        let validate = |direction, value: &JsonValue| {
            new.validate(
                "dev.teamarness.conversations",
                "message.changes",
                direction,
                value,
            )
        };
        let request = json!({"channelId": hex_id(GENERAL_ID), "epoch": "a".repeat(32), "after": "0", "limit": 32});
        assert!(validate("request", &request).is_ok());
        for (field, value) in [
            ("limit", json!(0)),
            ("limit", json!(33)),
            ("after", json!("-1")),
            ("actor", json!({})),
        ] {
            let mut invalid = request.clone();
            invalid[field] = value;
            assert!(validate("request", &invalid).is_err());
        }
        let event = json!({"id":"message", "position":"01", "kind":"insert", "revision":"1", "status":"pending"});
        assert!(
            validate(
                "response",
                &json!({"events": vec![event.clone();32], "continuation":"1", "highWater":"1"})
            )
            .is_ok()
        );
        assert!(
            validate(
                "response",
                &json!({"events": vec![event;33], "continuation":"1", "highWater":"1"})
            )
            .is_err()
        );
        assert!(
            validate(
                "response",
                &json!({"events":[], "continuation":"1", "highWater":"1", "unexpected":true})
            )
            .is_err()
        );
    }

    // Verifies: TEAM.SYS.PKG-R12
    #[test]
    fn current_port_contract_is_additive_over_the_shipped_1_0_document() {
        use teamarness_package_contract::port_contract::{
            check_additive_minor_documents, compile_port_contracts,
        };
        let old =
            compile_port_contracts(include_bytes!("../tests/fixtures/ports-1.0.json")).unwrap();
        let new = compile_port_contracts(include_bytes!("../../ports.json")).unwrap();
        assert_eq!((old.contracts[0].major, old.contracts[0].minor), (1, 0));
        assert_eq!((new.contracts[0].major, new.contracts[0].minor), (1, 3));
        check_additive_minor_documents(&old, &new, "dev.teamarness.conversations").unwrap();
        for name in ["message.post", "message.page", "messages.changed"] {
            let before = old.contracts[0]
                .ports
                .iter()
                .find(|port| port.name == name)
                .unwrap();
            let after = new.contracts[0]
                .ports
                .iter()
                .find(|port| port.name == name)
                .unwrap();
            assert_eq!(before.kind, after.kind);
            assert_eq!(before.request, after.request);
            assert_eq!(before.response, after.response);
            assert_eq!(before.event, after.event);
        }
    }

    // Verifies: TEAM.SYS.CONV-R18
    #[tokio::test]
    async fn agent_actor_post_appends_with_principal_and_projects_the_agent_author() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        let command = |actor: JsonValue, markdown: &str| {
            json!({
                "contract": "dev.teamarness.conversations", "major": 1, "minor": 1,
                "port": "message.post", "actor": actor,
                "request": {"channelId": hex_id(GENERAL_ID), "markdown": markdown}
            })
        };
        let agent = json!({"kind":"agent","principal":AGENT_PRINCIPAL,"identity":AGENT_KEY});
        assert_eq!(
            session
                .harness
                .request(
                    "teamarness.port.command.v1",
                    command(agent.clone(), "as agent")
                )
                .await
                .unwrap()["$teamarnessPortValue"],
            json!({"ok": true})
        );
        assert_eq!(
            session
                .harness
                .request(
                    "teamarness.port.command.v1",
                    command(json!({"kind":"person","identity":ALICE}), "as person")
                )
                .await
                .unwrap()["$teamarnessPortValue"],
            json!({"ok": true})
        );
        let appends: Vec<_> = session
            .scripted
            .recorded_calls()
            .into_iter()
            .filter(|call| call.method == "append")
            .collect();
        assert_eq!(appends.len(), 2);
        assert_eq!(appends[0].payload["principal"], AGENT_PRINCIPAL);
        assert_eq!(appends[0].payload["purpose"], "message.post");
        assert!(appends[1].payload.get("principal").is_none());
        let rows = session
            .store()
            .message_rows(&hex_id(GENERAL_ID), None, 8)
            .unwrap();
        let authors: Vec<&str> = rows.iter().map(|row| row.2.as_str()).collect();
        assert_eq!(authors, vec![ALICE, AGENT_KEY], "newest first");
        for (call, signer) in appends.iter().zip([AGENT_KEY, ALICE]) {
            let canonical = URL_SAFE_NO_PAD
                .decode(call.payload["canonical"].as_str().unwrap())
                .unwrap();
            let DomainRecord::MessagePost { author, .. } =
                DomainRecord::decode(&canonical).unwrap()
            else {
                panic!("a Message record");
            };
            assert_eq!(author, signer, "payload author equals the signed author");
        }
        assert!(!session.store().has_projection_warnings().unwrap());

        // A Person from another Installation is still refused, and an Agent actor that the Host
        // did not fully author (no principal) never reaches the handler.
        let stranger = session
            .harness
            .request(
                "teamarness.port.command.v1",
                command(json!({"kind":"person","identity":"stranger"}), "forbidden"),
            )
            .await
            .unwrap_err();
        assert_eq!(stranger.code(), "conversations.actor_unauthorized");
        assert!(
            session
                .harness
                .request(
                    "teamarness.port.command.v1",
                    command(json!({"kind":"agent","identity":AGENT_KEY}), "forbidden")
                )
                .await
                .is_err()
        );
        assert_eq!(
            session
                .scripted
                .recorded_calls()
                .into_iter()
                .filter(|call| call.method == "append")
                .count(),
            2
        );
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.CONV-R18
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn channel_list_matches_snapshot_visibility_and_is_bounded() {
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
        let mine = snapshot["channels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|channel| channel["slug"] == "mine")
            .unwrap()["id"]
            .clone();
        session
            .harness
            .request(
                "conversations.channel.hide",
                json!({ "channelId": mine, "confirm": true }),
            )
            .await
            .unwrap();

        let list = |actor: JsonValue| {
            json!({
                "contract": "dev.teamarness.conversations", "major": 1, "minor": 1,
                "port": "channel.list", "actor": actor, "request": {}
            })
        };
        let expected = |snapshot: &JsonValue, limit: usize| -> JsonValue {
            JsonValue::Array(
                snapshot["channels"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .take(limit)
                    .map(|channel| json!({"id": channel["id"], "name": channel["slug"]}))
                    .collect(),
            )
        };
        let snapshot = session
            .harness
            .request("conversations.snapshot", empty_payload())
            .await
            .unwrap();
        assert_eq!(snapshot["channels"].as_array().unwrap().len(), 2);
        let person = json!({"kind":"person","identity":ALICE});
        let listed = session
            .harness
            .request("teamarness.port.query.v1", list(person.clone()))
            .await
            .unwrap()["$teamarnessPortValue"]
            .clone();
        assert_eq!(listed, json!({"channels": expected(&snapshot, 64)}));
        assert_eq!(listed["channels"][0]["name"], "general");
        assert_eq!(listed["channels"][1]["name"], "theirs");
        let as_agent = session
            .harness
            .request(
                "teamarness.port.query.v1",
                list(json!({"kind":"agent","principal":AGENT_PRINCIPAL,"identity":AGENT_KEY})),
            )
            .await
            .unwrap()["$teamarnessPortValue"]
            .clone();
        assert_eq!(as_agent, listed);
        assert!(
            session
                .harness
                .request(
                    "teamarness.port.query.v1",
                    json!({
                        "contract": "dev.teamarness.conversations", "major": 1, "minor": 1,
                        "port": "channel.list", "actor": person.clone(), "request": {"limit": 1}
                    }),
                )
                .await
                .is_err()
        );

        for n in 0..70 {
            session
                .harness
                .request(
                    "conversations.channel.create",
                    json!({ "slug": format!("room-{n}") }),
                )
                .await
                .unwrap();
        }
        let snapshot = session
            .harness
            .request("conversations.snapshot", empty_payload())
            .await
            .unwrap();
        assert_eq!(snapshot["channels"].as_array().unwrap().len(), 72);
        let listed = session
            .harness
            .request("teamarness.port.query.v1", list(person))
            .await
            .unwrap()["$teamarnessPortValue"]
            .clone();
        assert_eq!(listed["channels"].as_array().unwrap().len(), 64);
        assert_eq!(listed, json!({"channels": expected(&snapshot, 64)}));
        session.harness.shutdown().await.unwrap();
    }

    #[test]
    fn uncertain_log_append_failure_remains_uncertain_at_port_boundary() {
        let cause =
            BackendError::new("outcome_unknown", false, json!({})).with_outcome_uncertain(true);
        let failure = super::capability_error("message.post", cause);
        assert!(failure.outcome_uncertain());
        assert!(!failure.retryable());
    }

    #[tokio::test]
    async fn committed_post_with_failed_local_projection_is_uncertain_and_not_retryable() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        let store = session.store();
        let before = session
            .scripted
            .recorded_calls()
            .into_iter()
            .filter(|call| call.method == "append")
            .count();
        store.fail_next_ingest_for_test();
        let failure = session
            .harness
            .request(
                "teamarness.port.command.v1",
                json!({
                    "contract":"dev.teamarness.conversations", "major":1, "minor":0,
                    "port":"message.post", "actor":{"kind":"person","identity":ALICE},
                    "request":{"channelId":hex_id(GENERAL_ID),"markdown":"committed once"}
                }),
            )
            .await
            .unwrap_err();
        assert_eq!(failure.code(), "conversations.storage_failed");
        assert!(
            failure.outcome_uncertain(),
            "the append returned a durable commit"
        );
        assert!(
            !failure.retryable(),
            "replaying could duplicate the committed post"
        );
        assert_eq!(
            session
                .scripted
                .recorded_calls()
                .into_iter()
                .filter(|call| call.method == "append")
                .count(),
            before + 1,
        );
        session.harness.shutdown().await.unwrap();
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

    #[test]
    fn partial_deferred_replay_notifies_after_a_later_journal_failure() {
        use teamarness_package_sdk::capabilities::record_log::{OrderKey, RecordId};

        let directory = tempfile::tempdir().unwrap();
        let store = ConversationStore::open(directory.path().join("partial.sqlite")).unwrap();
        let (changes, mut observer) = watch::channel(0);
        let sink = ConversationsSink {
            store: store.clone(),
            changes,
            sync: Arc::new(Mutex::new(SyncReport::catching_up())),
        };
        let event = |seed: u64, record: &DomainRecord| RecordEvent {
            cursor: Cursor::parse(hex(50, seed)).unwrap(),
            record_id: RecordId::parse(hex(64, seed)).unwrap(),
            order_key: OrderKey::parse(hex(82, seed)).unwrap(),
            author: ALICE.to_owned(),
            purpose: record.kind().to_owned(),
            signature: "signature".to_owned(),
            canonical: record.encode(),
            sealed_record: b"sealed".to_vec(),
        };
        for seed in 1..=2 {
            let message =
                DomainRecord::message_post(CHANNEL, ALICE, format!("deferred {seed}")).unwrap();
            sink.ingest(&event(seed, &message)).unwrap();
        }
        observer.borrow_and_update();
        store
            .inner
            .lock()
            .unwrap()
            .conn
            .execute_batch(
                "CREATE TRIGGER reject_second_history BEFORE INSERT ON history_changes \
             WHEN (SELECT COUNT(*) FROM history_changes) = 1 \
             BEGIN SELECT RAISE(ABORT, 'second deferred write failed'); END;",
            )
            .unwrap();
        let channel = DomainRecord::channel_create_with_id(CHANNEL, ALICE, "team").unwrap();
        let channel_event = event(3, &channel);
        assert!(sink.ingest(&channel_event).is_err());
        assert_eq!(store.history_head().unwrap().1, 1);
        assert_eq!(
            store
                .message_rows(&hex_id(CHANNEL), None, PAGE_SIZE)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(store.deferred_records().unwrap().len(), 1);
        assert_eq!(
            store.cursor().unwrap().as_deref(),
            Some(hex(50, 2).as_str())
        );
        assert!(
            observer.has_changed().unwrap(),
            "a committed projection must wake subscribers"
        );
        assert_eq!(*observer.borrow_and_update(), 4); // Two deferred ingests, Channel, first replay.
        store
            .inner
            .lock()
            .unwrap()
            .conn
            .execute_batch("DROP TRIGGER reject_second_history;")
            .unwrap();
        sink.ingest(&channel_event).unwrap(); // The resumed Channel must finish deferred replay.
        assert_eq!(store.history_head().unwrap().1, 2);
        assert_eq!(store.deferred_records().unwrap().len(), 0);
        assert_eq!(
            store.cursor().unwrap().as_deref(),
            Some(hex(50, 3).as_str())
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn failed_local_channel_append_announces_its_committed_deferred_message() {
        let directory = tempfile::tempdir().unwrap();
        let store_slot = Arc::new(Mutex::new(None::<ConversationStore>));
        let slot = store_slot.clone();
        let scripted = ScriptedCapability::new().calls(move |method, payload| match method {
            "open" => Ok(
                json!({ "results": (0..payload["sealedRecords"].as_array().unwrap().len())
                .map(|_| json!({ "status": "waiting_for_history_access" })).collect::<Vec<_>>() }),
            ),
            "append" => {
                let canonical = URL_SAFE_NO_PAD
                    .decode(payload["canonical"].as_str().unwrap())
                    .unwrap();
                let DomainRecord::ChannelCreate { id, .. } =
                    DomainRecord::decode(&canonical).unwrap()
                else {
                    panic!("expected a Channel creation");
                };
                let store = slot.lock().unwrap().as_ref().unwrap().clone();
                for seed in 1..=2 {
                    let message =
                        DomainRecord::message_post(id, ALICE, format!("deferred {seed}")).unwrap();
                    store
                        .ingest(
                            &hex(64, seed),
                            &hex(82, seed),
                            ALICE,
                            message.kind(),
                            "signature",
                            &message.encode(),
                            b"sealed",
                            None,
                        )
                        .unwrap();
                }
                store
                    .inner
                    .lock()
                    .unwrap()
                    .conn
                    .execute_batch(
                        "CREATE TRIGGER reject_second_history BEFORE INSERT ON history_changes \
                     WHEN (SELECT COUNT(*) FROM history_changes) = 1 \
                     BEGIN SELECT RAISE(ABORT, 'second deferred write failed'); END;",
                    )
                    .unwrap();
                Ok(
                    json!({ "recordId": hex(64, 3), "orderKey": hex(82, 3), "author": ALICE,
                    "signature": "c2ln", "sealedRecord": "c2VhbGVk", "durability": "committed" }),
                )
            }
            _ => Err(BackendError::new(
                "capability_unavailable",
                false,
                json!({}),
            )),
        });
        let mut session = start(directory.path(), scripted).await;
        *store_slot.lock().unwrap() = Some(session.store());
        let mut changes = session
            .harness
            .subscribe("conversations.changes", empty_payload())
            .await
            .unwrap();
        changes.next_event().await.unwrap().unwrap(); // Initial snapshot, before any local commit.
        assert!(
            session
                .harness
                .request("conversations.channel.create", json!({ "slug": "team" }))
                .await
                .is_err()
        );
        assert_eq!(session.store().history_head().unwrap().1, 1);
        let snapshot =
            tokio::time::timeout(std::time::Duration::from_secs(2), changes.next_event())
                .await
                .expect("a committed deferred Message must wake subscribers")
                .unwrap()
                .unwrap();
        assert_eq!(snapshot["channels"].as_array().unwrap().len(), 2);
        changes.close().await;
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn partially_replayed_channel_resumes_from_its_predecessor_cursor() {
        let directory = tempfile::tempdir().unwrap();
        let session = start(directory.path(), scripted_log()).await;
        let store = session.store();
        wait_for_opens(&session.scripted, 1).await;
        let first = DomainRecord::message_post(CHANNEL, ALICE, "first").unwrap();
        let second = DomainRecord::message_post(CHANNEL, ALICE, "second").unwrap();
        let channel = DomainRecord::channel_create_with_id(CHANNEL, ALICE, "team").unwrap();
        let sender = session.scripted.stream();
        for (seed, message) in [(1, &first), (2, &second)] {
            assert!(
                sender
                    .send(record_frame(seed, ALICE, "message.post", &message.encode()))
                    .await
            );
        }
        wait_until(
            || store.record_count().unwrap_or(0) == 2,
            "deferred Messages",
        )
        .await;
        store
            .inner
            .lock()
            .unwrap()
            .conn
            .execute_batch(
                "CREATE TRIGGER reject_second_history BEFORE INSERT ON history_changes \
             WHEN (SELECT COUNT(*) FROM history_changes) = 1 \
             BEGIN SELECT RAISE(ABORT, 'second deferred write failed'); END;",
            )
            .unwrap();
        assert!(
            sender
                .send(record_frame(3, ALICE, "channel.create", &channel.encode()))
                .await
        );
        wait_for_opens(&session.scripted, 2).await;
        assert_eq!(
            session.scripted.recorded_opens()[1]["afterCursor"],
            hex(50, 2)
        );
        assert_eq!(store.history_head().unwrap().1, 1);
        store
            .inner
            .lock()
            .unwrap()
            .conn
            .execute_batch("DROP TRIGGER reject_second_history;")
            .unwrap();
        assert!(
            session
                .scripted
                .stream()
                .send(record_frame(3, ALICE, "channel.create", &channel.encode()))
                .await
        );
        wait_until(
            || store.history_head().unwrap().1 == 2,
            "remaining deferred Message",
        )
        .await;
        assert_eq!(store.deferred_records().unwrap().len(), 0);
        assert_eq!(
            store.cursor().unwrap().as_deref(),
            Some(hex(50, 3).as_str())
        );
        session.harness.shutdown().await.unwrap();
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

    // The scripted open runs on the capability worker. The barrier is entered only after
    // the Router has selected a sealed row; commits on the test worker remain independent.
    fn paused_history_log(
        armed: Arc<std::sync::atomic::AtomicBool>,
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    ) -> ScriptedCapability {
        let release = Mutex::new(release);
        ScriptedCapability::new().calls(move |method, payload| match method {
            "open" => {
                let results = payload["sealedRecords"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| {
                        let marker = URL_SAFE_NO_PAD.decode(item.as_str().unwrap()).unwrap();
                        let seed: u64 = std::str::from_utf8(&marker).unwrap().parse().unwrap();
                        if armed.swap(false, std::sync::atomic::Ordering::SeqCst) {
                            entered.send(()).unwrap();
                            release
                                .lock()
                                .unwrap()
                                .recv_timeout(Duration::from_secs(5))
                                .unwrap();
                        }
                        let record =
                            DomainRecord::message_post(GENERAL_ID, ALICE, format!("row {seed}"))
                                .unwrap();
                        json!({ "status": "ready", "recordId": hex(64, seed),
                        "orderKey": hex(82, seed), "author": ALICE, "purpose": "message.post",
                        "signature": "c2ln", "canonical": URL_SAFE_NO_PAD.encode(record.encode()) })
                    })
                    .collect::<Vec<_>>();
                Ok(json!({ "results": results }))
            }
            _ => Err(BackendError::new(
                "capability_unavailable",
                false,
                json!({}),
            )),
        })
    }

    fn commit_history_row(store: &ConversationStore, seed: u64) -> String {
        let record = DomainRecord::message_post(GENERAL_ID, ALICE, format!("row {seed}")).unwrap();
        store
            .ingest(
                &hex(64, seed),
                &hex(82, seed),
                ALICE,
                record.kind(),
                "c2ln",
                &record.encode(),
                seed.to_string().as_bytes(),
                None,
            )
            .unwrap();
        hex_id(record.id())
    }

    async fn release_after_commit(
        entered: std::sync::mpsc::Receiver<()>,
        release: std::sync::mpsc::Sender<()>,
        store: ConversationStore,
        seed: u64,
    ) -> String {
        tokio::task::spawn_blocking(move || {
            entered
                .recv_timeout(Duration::from_secs(5))
                .expect("capability open reached barrier");
            let id = commit_history_row(&store, seed);
            release.send(()).unwrap();
            id
        })
        .await
        .unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn router_open_replays_commit_after_raw_row_selection() {
        let directory = tempfile::tempdir().unwrap();
        let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let mut session = start(
            directory.path(),
            paused_history_log(armed.clone(), entered_tx, release_rx),
        )
        .await;
        let store = session.store();
        let first_id = commit_history_row(&store, 1);
        armed.store(true, std::sync::atomic::Ordering::SeqCst);
        let (head, second_id) = tokio::join!(
            session.harness.request(
                "conversations.channel.open",
                json!({
                "channelId": hex_id(GENERAL_ID), "limit": 1 })
            ),
            release_after_commit(entered_rx, release_tx, store, 2)
        );
        let head = head.unwrap();
        assert_eq!(head["messages"][0]["id"], first_id);
        assert_eq!(
            head["revision"], "1",
            "open must capture high water with its raw rows"
        );
        assert_eq!(head["nextBefore"], hex(82, 1));
        let delta = session
            .harness
            .request(
                "conversations.channel.changes",
                json!({
            "channelId": hex_id(GENERAL_ID), "epoch": head["epoch"],
            "after": head["revision"], "limit": 1 }),
            )
            .await
            .unwrap();
        assert_eq!(delta["events"].as_array().unwrap().len(), 1);
        assert_eq!(delta["events"][0]["id"], second_id);
        assert_eq!(delta["events"][0]["message"]["markdown"], "row 2");
        assert_eq!(delta["continuation"], "2");
        assert_eq!(delta["highWater"], "2");
        let terminal = session
            .harness
            .request(
                "conversations.channel.changes",
                json!({
            "channelId": hex_id(GENERAL_ID), "epoch": head["epoch"],
            "after": delta["continuation"], "limit": 1 }),
            )
            .await
            .unwrap();
        assert_eq!(terminal["events"], json!([]));
        assert_eq!(terminal["continuation"], "2");
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn router_delta_replays_commit_during_in_flight_batch() {
        let directory = tempfile::tempdir().unwrap();
        let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let mut session = start(
            directory.path(),
            paused_history_log(armed.clone(), entered_tx, release_rx),
        )
        .await;
        let store = session.store();
        let ids = (1..=3)
            .map(|seed| commit_history_row(&store, seed))
            .collect::<Vec<_>>();
        let head = session
            .harness
            .request(
                "conversations.channel.open",
                json!({
            "channelId": hex_id(GENERAL_ID), "limit": 1 }),
            )
            .await
            .unwrap();
        assert_eq!(head["revision"], "3");
        armed.store(true, std::sync::atomic::Ordering::SeqCst);
        let (batch, last_id) = tokio::join!(
            session.harness.request(
                "conversations.channel.changes",
                json!({
                "channelId": hex_id(GENERAL_ID), "epoch": head["epoch"],
                "after": "0", "limit": 1 })
            ),
            release_after_commit(entered_rx, release_tx, store, 4)
        );
        let batch = batch.unwrap();
        assert_eq!(batch["events"].as_array().unwrap().len(), 1);
        assert_eq!(batch["events"][0]["id"], ids[0]);
        assert_eq!(batch["continuation"], "1");
        assert_eq!(
            batch["highWater"], "3",
            "batch must retain selected high water"
        );
        let mut after = batch["continuation"].clone();
        let mut seen = vec![batch["events"][0]["id"].as_str().unwrap().to_owned()];
        for _ in 0..3 {
            let reply = session
                .harness
                .request(
                    "conversations.channel.changes",
                    json!({
                "channelId": hex_id(GENERAL_ID), "epoch": head["epoch"],
                "after": after, "limit": 1 }),
                )
                .await
                .unwrap();
            assert!(reply["events"].as_array().unwrap().len() <= 1);
            seen.extend(
                reply["events"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|event| event["id"].as_str().unwrap().to_owned()),
            );
            after = reply["continuation"].clone();
        }
        assert_eq!(
            seen,
            [ids[0].clone(), ids[1].clone(), ids[2].clone(), last_id]
        );
        assert_eq!(after, "4");
        session.harness.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn router_pages_and_resolves_every_open_status_with_raw_cursor() {
        let directory = tempfile::tempdir().unwrap();
        let ready = DomainRecord::message_post(GENERAL_ID, ALICE, "readable").unwrap();
        let ready_bytes = URL_SAFE_NO_PAD.encode(ready.encode());
        let fail_open = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let failing = fail_open.clone();
        let scripted = ScriptedCapability::new().calls(move |method, payload| match method {
            "open" => {
                if failing.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    return Err(BackendError::new("capability_unavailable", true, json!({})));
                }
                let results = payload["sealedRecords"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| {
                        let marker = URL_SAFE_NO_PAD.decode(item.as_str().unwrap()).unwrap();
                        match marker.as_slice() {
                            b"ready" => json!({ "status": "ready", "recordId": hex(64, 1),
                            "orderKey": hex(82, 1), "author": ALICE, "purpose": "message.post",
                            "signature": "c2ln", "canonical": ready_bytes }),
                            b"waiting" => json!({ "status": "waiting_for_history_access" }),
                            b"wrong" => json!({ "status": "wrong_scope" }),
                            b"corrupt" => json!({ "status": "corrupt" }),
                            _ => panic!("unexpected sealed fixture"),
                        }
                    })
                    .collect::<Vec<_>>();
                Ok(json!({ "results": results }))
            }
            _ => Err(BackendError::new(
                "capability_unavailable",
                false,
                json!({}),
            )),
        });
        let mut session = start(directory.path(), scripted).await;
        let ids = [
            ready.id(),
            DomainRecord::message_post(GENERAL_ID, ALICE, "waiting")
                .unwrap()
                .id(),
            DomainRecord::message_post(GENERAL_ID, ALICE, "wrong")
                .unwrap()
                .id(),
            DomainRecord::message_post(GENERAL_ID, ALICE, "corrupt")
                .unwrap()
                .id(),
        ];
        for (index, marker) in ["ready", "waiting", "wrong", "corrupt"].iter().enumerate() {
            let row = DomainRecord::MessagePost {
                id: ids[index],
                channel_id: GENERAL_ID,
                author: ALICE.into(),
                markdown: (*marker).into(),
            };
            session
                .store()
                .ingest(
                    &hex(64, index as u64 + 1),
                    &hex(82, index as u64 + 1),
                    ALICE,
                    row.kind(),
                    "c2ln",
                    &row.encode(),
                    marker.as_bytes(),
                    None,
                )
                .unwrap();
        }
        let channel_id = hex_id(GENERAL_ID);
        let head = session
            .harness
            .request(
                "conversations.channel.open",
                json!({ "channelId": channel_id, "limit": 2 }),
            )
            .await
            .unwrap();
        assert_eq!(head["messages"].as_array().unwrap().len(), 0);
        assert_eq!(head["pending"].as_array().unwrap().len(), 2);
        assert!(
            head["pending"]
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row["status"] == "unavailable")
        );
        assert_eq!(head["nextBefore"], hex(82, 3));
        let page = session
            .harness
            .request(
                "conversations.channel.page",
                json!({ "channelId": channel_id, "before": head["nextBefore"], "limit": 2 }),
            )
            .await
            .unwrap();
        assert_eq!(page["messages"][0]["id"], hex_id(ids[0]));
        assert_eq!(page["pending"][0]["status"], "pending");
        assert_eq!(page["nextBefore"], hex(82, 1));
        let terminal = session
            .harness
            .request(
                "conversations.channel.page",
                json!({ "channelId": channel_id, "before": page["nextBefore"], "limit": 2 }),
            )
            .await
            .unwrap();
        assert!(terminal.get("nextBefore").is_none());
        let resolved = session.harness.request("conversations.channel.resolve", json!({
            "channelId": channel_id, "ids": ids.iter().map(|id| hex_id(*id)).chain([hex_id([9; 16])]).collect::<Vec<_>>(),
        })).await.unwrap();
        assert_eq!(resolved["messages"].as_array().unwrap().len(), 1);
        assert_eq!(resolved["pending"].as_array().unwrap().len(), 3);
        assert_eq!(resolved["missing"], json!([hex_id([9; 16])]));
        let delta = session
            .harness
            .request(
                "conversations.channel.changes",
                json!({
                    "channelId": channel_id, "epoch": head["epoch"], "after": "0", "limit": 2,
                }),
            )
            .await
            .unwrap();
        assert_eq!(delta["events"].as_array().unwrap().len(), 2);
        assert_eq!(delta["events"][0]["status"], "ready");
        assert_eq!(delta["events"][1]["status"], "pending");
        assert_eq!(delta["continuation"], "2");
        assert_eq!(delta["highWater"], "4");
        let port = session
            .harness
            .request(
                "teamarness.port.query.v1",
                json!({
                    "contract":"dev.teamarness.conversations", "major":1, "minor":2,
                    "port":"message.changes", "actor":{"kind":"person","identity":ALICE},
                    "request":{"channelId":channel_id,"epoch":head["epoch"],"after":"0","limit":2}
                }),
            )
            .await
            .unwrap();
        assert_eq!(port["$teamarnessPortValue"], delta);
        let unavailable = session
            .harness
            .request(
                "teamarness.port.query.v1",
                json!({
                    "contract":"dev.teamarness.conversations", "major":1, "minor":2,
                    "port":"message.changes", "actor":{"kind":"person","identity":ALICE},
                    "request":{"channelId":channel_id,"epoch":head["epoch"],"after":"2","limit":2}
                }),
            )
            .await
            .unwrap();
        assert!(
            unavailable["$teamarnessPortValue"]["events"]
                .as_array()
                .unwrap()
                .iter()
                .all(|event| event["status"] == "unavailable")
        );
        let stale = session
            .harness
            .request(
                "conversations.channel.changes",
                json!({
                    "channelId": channel_id, "epoch": "0".repeat(32), "after": "0", "limit": 2,
                }),
            )
            .await
            .unwrap();
        assert_eq!(stale["resyncRequired"], true);
        fail_open.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(
            session
                .harness
                .request(
                    "conversations.channel.page",
                    json!({
                        "channelId": channel_id, "before": head["nextBefore"], "limit": 2,
                    })
                )
                .await
                .is_err()
        );
        assert!(
            session
                .harness
                .request(
                    "conversations.channel.page",
                    json!({
                        "channelId": channel_id, "before": head["nextBefore"], "limit": 2,
                    })
                )
                .await
                .is_ok()
        );
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
    fn large_principal_snapshot_reaches_every_author_in_bounded_parts() {
        use teamarness_package_sdk::capabilities::principals::{Principal, PrincipalKind};
        let snapshot = PrincipalSnapshot {
            workspace_id: "a".repeat(32),
            cursor: 1,
            principals: (0..1024)
                .map(|n| Principal {
                    principal_id: format!("{n:064x}"),
                    kind: PrincipalKind::Person,
                    steward_principal_id: None,
                    keys: vec![format!("{n:064x}")],
                    name: Some("🌻".repeat(64)),
                    fingerprint: "a".repeat(12),
                })
                .collect(),
        };
        let events = principal_snapshots(snapshot);
        assert!(events.len() > 1);
        assert_eq!(
            events
                .iter()
                .map(|event| event["principals"].as_array().unwrap().len())
                .sum::<usize>(),
            1024
        );
        for (index, event) in events.iter().enumerate() {
            assert_eq!(event["cursor"], "1");
            assert_eq!(event["part"], index);
            assert_eq!(event["parts"], events.len());
            assert!(serde_json::to_vec(event).unwrap().len() <= HISTORY_REPLY_BUDGET);
        }
        let escaping = PrincipalSnapshot {
            workspace_id: "a".repeat(32),
            cursor: 2,
            principals: (0..1024)
                .map(|n| Principal {
                    principal_id: format!("{n:064x}"),
                    kind: PrincipalKind::Agent,
                    steward_principal_id: Some("c".repeat(64)),
                    keys: vec![format!("{n:064x}")],
                    name: Some("\\\"".repeat(128)),
                    fingerprint: "f".repeat(64),
                })
                .collect(),
        };
        let renamed = principal_snapshots(escaping);
        assert_eq!(
            renamed
                .iter()
                .map(|part| part["principals"].as_array().unwrap().len())
                .sum::<usize>(),
            1024
        );
        assert!(renamed.iter().all(|part| part["cursor"] == "2"
            && serde_json::to_vec(part).unwrap().len() <= HISTORY_REPLY_BUDGET));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn principal_changes_relabel_without_mutating_signed_message_author() {
        let directory = tempfile::tempdir().unwrap();
        let principals = ScriptedCapability::new().calls(|method, _| match method {
            "current" => Ok(
                json!({ "workspaceId": "a".repeat(32), "cursor": "0", "principals": [
                { "principalId": "c".repeat(64), "kind": "person", "stewardPrincipalId": null,
                  "keys": ["d".repeat(64)], "name": "Alice", "fingerprint": "dddddddddddd" },
                { "principalId": "e".repeat(64), "kind": "agent", "stewardPrincipalId": "c".repeat(64),
                  "keys": ["b".repeat(64)], "name": "Helper", "fingerprint": "bbbbbbbbbbbb" }
            ] }),
            ),
            _ => Err(BackendError::new(
                "capability_unavailable",
                false,
                json!({}),
            )),
        });
        let observer: ActivationObserver = Arc::new(Mutex::new(None));
        let mut harness =
            HarnessBuilder::new(router_observing(Some(observer.clone())).into_backend())
                .package_identity("dev.teamarness.conversations")
                .installation(ALICE, "Alice")
                .data_directory(directory.path())
                .capability(RECORD_LOG, scripted_log())
                .capability("teamarness.principals", principals.clone())
                .start()
                .await;
        harness.activate().await.unwrap();
        let store = observer.lock().unwrap().clone().unwrap().store;
        let signed_author = "b".repeat(64);
        let message =
            DomainRecord::message_post(GENERAL_ID, &signed_author, "before rename").unwrap();
        store
            .ingest(
                &hex(64, 1),
                &hex(82, 1),
                &signed_author,
                message.kind(),
                "sig",
                &message.encode(),
                b"sealed",
                None,
            )
            .unwrap();
        let mut changes = harness
            .subscribe("conversations.principals", json!({}))
            .await
            .unwrap();
        let first = changes.next_event().await.unwrap().unwrap();
        wait_for_opens(&principals, 1).await;
        assert_eq!(first["principals"][0]["name"], "Alice");
        assert_eq!(first["principals"][1]["identity"], signed_author);
        assert_eq!(first["principals"][1]["kind"], "agent");
        assert_eq!(first["principals"][1]["stewardPrincipalId"], "c".repeat(64));
        assert_eq!(
            store
                .message_rows(&hex_id(GENERAL_ID), None, PAGE_SIZE)
                .unwrap()[0]
                .2,
            signed_author
        );
        principals
            .stream()
            .send(
                json!({"workspaceId": "a".repeat(32), "cursor": "1", "principals": [
                    { "principalId": "c".repeat(64), "kind": "person", "stewardPrincipalId": null,
                      "keys": ["d".repeat(64)], "name": "Alice 🌻", "fingerprint": "dddddddddddd" },
                    { "principalId": "e".repeat(64), "kind": "agent", "stewardPrincipalId": "c".repeat(64),
                      "keys": [signed_author], "name": "Helper renamed", "fingerprint": "bbbbbbbbbbbb" }
                ]}),
            )
            .await;
        let renamed = tokio::time::timeout(Duration::from_secs(2), changes.next_event())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(renamed["principals"][0]["name"], "Alice 🌻");
        assert_eq!(renamed["principals"][1]["name"], "Helper renamed");
        assert_eq!(
            store
                .message_rows(&hex_id(GENERAL_ID), None, PAGE_SIZE)
                .unwrap()[0]
                .2,
            signed_author
        );
        changes.close().await;
        harness.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn closing_idle_principals_subscription_closes_capability_stream() {
        let directory = tempfile::tempdir().unwrap();
        let principals = ScriptedCapability::new().calls(|method, _| match method {
            "current" => {
                Ok(json!({ "workspaceId": "a".repeat(32), "cursor": "0", "principals": [] }))
            }
            _ => Err(BackendError::new(
                "capability_unavailable",
                false,
                json!({}),
            )),
        });
        let mut harness = HarnessBuilder::new(router().into_backend())
            .package_identity("dev.teamarness.conversations")
            .installation(ALICE, "Alice")
            .data_directory(directory.path())
            .capability(RECORD_LOG, scripted_log())
            .capability("teamarness.principals", principals.clone())
            .start()
            .await;
        harness.activate().await.unwrap();
        let mut subscription = harness
            .subscribe("conversations.principals", json!({}))
            .await
            .unwrap();
        assert_eq!(
            subscription.next_event().await.unwrap().unwrap()["cursor"],
            "0"
        );
        let stream = principals.stream();
        tokio::time::timeout(Duration::from_secs(2), stream.opened())
            .await
            .unwrap();
        subscription.close().await;
        tokio::time::timeout(
            Duration::from_secs(2),
            wait_until(|| !stream.is_open(), "idle principals stream closure"),
        )
        .await
        .unwrap();
        harness.shutdown().await.unwrap();
    }

    // Verifies: AC13 (better-doc-agent) — a Channel's draft round-trips through the Backend's
    // memory, an empty draft forgets it, at most 64 Channels are kept, and nothing survives a
    // Backend restart.
    #[tokio::test]
    async fn channel_drafts_round_trip_in_bounded_memory_only() {
        let directory = tempfile::tempdir().unwrap();
        let contract = teamarness_package_contract::ui_contract::compile_ui_contract(
            include_bytes!("../../ui-contract.json"),
        )
        .unwrap();
        let mut session = start(directory.path(), scripted_log()).await;
        let channel = |seed: u64| hex(32, seed);
        let get = |id: String| json!({ "channelId": id });
        let empty = session
            .harness
            .request("conversations.draft.get", get(channel(1)))
            .await
            .unwrap();
        contract
            .validate_response("conversations.draft.get", &empty)
            .unwrap();
        assert_eq!(empty, json!({ "text": "" }));

        let set = session
            .harness
            .request(
                "conversations.draft.set",
                json!({ "channelId": channel(1), "text": "half a *thought*" }),
            )
            .await
            .unwrap();
        contract
            .validate_response("conversations.draft.set", &set)
            .unwrap();
        session
            .harness
            .request(
                "conversations.draft.set",
                json!({ "channelId": channel(2), "text": "another" }),
            )
            .await
            .unwrap();
        let read = session
            .harness
            .request("conversations.draft.get", get(channel(1)))
            .await
            .unwrap();
        contract
            .validate_response("conversations.draft.get", &read)
            .unwrap();
        assert_eq!(read, json!({ "text": "half a *thought*" }));
        assert_eq!(
            session
                .harness
                .request("conversations.draft.get", get(channel(2)))
                .await
                .unwrap(),
            json!({ "text": "another" })
        );

        // An empty draft forgets the Channel's entry.
        session
            .harness
            .request(
                "conversations.draft.set",
                json!({ "channelId": channel(1), "text": "" }),
            )
            .await
            .unwrap();
        assert_eq!(
            session
                .harness
                .request("conversations.draft.get", get(channel(1)))
                .await
                .unwrap(),
            json!({ "text": "" })
        );

        // A draft over 32 KiB is refused.
        let refused = session
            .harness
            .request(
                "conversations.draft.set",
                json!({ "channelId": channel(3), "text": "é".repeat(16 * 1024 + 1) }),
            )
            .await
            .unwrap_err();
        assert_eq!(refused.code(), "conversations.invalid_request");

        // Channel 2 is the oldest; 64 more Channels push it out, and the newest 64 stay.
        for seed in 100..164 {
            session
                .harness
                .request(
                    "conversations.draft.set",
                    json!({ "channelId": channel(seed), "text": format!("draft {seed}") }),
                )
                .await
                .unwrap();
        }
        assert_eq!(
            session
                .harness
                .request("conversations.draft.get", get(channel(2)))
                .await
                .unwrap(),
            json!({ "text": "" })
        );
        for seed in [100, 163] {
            assert_eq!(
                session
                    .harness
                    .request("conversations.draft.get", get(channel(seed)))
                    .await
                    .unwrap(),
                json!({ "text": format!("draft {seed}") })
            );
        }
        // Saving again makes a Channel the newest, so the next new Channel drops the next oldest.
        session
            .harness
            .request(
                "conversations.draft.set",
                json!({ "channelId": channel(100), "text": "draft 100 again" }),
            )
            .await
            .unwrap();
        session
            .harness
            .request(
                "conversations.draft.set",
                json!({ "channelId": channel(200), "text": "newest" }),
            )
            .await
            .unwrap();
        assert_eq!(
            session
                .harness
                .request("conversations.draft.get", get(channel(100)))
                .await
                .unwrap(),
            json!({ "text": "draft 100 again" })
        );
        assert_eq!(
            session
                .harness
                .request("conversations.draft.get", get(channel(101)))
                .await
                .unwrap(),
            json!({ "text": "" })
        );
        session.harness.shutdown().await.unwrap();

        // A new Backend start remembers nothing.
        let mut restarted = start(directory.path(), scripted_log()).await;
        assert_eq!(
            restarted
                .harness
                .request("conversations.draft.get", get(channel(200)))
                .await
                .unwrap(),
            json!({ "text": "" })
        );
        restarted.harness.shutdown().await.unwrap();
    }

    #[test]
    fn router_matches_the_ui_contract() {
        assert_router_matches_contract(
            &router().operations(),
            include_str!("../../ui-contract.json"),
        );
    }
}
