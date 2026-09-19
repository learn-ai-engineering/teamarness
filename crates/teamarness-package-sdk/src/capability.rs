use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use teamarness_package_protocol::{INITIAL_STREAM_CREDIT, JsonValue};
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::priority_writer::{PriorityWriter, SharedState};
use crate::{BackendError, object};

/// How long a capability call waits for the Host's answer.
///
/// One second longer than the Host's own call deadline, so a Host that is
/// answering slowly reports its own `deadline_exceeded` first.
pub(crate) const CALL_DEADLINE: Duration = Duration::from_secs(11);

pub(crate) type PendingCapabilities =
    Arc<Mutex<BTreeMap<String, oneshot::Sender<Result<JsonValue, BackendError>>>>>;
pub(crate) type CapabilityStreams = Arc<Mutex<BTreeMap<String, CapabilityStreamRoute>>>;

pub(crate) struct CapabilityStreamRoute {
    pub(crate) sender: mpsc::Sender<Result<JsonValue, BackendError>>,
}

/// A cloneable client for generation-scoped Semantic Capability calls.
#[derive(Clone)]
pub struct CapabilityClient {
    writer: Arc<PriorityWriter>,
    pending: PendingCapabilities,
    next_id: Arc<AtomicU64>,
    streams: CapabilityStreams,
    state: SharedState,
}

/// A credit-controlled stream opened through a Semantic Capability.
pub struct CapabilitySubscription {
    id: String,
    receiver: mpsc::Receiver<Result<JsonValue, BackendError>>,
    writer: Arc<PriorityWriter>,
    streams: CapabilityStreams,
    state: SharedState,
    closed: bool,
}

impl CapabilitySubscription {
    /// Receives one event, returning `None` after the host closes the stream.
    pub async fn recv(&mut self) -> Option<Result<JsonValue, BackendError>> {
        let event = self.receiver.recv().await;
        if matches!(event, Some(Ok(_))) {
            let _ = self
                .writer
                .control(object([
                    ("kind", JsonValue::String("credit".into())),
                    ("streamId", JsonValue::String(self.id.clone())),
                    ("amount", JsonValue::from(1_u64)),
                ]))
                .await;
        }
        event
    }

    /// Receives one event and the credit it consumed, so the caller decides when to ask for more.
    ///
    /// The host sends no further event until the returned [`StreamCredit`] is released, which is
    /// what lets a consumer commit an event before accepting the next one.
    pub async fn recv_with_credit(
        &mut self,
    ) -> Option<Result<(JsonValue, StreamCredit), BackendError>> {
        match self.receiver.recv().await? {
            Ok(payload) => Some(Ok((
                payload,
                StreamCredit {
                    id: self.id.clone(),
                    writer: self.writer.clone(),
                },
            ))),
            Err(error) => Some(Err(error)),
        }
    }

    /// Closes the stream and releases its registry entry.
    ///
    /// The stream is retired before the entry goes, so events the host wrote
    /// against credit it still held are ignored rather than read as events for
    /// a stream that does not exist.
    pub async fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.state.lock().await.retire_stream(&self.id);
        self.streams.lock().await.remove(&self.id);
        let _ = self
            .writer
            .control(object([
                ("kind", JsonValue::String("stream_close".into())),
                ("streamId", JsonValue::String(self.id.clone())),
                ("code", JsonValue::String("closed".into())),
            ]))
            .await;
    }
}

/// One event's worth of credit, still owed to the host.
///
/// Dropping it without releasing it keeps the window closed, which is how a consumer applies
/// back pressure while it commits what it has already received.
#[must_use = "the host sends no further event until this credit is released"]
pub struct StreamCredit {
    id: String,
    writer: Arc<PriorityWriter>,
}

impl StreamCredit {
    /// Returns the credit, allowing the host to send one more event.
    pub async fn release(self) {
        let _ = self
            .writer
            .control(object([
                ("kind", JsonValue::String("credit".into())),
                ("streamId", JsonValue::String(self.id)),
                ("amount", JsonValue::from(1_u64)),
            ]))
            .await;
    }
}

impl Drop for CapabilitySubscription {
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        let id = self.id.clone();
        let streams = self.streams.clone();
        let writer = self.writer.clone();
        let state = self.state.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                state.lock().await.retire_stream(&id);
                streams.lock().await.remove(&id);
                let _ = writer
                    .control(object([
                        ("kind", JsonValue::String("stream_close".into())),
                        ("streamId", JsonValue::String(id)),
                        ("code", JsonValue::String("dropped".into())),
                    ]))
                    .await;
            });
        }
    }
}

impl std::fmt::Debug for CapabilityClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CapabilityClient")
            .finish_non_exhaustive()
    }
}

impl CapabilityClient {
    pub(crate) fn new(
        writer: Arc<PriorityWriter>,
        pending: PendingCapabilities,
        streams: CapabilityStreams,
        state: SharedState,
    ) -> Self {
        Self {
            writer,
            pending,
            next_id: Arc::new(AtomicU64::new(1)),
            streams,
            state,
        }
    }

    /// Opens the generic capability stream with the supplied opaque grant.
    pub async fn subscribe(
        &self,
        grant: &str,
        payload: JsonValue,
    ) -> Result<CapabilitySubscription, BackendError> {
        let JsonValue::Object(mut payload) = payload else {
            return Err(invalid_request());
        };
        if payload
            .insert("grant".into(), JsonValue::String(grant.into()))
            .is_some()
        {
            return Err(invalid_request());
        }
        let id = format!(
            "p:cap-stream-{}",
            self.next_id.fetch_add(1, Ordering::Relaxed)
        );
        // The route holds exactly the credit window the contract grants, so a
        // conforming host can never overrun it.
        let (sender, receiver) = mpsc::channel(INITIAL_STREAM_CREDIT);
        self.streams
            .lock()
            .await
            .insert(id.clone(), CapabilityStreamRoute { sender });
        let value = object([
            ("kind", JsonValue::String("stream_open".into())),
            ("id", JsonValue::String(id.clone())),
            ("topic", JsonValue::String("capability.stream".into())),
            (
                "initialCredit",
                JsonValue::from(INITIAL_STREAM_CREDIT as u64),
            ),
            ("payload", JsonValue::Object(payload)),
        ]);
        if self.writer.control(value).await.is_err() {
            self.streams.lock().await.remove(&id);
            return Err(unavailable());
        }
        Ok(CapabilitySubscription {
            id,
            receiver,
            writer: self.writer.clone(),
            streams: self.streams.clone(),
            state: self.state.clone(),
            closed: false,
        })
    }

    /// Routes one already-classified event to its stream.
    ///
    /// `ContractState` decides whether an event is legal, late or fatal before
    /// it reaches here, including its sequence. A route this side has already
    /// dropped is a race the classifier tolerated, so the event is discarded
    /// rather than reclassified. A full route is the one condition left, and a
    /// conforming host cannot produce it inside the credit window.
    pub(crate) async fn event(
        streams: &CapabilityStreams,
        id: &str,
        payload: JsonValue,
    ) -> Result<(), &'static str> {
        let mut routes = streams.lock().await;
        let Some(route) = routes.get_mut(id) else {
            return Ok(());
        };
        route
            .sender
            .try_send(Ok(payload))
            .map_err(|_| "event_lagged")
    }

    pub(crate) async fn close_stream(
        streams: &CapabilityStreams,
        id: &str,
        error: Option<BackendError>,
    ) {
        if let Some(route) = streams.lock().await.remove(id)
            && let Some(error) = error
        {
            let _ = route.sender.send(Err(error)).await;
        }
    }

    /// Calls one method through an opaque grant supplied during initialization.
    pub async fn call(
        &self,
        grant: &str,
        method: &str,
        payload: JsonValue,
    ) -> Result<JsonValue, BackendError> {
        let mut input = match payload {
            JsonValue::Object(input) => input,
            _ => {
                return Err(BackendError::new(
                    "capability_request_invalid",
                    false,
                    JsonValue::Object(Default::default()),
                ));
            }
        };
        if input
            .insert("method".into(), JsonValue::String(method.into()))
            .is_some()
        {
            return Err(BackendError::new(
                "capability_request_invalid",
                false,
                JsonValue::Object(Default::default()),
            ));
        }
        let id = format!("p:{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let deadline = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .saturating_add(10_000)
            .to_string();
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id.clone(), sender);
        let value = object([
            ("kind", JsonValue::String("request".into())),
            ("id", JsonValue::String(id.clone())),
            ("operation", JsonValue::String("capability.call".into())),
            ("grant", JsonValue::String(grant.into())),
            ("deadlineUnixMs", JsonValue::String(deadline)),
            ("payload", JsonValue::Object(input)),
        ]);
        if self.writer.data(value).await.is_err() {
            self.pending.lock().await.remove(&id);
            return Err(unavailable());
        }
        match tokio::time::timeout(CALL_DEADLINE, receiver).await {
            Ok(answer) => answer.unwrap_or_else(|_| Err(unavailable())),
            Err(_) => {
                // The caller stops waiting here, so the request is retired
                // here: the host always answers, and its late first terminal
                // frame is then ignored instead of read as an answer to a
                // request this side never made.
                self.pending.lock().await.remove(&id);
                self.state.lock().await.retire_request(&id);
                Err(BackendError::new(
                    "deadline_exceeded",
                    true,
                    JsonValue::Object(Default::default()),
                ))
            }
        }
    }
}

/// Fails every capability call and stream that is still waiting.
///
/// Called once the dispatcher has returned: nothing will answer them now, and a
/// Package task holding a client must not wait forever.
pub(crate) async fn fail_all(pending: &PendingCapabilities, streams: &CapabilityStreams) {
    let waiting = std::mem::take(&mut *pending.lock().await);
    for (_, sender) in waiting {
        let _ = sender.send(Err(unavailable()));
    }
    let routes = std::mem::take(&mut *streams.lock().await);
    for (_, route) in routes {
        let _ = route.sender.send(Err(unavailable())).await;
    }
}

fn invalid_request() -> BackendError {
    BackendError::new(
        "capability_request_invalid",
        false,
        JsonValue::Object(Default::default()),
    )
}

fn unavailable() -> BackendError {
    BackendError::new(
        "host_unavailable",
        true,
        JsonValue::Object(Default::default()),
    )
}
