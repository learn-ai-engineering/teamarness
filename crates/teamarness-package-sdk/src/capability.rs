use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use teamarness_package_protocol::JsonValue;
use tokio::sync::{Mutex, oneshot};

use crate::priority_writer::PriorityWriter;
use crate::{BackendError, object};

pub(crate) type PendingCapabilities =
    Arc<Mutex<BTreeMap<String, oneshot::Sender<Result<JsonValue, BackendError>>>>>;

/// A cloneable client for generation-scoped Semantic Capability calls.
#[derive(Clone)]
pub struct CapabilityClient {
    writer: Arc<PriorityWriter>,
    pending: PendingCapabilities,
    next_id: Arc<AtomicU64>,
}

impl std::fmt::Debug for CapabilityClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CapabilityClient")
            .finish_non_exhaustive()
    }
}

impl CapabilityClient {
    pub(crate) fn new(writer: Arc<PriorityWriter>, pending: PendingCapabilities) -> Self {
        Self {
            writer,
            pending,
            next_id: Arc::new(AtomicU64::new(1)),
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

fn unavailable() -> BackendError {
    BackendError::new(
        "host_unavailable",
        true,
        JsonValue::Object(Default::default()),
    )
}
