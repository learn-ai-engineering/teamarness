use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use teamarness_package_protocol::JsonValue;
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::priority_writer::PriorityWriter;
use crate::{BackendError, object};

pub(crate) type PendingCapabilities =
    Arc<Mutex<BTreeMap<String, oneshot::Sender<Result<JsonValue, BackendError>>>>>;
pub(crate) type CapabilityStreams = Arc<Mutex<BTreeMap<String, CapabilityStreamRoute>>>;

pub(crate) struct CapabilityStreamRoute {
    pub(crate) sender: mpsc::Sender<Result<JsonValue, BackendError>>,
    pub(crate) next_sequence: u64,
}

/// A cloneable client for generation-scoped Semantic Capability calls.
#[derive(Clone)]
pub struct CapabilityClient {
    writer: Arc<PriorityWriter>,
    pending: PendingCapabilities,
    next_id: Arc<AtomicU64>,
    streams: CapabilityStreams,
}

/// A credit-controlled stream opened through a Semantic Capability.
pub struct CapabilitySubscription {
    id: String,
    receiver: mpsc::Receiver<Result<JsonValue, BackendError>>,
    writer: Arc<PriorityWriter>,
    streams: CapabilityStreams,
    closed: bool,
}

impl CapabilitySubscription {
    /// Receives one event, returning `None` after the host closes the stream.
    pub async fn recv(&mut self) -> Option<Result<JsonValue, BackendError>> {
        let event = self.receiver.recv().await;
        if event.is_some() {
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

    /// Closes the stream and releases its registry entry.
    pub async fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
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

impl Drop for CapabilitySubscription {
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        let id = self.id.clone();
        let streams = self.streams.clone();
        let writer = self.writer.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
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
    ) -> Self {
        Self {
            writer,
            pending,
            next_id: Arc::new(AtomicU64::new(1)),
            streams,
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
        let (sender, receiver) = mpsc::channel(32);
        self.streams.lock().await.insert(
            id.clone(),
            CapabilityStreamRoute {
                sender,
                next_sequence: 1,
            },
        );
        let value = object([
            ("kind", JsonValue::String("stream_open".into())),
            ("id", JsonValue::String(id.clone())),
            ("topic", JsonValue::String("capability.stream".into())),
            ("initialCredit", JsonValue::from(32_u64)),
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
            closed: false,
        })
    }

    pub(crate) async fn event(
        streams: &CapabilityStreams,
        id: &str,
        sequence: u64,
        payload: JsonValue,
    ) -> Result<(), &'static str> {
        let mut routes = streams.lock().await;
        let Some(route) = routes.get_mut(id) else {
            return Err("unknown_stream");
        };
        if sequence != route.next_sequence {
            return Err("event_sequence");
        }
        route.next_sequence = route.next_sequence.saturating_add(1);
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
        receiver.await.unwrap_or_else(|_| Err(unavailable()))
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
