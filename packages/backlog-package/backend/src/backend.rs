//! The Backlog Backend runtime: activation, the record log session, UI requests and
//! subscriptions.
//!
//! Every change goes through one [`append_as`] path. A Ticket-changing request holds that
//! Ticket's lock while it compares `baseVersion` with the version this Installation has seen,
//! so a stale save is refused rather than silently overwriting a newer change (ADR 0039).
//! Remembered UI state lives only in this activation's memory (ADR 0027).

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{Value as JsonValue, json};
use teamarness_package_sdk::capabilities::principals::{
    PrincipalKind, PrincipalSnapshot, Principals,
};
use teamarness_package_sdk::capabilities::record_log::{
    Availability, Cursor, OpenResult, RecordEvent, RecordLog, RecordLogSink, SessionOptions,
    SyncState, SyncWarning, run_session,
};
use teamarness_package_sdk::ports::PortActor;
use teamarness_package_sdk::{
    BackendError, CancellationToken, Context, Empty, Router, event_channel,
};
use tokio::sync::watch;

use crate::domain::{
    DEFAULT_PRIORITY, DEFAULT_STATUS, DomainError, DomainRecord, Field, Priority, Status,
    TicketChanges,
};
use crate::store::{
    BacklogStore, Incoming, IngestResult, SealedRow, StoreError, TicketLocks, TicketView,
};

#[path = "guide.rs"]
mod guide;
#[path = "ports.rs"]
mod ports;

/// How many distinct synchronization warning codes are remembered at a time.
const MAX_WARNINGS: usize = 8;
/// The largest remembered UI state one Instance holds, in compact JSON bytes.
pub(crate) const REMEMBERED_STATE_LIMIT: usize = 256 * 1024;
/// The Host forwards Package error details only up to this many compact JSON bytes.
const ERROR_DETAILS_LIMIT: usize = 4096;
/// Shown for an Epic or Ticket whose title record has not been opened yet.
const UNTITLED: &str = "Untitled";
/// The warning shown when a synchronized record could not be applied.
const PROJECTION_WARNING: &str = "Some synchronized records could not be applied.";

fn error(code: &str, retryable: bool, details: JsonValue) -> BackendError {
    BackendError::new(code, retryable, details)
}

fn unavailable() -> BackendError {
    error("backlog.unavailable", true, json!({}))
}

fn storage_failed(_error: StoreError) -> BackendError {
    unavailable()
}

fn not_found(id: &str) -> BackendError {
    error("backlog.not_found", false, json!({ "id": id }))
}

fn invalid(field: &str, reason: &str) -> BackendError {
    error(
        "backlog.invalid",
        false,
        json!({ "field": field, "reason": reason }),
    )
}

fn capability_error(operation: &str, cause: BackendError) -> BackendError {
    eprintln!(
        "backlog capability failure: operation={operation} code={} retryable={}",
        cause.code(),
        cause.retryable()
    );
    error(
        "backlog.unavailable",
        cause.retryable(),
        json!({ "causeCode": cause.code() }),
    )
    .with_outcome_uncertain(cause.outcome_uncertain())
}

/// Turns a record validation failure into `backlog.invalid {field, reason}` or
/// `backlog.too_large {field, limit}`.
pub(crate) fn domain_error(failure: &DomainError) -> BackendError {
    match failure {
        DomainError::TooLarge(field) => error(
            "backlog.too_large",
            false,
            json!({ "field": field.as_str(), "limit": failure.limit() }),
        ),
        DomainError::Invalid(Field::Changes) => invalid(Field::Changes.as_str(), "empty"),
        DomainError::Invalid(field) => invalid(field.as_str(), "invalid"),
        DomainError::InvalidKind | DomainError::InvalidLength | DomainError::InvalidUtf8 => {
            invalid("record", "invalid")
        }
    }
}

/// The author-clock time in Unix milliseconds, for display only.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// What the Package tells the UI about synchronization.
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

/// Everything one activation of the Backlog owns.
pub struct BacklogActive {
    store: BacklogStore,
    log: RecordLog,
    principals: Principals,
    sync: Arc<Mutex<SyncReport>>,
    changes: watch::Sender<u64>,
    /// The Installation every UI request is signed as.
    installation: String,
    locks: TicketLocks,
    /// The UI's remembered state: drafts, selection and filters. Never persisted.
    remembered: Mutex<Option<JsonValue>>,
    session: Mutex<Option<Session>>,
}

impl BacklogActive {
    pub fn store(&self) -> &BacklogStore {
        &self.store
    }

    fn report(&self) -> SyncReport {
        self.sync
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn notify(&self) {
        notify(&self.changes);
    }

    /// The `backlog.changes` snapshot: availability, warnings, every Epic and every Ticket
    /// summary. A title whose record has not been opened yet is shown as `Untitled`.
    fn snapshot_value(&self) -> Result<JsonValue, BackendError> {
        let report = self.report();
        let epics: Vec<JsonValue> = self
            .store
            .epics()
            .map_err(storage_failed)?
            .into_iter()
            .map(|epic| {
                json!({
                    "epicId": epic.id,
                    "title": epic.title.unwrap_or_else(|| UNTITLED.into()),
                    "archived": epic.archived,
                    "createdOrder": epic.create_order,
                })
            })
            .collect();
        let tickets: Vec<JsonValue> = self
            .store
            .tickets()
            .map_err(storage_failed)?
            .into_iter()
            .map(|ticket| {
                json!({
                    "ticketId": ticket.id,
                    "epicId": ticket.epic_id,
                    "title": ticket.title.unwrap_or_else(|| UNTITLED.into()),
                    "status": ticket.status.as_str(),
                    "priority": ticket.priority.as_str(),
                    "assignee": ticket.assignee,
                    "archived": ticket.archived,
                    "version": ticket.version,
                    "createdOrder": ticket.create_order,
                })
            })
            .collect();
        let mut warnings = report.warnings;
        if self
            .store
            .has_projection_warnings()
            .map_err(storage_failed)?
        {
            warnings.push(PROJECTION_WARNING.into());
        }
        Ok(json!({
            "availability": report.availability.as_str(),
            "warnings": warnings,
            "epics": epics,
            "tickets": tickets,
        }))
    }
}

fn notify(changes: &watch::Sender<u64>) {
    changes.send_modify(|revision| *revision = revision.saturating_add(1));
}

/// The full Ticket shape `backlog.ticket.get` returns and a stale refusal carries.
pub(crate) fn ticket_value(ticket: &TicketView) -> JsonValue {
    json!({
        "ticketId": ticket.id,
        "epicId": ticket.epic_id,
        "title": ticket.title.clone().unwrap_or_else(|| UNTITLED.into()),
        "body": ticket.body.clone().unwrap_or_default(),
        "status": ticket.status.as_str(),
        "priority": ticket.priority.as_str(),
        "assignee": ticket.assignee,
        "archived": ticket.archived,
        "version": ticket.version,
        "creator": ticket.creator,
        "createdAt": ticket.create_at,
        "lastEditor": ticket.last_editor,
        "lastAt": ticket.last_at,
    })
}

/// `backlog.stale_version {ticket}` with the current Ticket.
///
/// The Host drops error details over 4,096 bytes, so a Ticket whose body would not fit is
/// sent without `body` and with `bodyOmitted: true`; the caller reads it with `ticket.get`.
pub(crate) fn stale_version(ticket: &TicketView) -> BackendError {
    let full = json!({ "ticket": ticket_value(ticket) });
    let fits = serde_json::to_vec(&full).is_ok_and(|bytes| bytes.len() <= ERROR_DETAILS_LIMIT);
    let details = if fits {
        full
    } else {
        let mut summary = ticket_value(ticket);
        if let Some(fields) = summary.as_object_mut() {
            fields.remove("body");
        }
        json!({ "ticket": summary, "bodyOmitted": true })
    };
    error("backlog.stale_version", false, details)
}

// ---------------------------------------------------------------------------
// requests
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EpicCreateRequest {
    title: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EpicRenameRequest {
    epic_id: String,
    title: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EpicRequest {
    epic_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TicketCreateRequest {
    epic_id: String,
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    priority: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TicketUpdateRequest {
    ticket_id: String,
    base_version: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    priority: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TicketAssignRequest {
    ticket_id: String,
    base_version: String,
    assignee: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TicketMoveRequest {
    ticket_id: String,
    base_version: String,
    epic_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TicketBaseRequest {
    ticket_id: String,
    base_version: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TicketRequest {
    ticket_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DraftPutRequest {
    state: JsonValue,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DraftClearRequest {
    key: String,
}

fn parse_status(value: Option<&str>) -> Result<Option<Status>, BackendError> {
    value
        .map(|value| Status::parse(value).map_err(|_| invalid(Field::Status.as_str(), "invalid")))
        .transpose()
}

fn parse_priority(value: Option<&str>) -> Result<Option<Priority>, BackendError> {
    value
        .map(|value| {
            Priority::parse(value).map_err(|_| invalid(Field::Priority.as_str(), "invalid"))
        })
        .transpose()
}

/// Who a record is written as: the signing key the record names and, for an Agent, the
/// Principal Core signs for. UI requests are always the Installation; Port commands are the
/// Host-derived actor (`ports::authorize_actor`).
pub(crate) struct Author {
    identity: String,
    principal: Option<String>,
}

impl Author {
    fn installation(state: &BacklogActive) -> Self {
        Self {
            identity: state.installation.clone(),
            principal: None,
        }
    }
}

/// The Backlog Package Backend.
#[must_use]
pub fn router() -> Router<BacklogActive> {
    Router::new(|activation| {
        let initialization = activation.initialization().clone();
        let capabilities = activation.capabilities().clone();
        let cancellation = activation.cancellation().clone();
        async move {
            let path = initialization
                .data_directory()
                .path()
                .join("backlog.sqlite3");
            let store = teamarness_package_sdk::blocking(&cancellation, move |_| {
                BacklogStore::open(path).map_err(storage_failed)
            })
            .await?;
            let log = RecordLog::from_initialization(&initialization, capabilities.clone())
                .map_err(|cause| capability_error("record-log", cause))?;
            let principals = Principals::from_initialization(&initialization, capabilities)
                .map_err(|cause| capability_error("principals", cause))?;
            rehydrate(&store, &log, &cancellation).await?;
            let sync = Arc::new(Mutex::new(SyncReport::catching_up()));
            let (changes, _) = watch::channel(0);
            let session = start_session(&store, &log, &sync, &changes);
            Ok(BacklogActive {
                store,
                log,
                principals,
                sync,
                changes,
                installation: initialization.installation().identity.clone(),
                locks: TicketLocks::default(),
                remembered: Mutex::new(None),
                session: Mutex::new(Some(session)),
            })
        }
    })
    .on_suspend(|state: Arc<BacklogActive>| async move {
        stop_session(&state).await;
        Ok(())
    })
    .on_resume(|state: Arc<BacklogActive>| async move {
        stop_session(&state).await;
        let session = start_session(&state.store, &state.log, &state.sync, &state.changes);
        *state
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(session);
        Ok(())
    })
    .on_dispose(|state: Arc<BacklogActive>| async move {
        stop_session(&state).await;
        Ok(())
    })
    .request(
        "backlog.epic.create",
        |context: Context<BacklogActive>, request: EpicCreateRequest| async move {
            let author = Author::installation(context.state());
            let record = DomainRecord::epic_create(&author.identity, now_ms(), &request.title)
                .map_err(|failure| domain_error(&failure))?;
            let epic_id = append_as(&context, &author, record).await?;
            Ok(json!({ "epicId": epic_id }))
        },
    )
    .request(
        "backlog.epic.rename",
        |context: Context<BacklogActive>, request: EpicRenameRequest| async move {
            let author = Author::installation(context.state());
            let record = DomainRecord::epic_rename(
                &author.identity,
                now_ms(),
                &request.epic_id,
                &request.title,
            )
            .map_err(|failure| domain_error(&failure))?;
            existing_epic(&context, &request.epic_id).await?;
            append_as(&context, &author, record).await?;
            Ok(json!({}))
        },
    )
    .request(
        "backlog.epic.archive",
        |context: Context<BacklogActive>, request: EpicRequest| async move {
            let author = Author::installation(context.state());
            let record = DomainRecord::epic_archive(&author.identity, now_ms(), &request.epic_id)
                .map_err(|failure| domain_error(&failure))?;
            existing_epic(&context, &request.epic_id).await?;
            append_as(&context, &author, record).await?;
            Ok(json!({}))
        },
    )
    .request(
        "backlog.epic.restore",
        |context: Context<BacklogActive>, request: EpicRequest| async move {
            let author = Author::installation(context.state());
            let record = DomainRecord::epic_restore(&author.identity, now_ms(), &request.epic_id)
                .map_err(|failure| domain_error(&failure))?;
            existing_epic(&context, &request.epic_id).await?;
            append_as(&context, &author, record).await?;
            Ok(json!({}))
        },
    )
    .request(
        "backlog.ticket.create",
        |context: Context<BacklogActive>, request: TicketCreateRequest| async move {
            let author = Author::installation(context.state());
            create_ticket(&context, &author, request).await
        },
    )
    .request(
        "backlog.ticket.update",
        |context: Context<BacklogActive>, request: TicketUpdateRequest| async move {
            let author = Author::installation(context.state());
            update_ticket(&context, &author, request).await
        },
    )
    .request(
        "backlog.ticket.assign",
        |context: Context<BacklogActive>, request: TicketAssignRequest| async move {
            let author = Author::installation(context.state());
            let record = DomainRecord::ticket_assign(
                &author.identity,
                now_ms(),
                &request.ticket_id,
                request.assignee.as_deref(),
            )
            .map_err(|failure| domain_error(&failure))?;
            let _guard = guarded(&context, &request.ticket_id, &request.base_version).await?;
            if let Some(assignee) = &request.assignee {
                check_assignee(&context, assignee).await?;
            }
            append_as(&context, &author, record).await?;
            version_value(&context, &request.ticket_id).await
        },
    )
    .request(
        "backlog.ticket.move",
        |context: Context<BacklogActive>, request: TicketMoveRequest| async move {
            let author = Author::installation(context.state());
            let record = DomainRecord::ticket_move(
                &author.identity,
                now_ms(),
                &request.ticket_id,
                &request.epic_id,
            )
            .map_err(|failure| domain_error(&failure))?;
            let _guard = guarded(&context, &request.ticket_id, &request.base_version).await?;
            open_epic(&context, &request.epic_id).await?;
            append_as(&context, &author, record).await?;
            version_value(&context, &request.ticket_id).await
        },
    )
    .request(
        "backlog.ticket.archive",
        |context: Context<BacklogActive>, request: TicketBaseRequest| async move {
            let author = Author::installation(context.state());
            let record =
                DomainRecord::ticket_archive(&author.identity, now_ms(), &request.ticket_id)
                    .map_err(|failure| domain_error(&failure))?;
            let _guard = guarded(&context, &request.ticket_id, &request.base_version).await?;
            append_as(&context, &author, record).await?;
            version_value(&context, &request.ticket_id).await
        },
    )
    .request(
        "backlog.ticket.restore",
        |context: Context<BacklogActive>, request: TicketBaseRequest| async move {
            let author = Author::installation(context.state());
            let record =
                DomainRecord::ticket_restore(&author.identity, now_ms(), &request.ticket_id)
                    .map_err(|failure| domain_error(&failure))?;
            let _guard = guarded(&context, &request.ticket_id, &request.base_version).await?;
            append_as(&context, &author, record).await?;
            version_value(&context, &request.ticket_id).await
        },
    )
    .request(
        "backlog.ticket.get",
        |context: Context<BacklogActive>, request: TicketRequest| async move {
            let ticket = read_ticket(&context, &request.ticket_id).await?;
            Ok(ticket_value(&ticket))
        },
    )
    .request(
        "backlog.draft.get",
        |context: Context<BacklogActive>, _: Empty| async move {
            let state = context
                .state()
                .remembered
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            Ok(json!({ "state": state }))
        },
    )
    .request(
        "backlog.draft.put",
        |context: Context<BacklogActive>, request: DraftPutRequest| async move {
            if !request.state.is_object() {
                return Err(invalid("state", "invalid"));
            }
            let size = serde_json::to_vec(&request.state).map_or(usize::MAX, |bytes| bytes.len());
            if size > REMEMBERED_STATE_LIMIT {
                return Err(error(
                    "backlog.too_large",
                    false,
                    json!({ "field": "state", "limit": REMEMBERED_STATE_LIMIT }),
                ));
            }
            *context
                .state()
                .remembered
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(request.state);
            Ok(json!({}))
        },
    )
    .request(
        "backlog.draft.clear",
        |context: Context<BacklogActive>, request: DraftClearRequest| async move {
            let mut remembered = context
                .state()
                .remembered
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(drafts) = remembered
                .as_mut()
                .and_then(|state| state.get_mut("drafts"))
                .and_then(JsonValue::as_object_mut)
            {
                drafts.remove(&request.key);
            }
            Ok(json!({}))
        },
    )
    .subscription(
        "backlog.changes",
        |context: Context<BacklogActive>, _: Empty| async move {
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
                    tokio::select! {
                        result = revisions.changed() => if result.is_err() { return; },
                        () = sender.closed() => return,
                    }
                }
            });
            Ok(subscription)
        },
    )
    .subscription(
        "backlog.names",
        |context: Context<BacklogActive>, _: Empty| async move {
            let (sender, subscription) = event_channel(4);
            let principals = context.state().principals.clone();
            tokio::spawn(async move {
                loop {
                    let mut cursor = match principals.current().await {
                        Ok(snapshot) => {
                            let cursor = snapshot.cursor;
                            for event in name_parts(snapshot) {
                                if sender.send(event).await.is_err() {
                                    return;
                                }
                            }
                            cursor
                        }
                        Err(_) => {
                            if sender.send(names_unavailable()).await.is_err() {
                                return;
                            }
                            // negative-wait: backoff before asking an unavailable capability again.
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
                                        for event in name_parts(snapshot) {
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
                    if sender.send(names_unavailable()).await.is_err() {
                        return;
                    }
                    // negative-wait: backoff before resubscribing to the roster.
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            });
            Ok(subscription)
        },
    )
    .port_request(
        ports::CONTRACT, "runtime.guide", "query",
        |_context: Context<BacklogActive>, _actor: PortActor, _: Empty| async move {
            Ok(guide::response())
        },
    )
    .port_request(
        ports::CONTRACT, "ticket.list", "query",
        |context: Context<BacklogActive>, _actor: PortActor, request: ports::ListRequest| async move {
            ports::list(&context, request).await
        },
    )
    .port_request(
        ports::CONTRACT, "ticket.get", "query",
        |context: Context<BacklogActive>, _actor: PortActor, request: TicketRequest| async move {
            let ticket = read_ticket(&context, &request.ticket_id).await?;
            Ok(ticket_value(&ticket))
        },
    )
    .port_request(
        ports::CONTRACT, "ticket.create", "command",
        |context: Context<BacklogActive>, actor: PortActor, request: TicketCreateRequest| async move {
            let author = ports::authorize_actor(&context, actor)?;
            create_ticket(&context, &author, request).await
        },
    )
    .port_request(
        ports::CONTRACT, "ticket.update", "command",
        |context: Context<BacklogActive>, actor: PortActor, request: TicketUpdateRequest| async move {
            let author = ports::authorize_actor(&context, actor)?;
            update_ticket(&context, &author, request).await
        },
    )
    .port_event(
        ports::CONTRACT, "backlog.changed",
        |context: Context<BacklogActive>, _actor: PortActor| async move {
            ports::changed(&context)
        },
    )
}

fn names_unavailable() -> JsonValue {
    json!({ "available": false, "cursor": "0", "principals": [] })
}

/// The roster as `backlog.names` events of at most 64 Principals each.
///
/// The capability bounds the roster to 1,024 Principals, so there are at most 16 parts.
pub(crate) fn name_parts(snapshot: PrincipalSnapshot) -> Vec<JsonValue> {
    const PRINCIPALS_PER_EVENT: usize = 64;
    let principals: Vec<JsonValue> = snapshot
        .principals
        .into_iter()
        .map(|principal| {
            json!({
                "principalId": principal.principal_id,
                "identity": principal.keys.first(),
                "kind": match principal.kind {
                    PrincipalKind::Person => "person",
                    PrincipalKind::Agent => "agent",
                },
                "name": principal.name,
                "fingerprint": principal.fingerprint,
            })
        })
        .collect();
    let cursor = snapshot.cursor.to_string();
    if principals.is_empty() {
        return vec![json!({
            "available": true, "cursor": cursor, "principals": [], "part": 0, "parts": 1,
        })];
    }
    let parts = principals.len().div_ceil(PRINCIPALS_PER_EVENT);
    principals
        .chunks(PRINCIPALS_PER_EVENT)
        .enumerate()
        .map(|(part, chunk)| {
            json!({
                "available": true, "cursor": cursor, "principals": chunk,
                "part": part, "parts": parts,
            })
        })
        .collect()
}

async fn read_ticket(
    context: &Context<BacklogActive>,
    ticket_id: &str,
) -> Result<TicketView, BackendError> {
    let state = context.state().clone();
    let id = ticket_id.to_owned();
    context
        .blocking(move |_| state.store.ticket(&id).map_err(storage_failed))
        .await?
        .ok_or_else(|| not_found(ticket_id))
}

/// Takes the Ticket's lock and refuses a save based on an older version than this
/// Installation has seen. The caller holds the guard until its append has been projected.
async fn guarded(
    context: &Context<BacklogActive>,
    ticket_id: &str,
    base_version: &str,
) -> Result<tokio::sync::OwnedMutexGuard<()>, BackendError> {
    let guard = context.state().locks.lock(ticket_id).await;
    let ticket = read_ticket(context, ticket_id).await?;
    if ticket.version != base_version {
        return Err(stale_version(&ticket));
    }
    Ok(guard)
}

async fn version_value(
    context: &Context<BacklogActive>,
    ticket_id: &str,
) -> Result<JsonValue, BackendError> {
    let ticket = read_ticket(context, ticket_id).await?;
    Ok(json!({ "version": ticket.version }))
}

/// Whether the Epic exists, and whether it is archived.
async fn epic_archived(
    context: &Context<BacklogActive>,
    epic_id: &str,
) -> Result<bool, BackendError> {
    let state = context.state().clone();
    let id = epic_id.to_owned();
    context
        .blocking(move |_| {
            Ok(state
                .store
                .epics()
                .map_err(storage_failed)?
                .into_iter()
                .find(|epic| epic.id == id)
                .map(|epic| epic.archived))
        })
        .await?
        .ok_or_else(|| not_found(epic_id))
}

async fn existing_epic(
    context: &Context<BacklogActive>,
    epic_id: &str,
) -> Result<(), BackendError> {
    epic_archived(context, epic_id).await.map(|_| ())
}

/// Refuses an unknown or archived Epic as the home of a new or moved Ticket.
async fn open_epic(context: &Context<BacklogActive>, epic_id: &str) -> Result<(), BackendError> {
    if epic_archived(context, epic_id).await? {
        return Err(error(
            "backlog.epic_archived",
            false,
            json!({ "epicId": epic_id }),
        ));
    }
    Ok(())
}

/// Accepts only a current Workspace Person as an assignee.
async fn check_assignee(
    context: &Context<BacklogActive>,
    principal_id: &str,
) -> Result<(), BackendError> {
    let roster = context
        .state()
        .principals
        .current()
        .await
        .map_err(|cause| capability_error("principals", cause))?;
    if roster.principals.iter().any(|principal| {
        principal.principal_id == principal_id && principal.kind == PrincipalKind::Person
    }) {
        Ok(())
    } else {
        Err(error(
            "backlog.assignee_invalid",
            false,
            json!({ "principalId": principal_id }),
        ))
    }
}

async fn create_ticket(
    context: &Context<BacklogActive>,
    author: &Author,
    request: TicketCreateRequest,
) -> Result<JsonValue, BackendError> {
    let status = parse_status(request.status.as_deref())?.unwrap_or(DEFAULT_STATUS);
    let priority = parse_priority(request.priority.as_deref())?.unwrap_or(DEFAULT_PRIORITY);
    let record = DomainRecord::ticket_create(
        &author.identity,
        now_ms(),
        &request.epic_id,
        &request.title,
        request.body.as_deref().unwrap_or_default(),
        status,
        priority,
    )
    .map_err(|failure| domain_error(&failure))?;
    open_epic(context, &request.epic_id).await?;
    let ticket_id = append_as(context, author, record).await?;
    let mut value = version_value(context, &ticket_id).await?;
    value["ticketId"] = JsonValue::String(ticket_id);
    Ok(value)
}

async fn update_ticket(
    context: &Context<BacklogActive>,
    author: &Author,
    request: TicketUpdateRequest,
) -> Result<JsonValue, BackendError> {
    let changes = TicketChanges {
        title: request.title,
        body: request.body,
        status: parse_status(request.status.as_deref())?,
        priority: parse_priority(request.priority.as_deref())?,
    };
    let record =
        DomainRecord::ticket_update(&author.identity, now_ms(), &request.ticket_id, changes)
            .map_err(|failure| domain_error(&failure))?;
    let _guard = guarded(context, &request.ticket_id, &request.base_version).await?;
    append_as(context, author, record).await?;
    version_value(context, &request.ticket_id).await
}

/// Appends one record, signed by the Agent `author` names or else by this Installation, and
/// projects it locally before returning its record id.
///
/// An error after the append is marked uncertain: the record may already be on the log, and
/// the session will deliver and project it.
async fn append_as(
    context: &Context<BacklogActive>,
    author: &Author,
    record: DomainRecord,
) -> Result<String, BackendError> {
    let purpose = record.kind();
    let canonical = record.encode();
    let appended = context
        .state()
        .log
        .append_with_principal(purpose, &canonical, author.principal.as_deref())
        .await
        .map_err(|cause| capability_error(purpose, cause))?;
    let state = context.state().clone();
    let record_id = appended.record_id.as_str().to_owned();
    let outcome = context
        .blocking(move |_| {
            let outcome = state
                .store
                .ingest(Incoming {
                    record_id: appended.record_id.as_str(),
                    order_key: appended.order_key.as_str(),
                    author: &appended.author,
                    purpose,
                    signature: &appended.signature,
                    canonical: &canonical,
                    sealed: &appended.sealed_record,
                    cursor: None,
                })
                .map_err(storage_failed);
            state.notify();
            outcome
        })
        .await
        .map_err(|failure| failure.with_outcome_uncertain(true))?;
    match outcome {
        IngestResult::Applied | IngestResult::Duplicate => Ok(record_id),
        IngestResult::Waiting(_) | IngestResult::Quarantined(_) => {
            Err(unavailable().with_outcome_uncertain(true))
        }
    }
}

/// Splits stored rows into `open` batches within the capability's 32-record, 512 KiB bound.
fn open_batches(rows: &[SealedRow]) -> impl Iterator<Item = &[SealedRow]> {
    let mut remaining = rows;
    std::iter::from_fn(move || {
        if remaining.is_empty() {
            return None;
        }
        let mut count = 0;
        let mut bytes: usize = 0;
        for row in remaining.iter().take(32) {
            let Some(total) = bytes.checked_add(row.sealed.len()) else {
                break;
            };
            if total > 512 * 1024 {
                break;
            }
            count += 1;
            bytes = total;
        }
        let count = count.max(1);
        let (batch, rest) = remaining.split_at(count);
        remaining = rest;
        Some(batch)
    })
}

/// Opens every applied or waiting record again, so titles and bodies return to memory and
/// records still waiting for their target wait in memory again.
async fn rehydrate(
    store: &BacklogStore,
    log: &RecordLog,
    cancellation: &CancellationToken,
) -> Result<(), BackendError> {
    let reader = store.clone();
    let rows = teamarness_package_sdk::blocking(cancellation, move |_| {
        reader.sealed_rows().map_err(storage_failed)
    })
    .await?;
    for batch in open_batches(&rows) {
        let sealed: Vec<Vec<u8>> = batch.iter().map(|row| row.sealed.clone()).collect();
        let results = log
            .open(&sealed)
            .await
            .map_err(|cause| capability_error("open-records", cause))?;
        let opened: Vec<(SealedRow, Vec<u8>)> = batch
            .iter()
            .zip(results)
            .filter_map(|(row, result)| match result {
                OpenResult::Ready(record)
                    if record.record_id.as_str() == row.record_id
                        && record.order_key.as_str() == row.order_key
                        && record.author == row.author
                        && record.purpose == row.purpose =>
                {
                    Some((row.clone(), record.canonical))
                }
                // A record that cannot be opened keeps its persisted state; its text shows
                // as Untitled until it can be.
                _ => None,
            })
            .collect();
        let writer = store.clone();
        teamarness_package_sdk::blocking(cancellation, move |_| {
            for (row, canonical) in &opened {
                writer.reapply(row, canonical).map_err(storage_failed)?;
            }
            Ok(())
        })
        .await?;
    }
    Ok(())
}

fn start_session(
    store: &BacklogStore,
    log: &RecordLog,
    sync: &Arc<Mutex<SyncReport>>,
    changes: &watch::Sender<u64>,
) -> Session {
    let sink = Arc::new(BacklogSink {
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
async fn stop_session(state: &BacklogActive) {
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

/// Where the record log session commits what it receives. The session credits a delivery only
/// after `ingest` returned, so a failed ingest is delivered again rather than skipped.
struct BacklogSink {
    store: BacklogStore,
    changes: watch::Sender<u64>,
    sync: Arc<Mutex<SyncReport>>,
}

impl BacklogSink {
    fn with_report(&self, change: impl FnOnce(&mut SyncReport)) {
        change(
            &mut self
                .sync
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        notify(&self.changes);
    }
}

fn remember_warning(report: &mut SyncReport, code: &str) {
    if !report.warnings.iter().any(|entry| entry == code) && report.warnings.len() < MAX_WARNINGS {
        report.warnings.push(code.to_owned());
    }
}

impl RecordLogSink for BacklogSink {
    fn committed_cursor(&self) -> Result<Option<Cursor>, BackendError> {
        let stored = self.store.cursor().map_err(storage_failed)?;
        // A stored value the contract refuses is no cursor: the session replays from the
        // beginning, which record identity deduplicates.
        Ok(stored.and_then(|value| Cursor::parse(value).ok()))
    }

    fn ingest(&self, record: &RecordEvent) -> Result<(), BackendError> {
        let result = self
            .store
            .ingest(Incoming {
                record_id: record.record_id.as_str(),
                order_key: record.order_key.as_str(),
                author: &record.author,
                purpose: &record.purpose,
                signature: &record.signature,
                canonical: &record.canonical,
                sealed: &record.sealed_record,
                cursor: Some(record.cursor.as_str()),
            })
            .map_err(storage_failed)?;
        if result != IngestResult::Duplicate {
            notify(&self.changes);
        }
        Ok(())
    }

    fn reset_cursor(&self) -> Result<(), BackendError> {
        self.store.reset_cursor().map_err(storage_failed)
    }

    fn on_state(&self, state: &SyncState) {
        self.with_report(|report| report.availability = state.availability);
    }

    fn on_warning(&self, warning: &SyncWarning) {
        self.with_report(|report| remember_warning(report, &warning.code));
    }

    fn on_session_error(&self, error: &BackendError) {
        eprintln!(
            "backlog session failure: code={} retryable={}",
            error.code(),
            error.retryable()
        );
        if error.retryable() {
            return;
        }
        let code = error.code().to_owned();
        self.with_report(|report| {
            report.availability = Availability::Offline;
            remember_warning(report, &code);
        });
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use teamarness_package_sdk::capabilities::principals::Principal;
    use teamarness_package_sdk::testing::{
        Harness, HarnessBuilder, ScriptedCapability, assert_router_matches_contract,
    };

    use super::*;

    const RECORD_LOG: &str = "teamarness.synchronization.immutable-record-log";
    pub(super) const ALICE: &str = "alice";
    const BOB: &str = "bob";
    const SENTINEL: &str = "backlog-plaintext-sentinel";

    pub(super) fn hex(length: usize, seed: u64) -> String {
        format!("{seed:0length$x}")
    }

    pub(super) fn person() -> String {
        "c".repeat(64)
    }

    pub(super) fn agent() -> String {
        "e".repeat(64)
    }

    /// What a scripted record log has appended, by sealed bytes, so `open` can answer it.
    #[derive(Clone, Default)]
    pub(super) struct Ledger {
        opened: Arc<Mutex<BTreeMap<String, JsonValue>>>,
        next: Arc<AtomicU64>,
    }

    /// The signing key of the Agent [`agent`] names, as the roster lists it.
    pub(super) fn agent_key() -> String {
        "b".repeat(64)
    }

    /// A record log that signs as this Installation, or as the Agent when an append names the
    /// Agent's Principal, as Core does.
    fn scripted_log(ledger: &Ledger) -> ScriptedCapability {
        let ledger = ledger.clone();
        ledger.next.fetch_max(100, Ordering::SeqCst);
        ScriptedCapability::new().calls(move |method, payload| match method {
            "append" => {
                let seed = ledger.next.fetch_add(1, Ordering::SeqCst);
                let sealed = URL_SAFE_NO_PAD.encode(format!("sealed-{seed}"));
                let author = match payload.get("principal").and_then(JsonValue::as_str) {
                    Some(principal) if principal == agent() => agent_key(),
                    Some(_) => {
                        return Err(BackendError::new("principal_not_bound", false, json!({})));
                    }
                    None => ALICE.to_owned(),
                };
                let opened = json!({
                    "status": "ready", "recordId": hex(64, seed), "orderKey": hex(82, seed),
                    "author": author, "purpose": payload["purpose"], "signature": "c2ln",
                    "canonical": payload["canonical"],
                });
                ledger.opened.lock().unwrap().insert(sealed.clone(), opened);
                Ok(json!({
                    "recordId": hex(64, seed), "orderKey": hex(82, seed), "author": author,
                    "signature": "c2ln", "sealedRecord": sealed, "durability": "committed",
                }))
            }
            "open" => {
                let opened = ledger.opened.lock().unwrap();
                let results: Vec<JsonValue> = payload["sealedRecords"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|sealed| {
                        opened
                            .get(sealed.as_str().unwrap())
                            .cloned()
                            .unwrap_or_else(|| json!({ "status": "waiting_for_history_access" }))
                    })
                    .collect();
                Ok(json!({ "results": results }))
            }
            _ => Err(BackendError::new(
                "capability_unavailable",
                false,
                json!({}),
            )),
        })
    }

    fn scripted_principals() -> ScriptedCapability {
        ScriptedCapability::new().calls(|method, _| match method {
            "current" => Ok(
                json!({ "workspaceId": "a".repeat(32), "cursor": "3", "principals": [
                { "principalId": person(), "kind": "person", "stewardPrincipalId": null,
                  "keys": ["d".repeat(64)], "name": "Alice", "fingerprint": "dddddddddddd" },
                { "principalId": agent(), "kind": "agent", "stewardPrincipalId": person(),
                  "keys": ["b".repeat(64)], "name": "Helper", "fingerprint": "bbbbbbbbbbbb" }
            ] }),
            ),
            _ => Err(BackendError::new(
                "capability_unavailable",
                false,
                json!({}),
            )),
        })
    }

    pub(super) struct Session {
        pub(super) harness: Harness,
        pub(super) log: ScriptedCapability,
    }

    pub(super) async fn start(directory: &std::path::Path, ledger: &Ledger) -> Session {
        let log = scripted_log(ledger);
        let mut harness = HarnessBuilder::new(router().into_backend())
            .package_identity("dev.teamarness.backlog")
            .installation(ALICE, "Alice")
            .data_directory(directory)
            .capability_minor(RECORD_LOG, log.clone(), 1)
            .capability("teamarness.principals", scripted_principals())
            .start()
            .await;
        harness.activate().await.unwrap();
        Session { harness, log }
    }

    pub(super) async fn request(
        session: &mut Session,
        operation: &str,
        payload: JsonValue,
    ) -> JsonValue {
        session
            .harness
            .request(operation, payload)
            .await
            .unwrap_or_else(|failure| panic!("{operation}: {}", failure.code()))
    }

    pub(super) async fn refusal(
        session: &mut Session,
        operation: &str,
        payload: JsonValue,
    ) -> BackendError {
        session
            .harness
            .request(operation, payload)
            .await
            .expect_err(operation)
    }

    pub(super) async fn snapshot(session: &mut Session) -> JsonValue {
        let mut changes = session
            .harness
            .subscribe("backlog.changes", json!({}))
            .await
            .unwrap();
        let first = changes.next_event().await.unwrap().unwrap();
        changes.close().await;
        first
    }

    pub(super) async fn epic(session: &mut Session, title: &str) -> String {
        request(session, "backlog.epic.create", json!({ "title": title })).await["epicId"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    pub(super) async fn ticket(
        session: &mut Session,
        epic_id: &str,
        title: &str,
    ) -> (String, String) {
        let created = request(
            session,
            "backlog.ticket.create",
            json!({ "epicId": epic_id, "title": title }),
        )
        .await;
        (
            created["ticketId"].as_str().unwrap().to_owned(),
            created["version"].as_str().unwrap().to_owned(),
        )
    }

    pub(super) async fn get(session: &mut Session, ticket_id: &str) -> JsonValue {
        request(
            session,
            "backlog.ticket.get",
            json!({ "ticketId": ticket_id }),
        )
        .await
    }

    // Verifies: TEAM.SYS.BACKLOG-R01
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn epic_requests_create_rename_archive_restore_and_refuse() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), &Ledger::default()).await;
        let epic_id = epic(&mut session, "Launch").await;
        request(
            &mut session,
            "backlog.epic.rename",
            json!({ "epicId": epic_id, "title": "Launch v2" }),
        )
        .await;
        request(
            &mut session,
            "backlog.epic.archive",
            json!({ "epicId": epic_id }),
        )
        .await;
        let archived = snapshot(&mut session).await;
        assert_eq!(archived["epics"][0]["title"], "Launch v2");
        assert_eq!(archived["epics"][0]["archived"], true);
        assert_eq!(archived["epics"][0]["createdOrder"], hex(82, 100));
        assert_eq!(archived["availability"], "catching_up");
        assert_eq!(archived["warnings"], json!([]));
        request(
            &mut session,
            "backlog.epic.restore",
            json!({ "epicId": epic_id }),
        )
        .await;
        assert_eq!(snapshot(&mut session).await["epics"][0]["archived"], false);

        let unknown = hex(64, 9);
        for operation in ["backlog.epic.archive", "backlog.epic.restore"] {
            let failure = refusal(&mut session, operation, json!({ "epicId": unknown })).await;
            assert_eq!(failure.code(), "backlog.not_found");
            assert_eq!(failure.details()["id"], unknown);
        }
        let failure = refusal(
            &mut session,
            "backlog.epic.rename",
            json!({ "epicId": unknown, "title": "x" }),
        )
        .await;
        assert_eq!(failure.code(), "backlog.not_found");
        let failure = refusal(
            &mut session,
            "backlog.epic.create",
            json!({ "title": " x" }),
        )
        .await;
        assert_eq!(failure.code(), "backlog.invalid");
        assert_eq!(
            failure.details(),
            &json!({ "field": "title", "reason": "invalid" })
        );
        let failure = refusal(
            &mut session,
            "backlog.epic.create",
            json!({ "title": "x".repeat(257) }),
        )
        .await;
        assert_eq!(failure.code(), "backlog.too_large");
        assert_eq!(
            failure.details(),
            &json!({ "field": "title", "limit": 256 })
        );
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.BACKLOG-R02, TEAM.SYS.BACKLOG-R05
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ticket_create_defaults_and_refuses_unknown_or_archived_epics() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), &Ledger::default()).await;
        let epic_id = epic(&mut session, "Launch").await;
        let (ticket_id, version) = ticket(&mut session, &epic_id, "Write the plan").await;
        let full = get(&mut session, &ticket_id).await;
        assert_eq!(
            full,
            json!({
                "ticketId": ticket_id, "epicId": epic_id, "title": "Write the plan", "body": "",
                "status": "not_ready", "priority": "medium", "assignee": null,
                "archived": false, "version": version, "creator": ALICE,
                "createdAt": full["createdAt"], "lastEditor": ALICE, "lastAt": full["createdAt"],
            })
        );
        assert!(full["createdAt"].as_u64().unwrap() > 0);
        let created = request(
            &mut session,
            "backlog.ticket.create",
            json!({ "epicId": epic_id, "title": "Ship", "body": "- [ ] go",
                    "status": "ready", "priority": "high" }),
        )
        .await;
        let shipped = get(&mut session, created["ticketId"].as_str().unwrap()).await;
        assert_eq!(
            (&shipped["body"], &shipped["status"], &shipped["priority"]),
            (&json!("- [ ] go"), &json!("ready"), &json!("high"))
        );
        let listed = snapshot(&mut session).await;
        assert_eq!(listed["tickets"].as_array().unwrap().len(), 2);
        assert!(listed["tickets"][0].get("body").is_none());

        let unknown = hex(64, 9);
        let failure = refusal(
            &mut session,
            "backlog.ticket.create",
            json!({ "epicId": unknown, "title": "x" }),
        )
        .await;
        assert_eq!(failure.code(), "backlog.not_found");
        let failure = refusal(
            &mut session,
            "backlog.ticket.get",
            json!({ "ticketId": unknown }),
        )
        .await;
        assert_eq!(failure.code(), "backlog.not_found");
        let failure = refusal(
            &mut session,
            "backlog.ticket.create",
            json!({ "epicId": epic_id, "title": "x", "status": "blocked" }),
        )
        .await;
        assert_eq!(
            failure.details(),
            &json!({ "field": "status", "reason": "invalid" })
        );
        let failure = refusal(
            &mut session,
            "backlog.ticket.create",
            json!({ "epicId": epic_id, "title": "x", "body": "x".repeat(65_537) }),
        )
        .await;
        assert_eq!(failure.code(), "backlog.too_large");
        assert_eq!(
            failure.details(),
            &json!({ "field": "body", "limit": 65_536 })
        );
        request(
            &mut session,
            "backlog.epic.archive",
            json!({ "epicId": epic_id }),
        )
        .await;
        let failure = refusal(
            &mut session,
            "backlog.ticket.create",
            json!({ "epicId": epic_id, "title": "x" }),
        )
        .await;
        assert_eq!(failure.code(), "backlog.epic_archived");
        assert_eq!(failure.details(), &json!({ "epicId": epic_id }));
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.BACKLOG-R02, TEAM.SYS.BACKLOG-R04
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ticket_changes_check_the_base_version_and_their_targets() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), &Ledger::default()).await;
        let home = epic(&mut session, "Home").await;
        let away = epic(&mut session, "Away").await;
        let (ticket_id, created) = ticket(&mut session, &home, "Plan").await;

        let updated = request(
            &mut session,
            "backlog.ticket.update",
            json!({ "ticketId": ticket_id, "baseVersion": created, "body": "New body",
                    "status": "in_progress" }),
        )
        .await["version"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(updated > created);
        let stale = refusal(
            &mut session,
            "backlog.ticket.update",
            json!({ "ticketId": ticket_id, "baseVersion": created, "title": "Lost" }),
        )
        .await;
        assert_eq!(stale.code(), "backlog.stale_version");
        assert_eq!(
            stale.details()["ticket"],
            get(&mut session, &ticket_id).await
        );
        assert_eq!(stale.details()["ticket"]["body"], "New body");
        let empty = refusal(
            &mut session,
            "backlog.ticket.update",
            json!({ "ticketId": ticket_id, "baseVersion": updated }),
        )
        .await;
        assert_eq!(
            empty.details(),
            &json!({ "field": "changes", "reason": "empty" })
        );

        let moved = request(
            &mut session,
            "backlog.ticket.move",
            json!({ "ticketId": ticket_id, "baseVersion": updated, "epicId": away }),
        )
        .await["version"]
            .clone();
        let current = get(&mut session, &ticket_id).await;
        assert_eq!(current["epicId"], away);
        assert_eq!(current["version"], moved);
        request(
            &mut session,
            "backlog.epic.archive",
            json!({ "epicId": home }),
        )
        .await;
        let refused = refusal(
            &mut session,
            "backlog.ticket.move",
            json!({ "ticketId": ticket_id, "baseVersion": moved, "epicId": home }),
        )
        .await;
        assert_eq!(refused.code(), "backlog.epic_archived");
        let refused = refusal(
            &mut session,
            "backlog.ticket.move",
            json!({ "ticketId": ticket_id, "baseVersion": moved, "epicId": hex(64, 9) }),
        )
        .await;
        assert_eq!(refused.code(), "backlog.not_found");

        let archived = request(
            &mut session,
            "backlog.ticket.archive",
            json!({ "ticketId": ticket_id, "baseVersion": moved }),
        )
        .await["version"]
            .clone();
        assert_eq!(get(&mut session, &ticket_id).await["archived"], true);
        let stale = refusal(
            &mut session,
            "backlog.ticket.restore",
            json!({ "ticketId": ticket_id, "baseVersion": moved }),
        )
        .await;
        assert_eq!(stale.code(), "backlog.stale_version");
        request(
            &mut session,
            "backlog.ticket.restore",
            json!({ "ticketId": ticket_id, "baseVersion": archived }),
        )
        .await;
        assert_eq!(get(&mut session, &ticket_id).await["archived"], false);
        let missing = refusal(
            &mut session,
            "backlog.ticket.archive",
            json!({ "ticketId": hex(64, 9), "baseVersion": archived }),
        )
        .await;
        assert_eq!(missing.code(), "backlog.not_found");
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.BACKLOG-R04 — two concurrent saves on one base: one wins, the other
    // is refused with the winner's Ticket.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_saves_on_one_base_refuse_all_but_one() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), &Ledger::default()).await;
        let epic_id = epic(&mut session, "Launch").await;
        let (ticket_id, base) = ticket(&mut session, &epic_id, "Plan").await;
        let first = session
            .harness
            .start_request(
                "backlog.ticket.update",
                json!({ "ticketId": ticket_id, "baseVersion": base, "title": "First" }),
            )
            .await;
        let second = session
            .harness
            .start_request(
                "backlog.ticket.update",
                json!({ "ticketId": ticket_id, "baseVersion": base, "title": "Second" }),
            )
            .await;
        let (first, second) = tokio::join!(first.result(), second.result());
        let (won, lost) = match (first, second) {
            (Ok(won), Err(lost)) | (Err(lost), Ok(won)) => (won, lost),
            other => panic!("exactly one save must win: {other:?}"),
        };
        assert_eq!(lost.code(), "backlog.stale_version");
        assert_eq!(lost.details()["ticket"]["version"], won["version"]);
        let current = get(&mut session, &ticket_id).await;
        assert_eq!(current["version"], won["version"]);
        assert_eq!(lost.details()["ticket"]["title"], current["title"]);
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.BACKLOG-R06
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn assignees_are_current_persons_or_none() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), &Ledger::default()).await;
        let epic_id = epic(&mut session, "Launch").await;
        let (ticket_id, base) = ticket(&mut session, &epic_id, "Plan").await;
        for assignee in [agent(), hex(64, 9)] {
            let failure = refusal(
                &mut session,
                "backlog.ticket.assign",
                json!({ "ticketId": ticket_id, "baseVersion": base, "assignee": assignee }),
            )
            .await;
            assert_eq!(failure.code(), "backlog.assignee_invalid");
            assert_eq!(failure.details(), &json!({ "principalId": assignee }));
        }
        let assigned = request(
            &mut session,
            "backlog.ticket.assign",
            json!({ "ticketId": ticket_id, "baseVersion": base, "assignee": person() }),
        )
        .await["version"]
            .clone();
        assert_eq!(get(&mut session, &ticket_id).await["assignee"], person());
        let stale = refusal(
            &mut session,
            "backlog.ticket.assign",
            json!({ "ticketId": ticket_id, "baseVersion": base, "assignee": null }),
        )
        .await;
        assert_eq!(stale.code(), "backlog.stale_version");
        request(
            &mut session,
            "backlog.ticket.assign",
            json!({ "ticketId": ticket_id, "baseVersion": assigned, "assignee": null }),
        )
        .await;
        assert_eq!(
            get(&mut session, &ticket_id).await["assignee"],
            JsonValue::Null
        );
        session.harness.shutdown().await.unwrap();
    }

    fn files_contain(directory: &std::path::Path, needle: &[u8]) -> bool {
        std::fs::read_dir(directory).unwrap().any(|entry| {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files_contain(&path, needle)
            } else {
                std::fs::read(&path)
                    .unwrap()
                    .windows(needle.len())
                    .any(|window| window == needle)
            }
        })
    }

    fn remembered(drafts: JsonValue) -> JsonValue {
        json!({
            "selectedTicketId": hex(64, 1), "editing": true,
            "filters": { "priority": "high", "status": null }, "showArchived": false,
            "expandedEpicIds": [hex(64, 2)], "drafts": drafts,
        })
    }

    // Verifies: TEAM.SYS.BACKLOG-R07, TEAM.SYS.BACKLOG-R10 — remembered state round-trips in
    // memory and never reaches the data directory.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn remembered_state_round_trips_clears_by_key_and_is_never_persisted() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), &Ledger::default()).await;
        assert_eq!(
            request(&mut session, "backlog.draft.get", json!({})).await,
            json!({ "state": null })
        );
        let ticket_key = hex(64, 1);
        let new_key = format!("new:{}", hex(64, 2));
        let state = remembered(json!({
            ticket_key.clone(): { "body": SENTINEL, "baseVersion": hex(82, 5) },
            new_key.clone(): { "title": "New", "baseVersion": "" },
        }));
        request(&mut session, "backlog.draft.put", json!({ "state": state })).await;
        assert_eq!(
            request(&mut session, "backlog.draft.get", json!({})).await["state"],
            state
        );
        assert!(!files_contain(directory.path(), SENTINEL.as_bytes()));

        request(
            &mut session,
            "backlog.draft.clear",
            json!({ "key": ticket_key }),
        )
        .await;
        let cleared = request(&mut session, "backlog.draft.get", json!({})).await["state"].clone();
        assert_eq!(
            cleared["drafts"],
            json!({ new_key: { "title": "New", "baseVersion": "" } })
        );
        assert_eq!(cleared["filters"], state["filters"]);

        let oversized = remembered(json!({
            hex(64, 1): { "body": "x".repeat(65_536), "baseVersion": "" },
            hex(64, 2): { "body": "x".repeat(65_536), "baseVersion": "" },
            hex(64, 3): { "body": "x".repeat(65_536), "baseVersion": "" },
            hex(64, 4): { "body": "x".repeat(65_536), "baseVersion": "" },
        }));
        let failure = refusal(
            &mut session,
            "backlog.draft.put",
            json!({ "state": oversized }),
        )
        .await;
        assert_eq!(failure.code(), "backlog.too_large");
        assert_eq!(
            failure.details(),
            &json!({ "field": "state", "limit": REMEMBERED_STATE_LIMIT })
        );
        session.harness.shutdown().await.unwrap();
        assert!(!files_contain(directory.path(), SENTINEL.as_bytes()));
    }

    // Verifies: TEAM.SYS.BACKLOG-R10 — after a restart, titles and bodies are rebuilt from the
    // sealed rows the record log opens, and SQLite never held them.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn activation_rebuilds_titles_and_bodies_from_sealed_rows() {
        let directory = tempfile::tempdir().unwrap();
        let ledger = Ledger::default();
        let mut session = start(directory.path(), &ledger).await;
        let epic_id = epic(&mut session, &format!("{SENTINEL} epic")).await;
        let (ticket_id, base) = ticket(&mut session, &epic_id, "Plan").await;
        request(
            &mut session,
            "backlog.ticket.update",
            json!({ "ticketId": ticket_id, "baseVersion": base,
                    "title": format!("{SENTINEL} title"), "body": format!("{SENTINEL} body") }),
        )
        .await;
        let before = get(&mut session, &ticket_id).await;
        session.harness.shutdown().await.unwrap();
        assert!(!files_contain(directory.path(), SENTINEL.as_bytes()));

        let mut session = start(directory.path(), &ledger).await;
        let opens = session
            .log
            .recorded_calls()
            .into_iter()
            .filter(|call| call.method == "open")
            .collect::<Vec<_>>();
        assert_eq!(opens.len(), 1);
        assert_eq!(
            opens[0].payload["sealedRecords"].as_array().unwrap().len(),
            3
        );
        assert_eq!(get(&mut session, &ticket_id).await, before);
        let listed = snapshot(&mut session).await;
        assert_eq!(listed["epics"][0]["title"], format!("{SENTINEL} epic"));
        assert_eq!(listed["tickets"][0]["title"], format!("{SENTINEL} title"));
        session.harness.shutdown().await.unwrap();

        // Records the log cannot open keep their state and show as Untitled.
        let mut session = start(directory.path(), &Ledger::default()).await;
        let unopened = get(&mut session, &ticket_id).await;
        assert_eq!(unopened["title"], UNTITLED);
        assert_eq!(unopened["version"], before["version"]);
        session.harness.shutdown().await.unwrap();
    }

    fn record_frame(seed: u64, author: &str, record: &DomainRecord) -> JsonValue {
        json!({
            "kind": "record", "cursor": hex(50, seed), "recordId": hex(64, seed),
            "orderKey": hex(82, seed), "author": author, "purpose": record.kind(),
            "signature": "c2ln", "canonical": URL_SAFE_NO_PAD.encode(record.encode()),
            "sealedRecord": URL_SAFE_NO_PAD.encode(format!("remote-{seed}")),
        })
    }

    pub(super) async fn next_snapshot(
        changes: &mut teamarness_package_sdk::testing::HarnessSubscription,
        ready: impl Fn(&JsonValue) -> bool,
    ) -> JsonValue {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let event = changes.next_event().await.unwrap().unwrap();
                if ready(&event) {
                    return event;
                }
            }
        })
        .await
        .expect("a matching snapshot")
    }

    // Verifies: TEAM.SYS.BACKLOG-R03 — the session ingests remote records, a record that
    // arrives before its target applies once the target arrives, and every change resends the
    // snapshot.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn remote_records_reach_the_change_subscription_in_any_order() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), &Ledger::default()).await;
        let mut changes = session
            .harness
            .subscribe("backlog.changes", json!({}))
            .await
            .unwrap();
        let first = changes.next_event().await.unwrap().unwrap();
        assert_eq!(first["epics"], json!([]));
        let stream = session.log.stream();
        tokio::time::timeout(Duration::from_secs(5), stream.opened())
            .await
            .unwrap();

        let epic_id = hex(64, 1);
        let ticket_id = hex(64, 2);
        let update = DomainRecord::ticket_update(
            BOB,
            40,
            &ticket_id,
            TicketChanges {
                status: Some(Status::Done),
                ..TicketChanges::default()
            },
        )
        .unwrap();
        let create = DomainRecord::ticket_create(
            BOB,
            20,
            &epic_id,
            "Remote",
            "",
            Status::Ready,
            Priority::Low,
        )
        .unwrap();
        let epic = DomainRecord::epic_create(BOB, 10, "Remote epic").unwrap();
        assert!(stream.send(record_frame(3, BOB, &update)).await);
        assert!(stream.send(record_frame(2, BOB, &create)).await);
        assert!(stream.send(record_frame(1, BOB, &epic)).await);
        let projected = next_snapshot(&mut changes, |event| {
            event["tickets"]
                .as_array()
                .is_some_and(|tickets| !tickets.is_empty())
        })
        .await;
        assert_eq!(projected["epics"][0]["title"], "Remote epic");
        assert_eq!(projected["tickets"][0]["status"], "done");
        assert_eq!(projected["tickets"][0]["version"], hex(82, 3));
        assert_eq!(get(&mut session, &ticket_id).await["lastEditor"], BOB);

        let forged = DomainRecord::ticket_archive(ALICE, 50, &ticket_id).unwrap();
        assert!(stream.send(record_frame(4, BOB, &forged)).await);
        let warned = next_snapshot(&mut changes, |event| {
            event["warnings"]
                .as_array()
                .is_some_and(|warnings| !warnings.is_empty())
        })
        .await;
        assert_eq!(warned["warnings"], json!([PROJECTION_WARNING]));
        assert_eq!(warned["tickets"][0]["archived"], false);
        changes.close().await;
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.BACKLOG-R06 — the roster names every Principal with its kind and key.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn names_relay_the_principal_roster() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), &Ledger::default()).await;
        let mut names = session
            .harness
            .subscribe("backlog.names", json!({}))
            .await
            .unwrap();
        let first = names.next_event().await.unwrap().unwrap();
        assert_eq!(
            first,
            json!({ "available": true, "cursor": "3", "part": 0, "parts": 1, "principals": [
                { "principalId": person(), "identity": "d".repeat(64), "kind": "person",
                  "name": "Alice", "fingerprint": "dddddddddddd" },
                { "principalId": agent(), "identity": "b".repeat(64), "kind": "agent",
                  "name": "Helper", "fingerprint": "bbbbbbbbbbbb" }
            ] })
        );
        names.close().await;
        session.harness.shutdown().await.unwrap();
    }

    #[test]
    fn a_full_roster_splits_into_bounded_parts() {
        let snapshot = PrincipalSnapshot {
            workspace_id: "a".repeat(32),
            cursor: 7,
            principals: (0..1024)
                .map(|n| Principal {
                    principal_id: format!("{n:064x}"),
                    kind: PrincipalKind::Person,
                    steward_principal_id: None,
                    keys: vec![format!("{n:064x}")],
                    name: Some("\\\"".repeat(128)),
                    fingerprint: "f".repeat(64),
                })
                .collect(),
        };
        let parts = name_parts(snapshot);
        assert_eq!(parts.len(), 16);
        for (index, part) in parts.iter().enumerate() {
            assert_eq!(part["part"], index);
            assert_eq!(part["parts"], 16);
            assert_eq!(part["principals"].as_array().unwrap().len(), 64);
            assert!(serde_json::to_vec(part).unwrap().len() < 256 * 1024);
        }
        let empty = name_parts(PrincipalSnapshot {
            workspace_id: "a".repeat(32),
            cursor: 0,
            principals: Vec::new(),
        });
        assert_eq!(
            empty,
            vec![
                json!({ "available": true, "cursor": "0", "principals": [], "part": 0, "parts": 1 })
            ]
        );
    }

    #[test]
    fn a_stale_refusal_that_would_not_fit_omits_the_body() {
        let mut ticket = TicketView {
            id: hex(64, 1),
            epic_id: hex(64, 2),
            title: Some("Plan".into()),
            body: Some("short".into()),
            status: Status::Ready,
            priority: Priority::High,
            assignee: None,
            archived: false,
            version: hex(82, 3),
            creator: ALICE.into(),
            create_order: hex(82, 1),
            create_at: 1,
            last_editor: ALICE.into(),
            last_at: 2,
        };
        let small = stale_version(&ticket);
        assert_eq!(small.details()["ticket"], ticket_value(&ticket));
        assert!(small.details().get("bodyOmitted").is_none());
        ticket.body = Some("x".repeat(65_536));
        let large = stale_version(&ticket);
        assert_eq!(large.details()["bodyOmitted"], true);
        assert!(large.details()["ticket"].get("body").is_none());
        assert_eq!(large.details()["ticket"]["version"], hex(82, 3));
        assert!(serde_json::to_vec(large.details()).unwrap().len() <= ERROR_DETAILS_LIMIT);
    }

    #[test]
    fn router_matches_the_ui_contract() {
        assert_router_matches_contract(
            &router().operations(),
            include_str!("../../ui-contract.json"),
        );
    }
}
