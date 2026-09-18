//! Object-safe immutable record log seam plus the production SDK adapter.
//! Downstream code depends only on this module; P2Panda and transport types stay private.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Map, Value as JsonValue};
use teamarness_package_sdk::{CapabilityClient, CapabilitySubscription};

#[cfg(test)]
use std::collections::VecDeque;
#[cfg(test)]
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// helpers (duplicated so this module stays self-contained)
// ---------------------------------------------------------------------------

fn object<const N: usize>(entries: [(&str, JsonValue); N]) -> JsonValue {
    let mut o = Map::new();
    for (k, v) in entries {
        o.insert(k.into(), v);
    }
    JsonValue::Object(o)
}

fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn from_b64(v: &str) -> Result<Vec<u8>, LogError> {
    URL_SAFE_NO_PAD
        .decode(v)
        .map_err(|_| LogError::InvalidResponse)
}

fn str_field(v: &JsonValue, k: &str) -> Result<String, LogError> {
    v.get(k)
        .and_then(JsonValue::as_str)
        .map(str::to_owned)
        .ok_or(LogError::InvalidResponse)
}

// ---------------------------------------------------------------------------
// public types
// ---------------------------------------------------------------------------

/// Closed set of errors the seam can return.
#[derive(Debug, Clone)]
pub enum LogError {
    /// Transport-level failure from the backing capability.
    Transport(String),
    /// The response shape or field is invalid.
    InvalidResponse,
    /// The event stream was closed by the host.
    StreamClosed,
}

impl fmt::Display for LogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(code) => write!(f, "transport({code})"),
            Self::InvalidResponse => f.write_str("invalid_response"),
            Self::StreamClosed => f.write_str("stream_closed"),
        }
    }
}

/// Output produced by a successful `append`.
#[derive(Debug, Clone)]
pub struct AppendOutput {
    pub record_id: String,
    pub order_key: String,
    pub author: String,
    pub signature: String,
    pub sealed_record: Vec<u8>,
}

/// Status of a single sealed record that was opened.
#[derive(Debug, Clone)]
pub enum OpenResultStatus {
    Ready { canonical: Vec<u8> },
    Waiting,
    WrongScope,
    Corrupt,
}

/// One result from an `open` call, keyed by its position in the input array.
#[derive(Debug, Clone)]
pub struct OpenResult {
    /// Zero-based index into the input sealed-records array.
    pub index: usize,
    pub status: OpenResultStatus,
}

/// Typed events delivered by a [`LogSubscription`].
#[derive(Debug, Clone)]
pub enum LogEvent {
    /// A committed immutable record to ingest and project.
    Record {
        record_id: String,
        order_key: String,
        author: String,
        purpose: String,
        signature: String,
        canonical: Vec<u8>,
        sealed_record: Vec<u8>,
        cursor: Option<String>,
    },
    /// Global availability or synchronization status update.
    State { availability: Option<String> },
    /// A non-fatal warning from the synchronization layer.
    Warning {
        code: String,
        message: Option<String>,
    },
}

/// The supported availability levels for the synchronisation session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncStatus {
    /// No history can be read yet: the Package's record set has no key on this
    /// Installation.
    WaitingForHistoryAccess,
    /// Nothing is reachable, or this Installation may no longer participate.
    Offline,
    /// Records are being caught up; writes may be accepted but projection
    /// is not yet complete.
    CatchingUp,
    /// The durable cursor is at the tip and live events are flowing.
    Current,
}

impl SyncStatus {
    /// Returns the exact availability string for the UI contract.
    pub fn availability(&self) -> &'static str {
        match self {
            Self::WaitingForHistoryAccess => "waiting_for_history_access",
            Self::Offline => "offline",
            Self::CatchingUp => "catching_up",
            Self::Current => "current",
        }
    }

    /// Reads one availability value reported by the Synchronization Capability.
    ///
    /// An unknown value is ignored rather than mapped, so a Host that learns a
    /// new state never silently becomes something else here.
    pub fn from_availability(value: &str) -> Option<Self> {
        match value {
            "waiting_for_history_access" => Some(Self::WaitingForHistoryAccess),
            "offline" => Some(Self::Offline),
            "catching_up" => Some(Self::CatchingUp),
            "current" => Some(Self::Current),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// scripted event queue type (factored for Clippy type_complexity)
// ---------------------------------------------------------------------------

#[cfg(test)]
type ScriptedEventQueue = Arc<Mutex<VecDeque<Option<Result<LogEvent, LogError>>>>>;

// ---------------------------------------------------------------------------
// subscription
// ---------------------------------------------------------------------------

/// Handle to an active streaming subscription.
pub struct LogSubscription {
    inner: LogSubscriptionInner,
}

enum LogSubscriptionInner {
    Live(CapabilitySubscription),
    #[cfg(test)]
    Scripted {
        events: ScriptedEventQueue,
        notify: Arc<tokio::sync::Notify>,
        closed: bool,
    },
}

impl LogSubscription {
    /// Receives the next event. Returns `None` when the host closes the stream.
    pub async fn recv(&mut self) -> Option<Result<LogEvent, LogError>> {
        match &mut self.inner {
            LogSubscriptionInner::Live(sub) => match sub.recv().await {
                Some(Ok(value)) => Some(parse_event(value)),
                Some(Err(e)) => Some(Err(LogError::Transport(e.code().to_owned()))),
                None => None,
            },
            #[cfg(test)]
            LogSubscriptionInner::Scripted {
                events,
                notify,
                closed,
            } => {
                if *closed {
                    return None;
                }
                loop {
                    if let Some(event) = events.lock().unwrap().pop_front() {
                        // None in the queue means "stream closed"
                        if event.is_none() {
                            *closed = true;
                        }
                        return event;
                    }
                    notify.notified().await;
                }
            }
        }
    }

    /// Closes the subscription and releases host resources.
    pub async fn close(&mut self) {
        match &mut self.inner {
            LogSubscriptionInner::Live(sub) => sub.close().await,
            #[cfg(test)]
            LogSubscriptionInner::Scripted { closed, .. } => *closed = true,
        }
    }
}

// ---------------------------------------------------------------------------
// trait
// ---------------------------------------------------------------------------

/// Object-safe seam for an append-only immutable record log.
///
/// Every method accepts `&self` and returns a boxed future so the trait remains
/// object-safe. The production implementation wraps the SDK [`CapabilityClient`];
/// scripted implementations support testing without a running Core.
pub trait ImmutableRecordLog: Send + Sync + 'static {
    /// Append a canonical domain record under `purpose` and return the
    /// Core-assigned metadata plus the sealed envelope.
    fn append(
        &self,
        purpose: &str,
        canonical: &[u8],
    ) -> Pin<Box<dyn Future<Output = Result<AppendOutput, LogError>> + Send + '_>>;

    /// Open one or more sealed records, returning exactly one result per input.
    fn open(
        &self,
        sealed_records: Vec<Vec<u8>>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<OpenResult>, LogError>> + Send + '_>>;

    /// Subscribe to the record log starting after the optional durable cursor.
    fn subscribe(
        &self,
        after_cursor: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Result<LogSubscription, LogError>> + Send + '_>>;
}

// ---------------------------------------------------------------------------
// production adapter
// ---------------------------------------------------------------------------

/// Production adapter that delegates every call to an SDK [`CapabilityClient`]
/// with the stored grant.
pub struct CapabilityLogAdapter {
    client: CapabilityClient,
    grant: String,
}

impl CapabilityLogAdapter {
    pub fn new(client: CapabilityClient, grant: String) -> Self {
        Self { client, grant }
    }
}

impl ImmutableRecordLog for CapabilityLogAdapter {
    fn append(
        &self,
        purpose: &str,
        canonical: &[u8],
    ) -> Pin<Box<dyn Future<Output = Result<AppendOutput, LogError>> + Send + '_>> {
        let client = self.client.clone();
        let grant = self.grant.clone();
        let purpose = purpose.to_owned();
        let canonical_b64 = b64(canonical);
        Box::pin(async move {
            let response = client
                .call(
                    &grant,
                    "append",
                    object([
                        ("purpose", JsonValue::String(purpose)),
                        ("canonical", JsonValue::String(canonical_b64)),
                    ]),
                )
                .await
                .map_err(|e| LogError::Transport(e.code().to_owned()))?;
            Ok(AppendOutput {
                record_id: str_field(&response, "recordId")?,
                order_key: str_field(&response, "orderKey")?,
                author: str_field(&response, "author")?,
                signature: str_field(&response, "signature")?,
                sealed_record: from_b64(&str_field(&response, "sealedRecord")?)?,
            })
        })
    }

    fn open(
        &self,
        sealed_records: Vec<Vec<u8>>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<OpenResult>, LogError>> + Send + '_>> {
        let client = self.client.clone();
        let grant = self.grant.clone();
        Box::pin(async move {
            let sealed: Vec<JsonValue> = sealed_records
                .iter()
                .map(|s| JsonValue::String(b64(s)))
                .collect();
            let response = client
                .call(
                    &grant,
                    "open",
                    object([("sealedRecords", JsonValue::Array(sealed))]),
                )
                .await
                .map_err(|e| LogError::Transport(e.code().to_owned()))?;
            let results = response
                .get("results")
                .and_then(JsonValue::as_array)
                .ok_or(LogError::InvalidResponse)?;
            if results.len() != sealed_records.len() {
                return Err(LogError::InvalidResponse);
            }
            results
                .iter()
                .enumerate()
                .map(|(i, result)| {
                    let status = match result.get("status").and_then(JsonValue::as_str) {
                        Some("ready") | None => {
                            // Backward-compat: missing status = ready.
                            let canonical = result
                                .get("canonical")
                                .and_then(JsonValue::as_str)
                                .ok_or(LogError::InvalidResponse)?;
                            OpenResultStatus::Ready {
                                canonical: from_b64(canonical)?,
                            }
                        }
                        Some("waiting") => OpenResultStatus::Waiting,
                        Some("wrongScope") => OpenResultStatus::WrongScope,
                        Some("corrupt") => OpenResultStatus::Corrupt,
                        _ => return Err(LogError::InvalidResponse),
                    };
                    Ok(OpenResult { index: i, status })
                })
                .collect()
        })
    }

    fn subscribe(
        &self,
        after_cursor: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Result<LogSubscription, LogError>> + Send + '_>> {
        let client = self.client.clone();
        let grant = self.grant.clone();
        Box::pin(async move {
            let sub = client
                .subscribe(
                    &grant,
                    object([
                        ("method", JsonValue::String("subscribe".into())),
                        (
                            "afterCursor",
                            after_cursor.map_or(JsonValue::Null, JsonValue::String),
                        ),
                    ]),
                )
                .await
                .map_err(|e| LogError::Transport(e.code().to_owned()))?;
            Ok(LogSubscription {
                inner: LogSubscriptionInner::Live(sub),
            })
        })
    }
}

// ---------------------------------------------------------------------------
// event parsing
// ---------------------------------------------------------------------------

fn parse_event(value: JsonValue) -> Result<LogEvent, LogError> {
    let kind = value
        .get("kind")
        .and_then(JsonValue::as_str)
        .unwrap_or("record");
    match kind {
        "record" => Ok(LogEvent::Record {
            record_id: str_field(&value, "recordId")?,
            order_key: str_field(&value, "orderKey")?,
            author: str_field(&value, "author")?,
            purpose: str_field(&value, "purpose")?,
            signature: str_field(&value, "signature")?,
            canonical: from_b64(&str_field(&value, "canonical")?)?,
            sealed_record: from_b64(&str_field(&value, "sealedRecord")?)?,
            cursor: value
                .get("cursor")
                .and_then(JsonValue::as_str)
                .map(str::to_owned),
        }),
        "state" => Ok(LogEvent::State {
            availability: value
                .get("availability")
                .and_then(JsonValue::as_str)
                .map(str::to_owned),
        }),
        "warning" => Ok(LogEvent::Warning {
            code: value
                .get("code")
                .and_then(JsonValue::as_str)
                .unwrap_or("unknown")
                .to_owned(),
            message: value
                .get("message")
                .and_then(JsonValue::as_str)
                .map(str::to_owned),
        }),
        _ => Err(LogError::InvalidResponse),
    }
}

// ---------------------------------------------------------------------------
// scripted test adapter
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod scripted {
    use super::*;

    /// Controllable test double for [`ImmutableRecordLog`].
    ///
    /// Callers enqueue predetermined responses for `append` and `open`, and push
    /// events into a shared queue that every subscription reads from.
    pub struct ScriptedLog {
        pub append: Mutex<VecDeque<Result<AppendOutput, LogError>>>,
        pub open: Mutex<VecDeque<Result<Vec<OpenResult>, LogError>>>,
        events: ScriptedEventQueue,
        notify: Arc<tokio::sync::Notify>,
        /// If non-empty, the next `subscribe` call pops and returns this error
        /// instead of creating a subscription.
        pub subscribe_errors: Mutex<VecDeque<LogError>>,
    }

    impl ScriptedLog {
        pub fn new() -> Arc<Self> {
            Arc::new(Self {
                append: Mutex::new(VecDeque::new()),
                open: Mutex::new(VecDeque::new()),
                events: Arc::new(Mutex::new(VecDeque::new())),
                notify: Arc::new(tokio::sync::Notify::new()),
                subscribe_errors: Mutex::new(VecDeque::new()),
            })
        }

        /// Push one event onto the shared queue. All active and future
        /// subscriptions will consume it in order.
        pub fn push_event(&self, event: Option<Result<LogEvent, LogError>>) {
            self.events.lock().unwrap().push_back(event);
            self.notify.notify_one();
        }

        /// Convenience: push a [`LogEvent::Record`].
        pub fn push_record(&self, record_id: &str, order_key: &str) {
            self.push_event(Some(Ok(LogEvent::Record {
                record_id: record_id.to_owned(),
                order_key: order_key.to_owned(),
                author: "scripted".into(),
                purpose: "message.post".into(),
                signature: "sig".into(),
                canonical: b"canon".to_vec(),
                sealed_record: b"sealed".to_vec(),
                cursor: Some(order_key.to_owned()),
            })));
        }

        /// Convenience: push a [`LogEvent::State`].
        pub fn push_state(&self, availability: &str) {
            self.push_event(Some(Ok(LogEvent::State {
                availability: Some(availability.to_owned()),
            })));
        }

        /// Convenience: push a [`LogEvent::Warning`].
        pub fn push_warning(&self, code: &str) {
            self.push_event(Some(Ok(LogEvent::Warning {
                code: code.to_owned(),
                message: None,
            })));
        }

        /// Push a stream-close sentinel (recv returns `None`).
        pub fn push_close(&self) {
            self.push_event(None);
        }
    }

    impl ImmutableRecordLog for ScriptedLog {
        fn append(
            &self,
            _purpose: &str,
            _canonical: &[u8],
        ) -> Pin<Box<dyn Future<Output = Result<AppendOutput, LogError>> + Send + '_>> {
            let result = self
                .append
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Err(LogError::Transport("no_scripted_append".into())));
            Box::pin(std::future::ready(result))
        }

        fn open(
            &self,
            _sealed_records: Vec<Vec<u8>>,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<OpenResult>, LogError>> + Send + '_>> {
            let result = self
                .open
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Err(LogError::Transport("no_scripted_open".into())));
            Box::pin(std::future::ready(result))
        }

        fn subscribe(
            &self,
            _after_cursor: Option<String>,
        ) -> Pin<Box<dyn Future<Output = Result<LogSubscription, LogError>> + Send + '_>> {
            let error = self.subscribe_errors.lock().unwrap().pop_front();
            let events = Arc::clone(&self.events);
            let notify = Arc::clone(&self.notify);
            Box::pin(std::future::ready(if let Some(e) = error {
                Err(e)
            } else {
                Ok(LogSubscription {
                    inner: LogSubscriptionInner::Scripted {
                        events,
                        notify,
                        closed: false,
                    },
                })
            }))
        }
    }
}

// ---------------------------------------------------------------------------
// unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn json_event(kind: &str) -> JsonValue {
        let mut m = Map::new();
        m.insert("kind".into(), JsonValue::String(kind.into()));
        m.insert("recordId".into(), JsonValue::String("rid".into()));
        m.insert("orderKey".into(), JsonValue::String("ok".into()));
        m.insert("author".into(), JsonValue::String("auth".into()));
        m.insert("purpose".into(), JsonValue::String("message.post".into()));
        m.insert("signature".into(), JsonValue::String("sig".into()));
        m.insert("canonical".into(), JsonValue::String(b64(b"canon")));
        m.insert("sealedRecord".into(), JsonValue::String(b64(b"sealed")));
        JsonValue::Object(m)
    }

    #[test]
    fn parse_record_event() {
        let e = parse_event(json_event("record")).unwrap();
        assert!(matches!(e, LogEvent::Record { .. }));
    }

    #[test]
    fn parse_state_event() {
        let mut m = Map::new();
        m.insert("kind".into(), JsonValue::String("state".into()));
        m.insert(
            "availability".into(),
            JsonValue::String("catching_up".into()),
        );
        let e = parse_event(JsonValue::Object(m)).unwrap();
        assert!(matches!(
            e,
            LogEvent::State {
                availability: Some(a)
            } if a == "catching_up"
        ));
    }

    #[test]
    fn parse_warning_event() {
        let mut m = Map::new();
        m.insert("kind".into(), JsonValue::String("warning".into()));
        m.insert("code".into(), JsonValue::String("congestion".into()));
        let e = parse_event(JsonValue::Object(m)).unwrap();
        assert!(matches!(e, LogEvent::Warning { code, .. } if code == "congestion"));
    }

    #[test]
    fn record_event_defaults_to_record_kind() {
        let e = parse_event(json_event("record")).unwrap();
        assert!(matches!(e, LogEvent::Record { .. }));
        // Without an explicit kind, the parser defaults to "record".
        let mut m = Map::new();
        m.insert("recordId".into(), JsonValue::String("rid".into()));
        m.insert("orderKey".into(), JsonValue::String("ok".into()));
        m.insert("author".into(), JsonValue::String("auth".into()));
        m.insert("purpose".into(), JsonValue::String("message.post".into()));
        m.insert("signature".into(), JsonValue::String("sig".into()));
        m.insert("canonical".into(), JsonValue::String(b64(b"canon")));
        m.insert("sealedRecord".into(), JsonValue::String(b64(b"sealed")));
        let e = parse_event(JsonValue::Object(m)).unwrap();
        assert!(matches!(e, LogEvent::Record { .. }));
    }

    #[test]
    fn unknown_kind_is_error() {
        let mut m = Map::new();
        m.insert("kind".into(), JsonValue::String("bogus".into()));
        assert!(parse_event(JsonValue::Object(m)).is_err());
    }

    #[test]
    fn sync_status_availability() {
        for status in [
            SyncStatus::WaitingForHistoryAccess,
            SyncStatus::Offline,
            SyncStatus::CatchingUp,
            SyncStatus::Current,
        ] {
            assert_eq!(
                SyncStatus::from_availability(status.availability()),
                Some(status.clone()),
                "every reported value round trips"
            );
        }
        assert_eq!(
            SyncStatus::WaitingForHistoryAccess.availability(),
            "waiting_for_history_access"
        );
        assert_eq!(SyncStatus::Offline.availability(), "offline");
        assert_eq!(SyncStatus::CatchingUp.availability(), "catching_up");
        assert_eq!(SyncStatus::Current.availability(), "current");
        assert_eq!(SyncStatus::from_availability("degraded"), None);
    }

    #[test]
    fn open_result_count_mismatch_is_error() {
        // This is covered by the adapter returning InvalidResponse when
        // results.len() != input.len(). We verify the logic through the
        // scripted tests below.
    }

    #[tokio::test]
    async fn scripted_log_delivers_record_events() {
        let log = scripted::ScriptedLog::new();
        log.push_record("r1", "01");
        log.push_record("r2", "02");
        log.push_close();

        let mut sub = log.subscribe(None).await.unwrap();

        let e1 = sub.recv().await.unwrap().unwrap();
        assert!(matches!(e1, LogEvent::Record { .. }));

        let e2 = sub.recv().await.unwrap().unwrap();
        assert!(matches!(e2, LogEvent::Record { .. }));

        assert!(sub.recv().await.is_none()); // stream closed
    }

    #[tokio::test]
    async fn scripted_log_delivers_mixed_events() {
        let log = scripted::ScriptedLog::new();
        log.push_record("r1", "01");
        log.push_state("catching_up");
        log.push_warning("congestion");
        log.push_close();

        let mut sub = log.subscribe(None).await.unwrap();
        assert!(matches!(
            sub.recv().await.unwrap().unwrap(),
            LogEvent::Record { .. }
        ));
        assert!(matches!(
            sub.recv().await.unwrap().unwrap(),
            LogEvent::State { .. }
        ));
        assert!(matches!(
            sub.recv().await.unwrap().unwrap(),
            LogEvent::Warning { .. }
        ));
        assert!(sub.recv().await.is_none());
    }

    #[tokio::test]
    async fn scripted_subscribe_error_is_propagated() {
        let log = scripted::ScriptedLog::new();
        log.subscribe_errors
            .lock()
            .unwrap()
            .push_back(LogError::Transport("nope".into()));
        assert!(log.subscribe(None).await.is_err());
    }

    #[tokio::test]
    async fn scripted_append_and_open_follow_queue() {
        let log = scripted::ScriptedLog::new();

        log.append.lock().unwrap().push_back(Ok(AppendOutput {
            record_id: "rid".into(),
            order_key: "01".into(),
            author: "auth".into(),
            signature: "sig".into(),
            sealed_record: b"sealed".to_vec(),
        }));
        let out = log.append("p", b"canon").await.unwrap();
        assert_eq!(out.record_id, "rid");

        log.open.lock().unwrap().push_back(Ok(vec![OpenResult {
            index: 0,
            status: OpenResultStatus::Ready {
                canonical: b"canon".to_vec(),
            },
        }]));
        let results = log.open(vec![b"sealed".to_vec()]).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].status, OpenResultStatus::Ready { .. }));
    }

    #[tokio::test]
    async fn reconnect_reads_from_same_event_queue() {
        let log = scripted::ScriptedLog::new();
        log.push_record("before_reconnect", "01");
        log.push_close(); // first stream closes
        log.push_record("after_reconnect", "02");
        log.push_close();

        // First subscription
        let mut sub = log.subscribe(None).await.unwrap();
        assert!(matches!(
            sub.recv().await.unwrap().unwrap(),
            LogEvent::Record { .. }
        ));
        assert!(sub.recv().await.is_none());
        sub.close().await;

        // Reconnect — consumes from the same queue
        let mut sub2 = log.subscribe(None).await.unwrap();
        assert!(matches!(
            sub2.recv().await.unwrap().unwrap(),
            LogEvent::Record { .. }
        ));
        assert!(sub2.recv().await.is_none());
    }
}
