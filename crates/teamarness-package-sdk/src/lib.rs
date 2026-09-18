//! Rust SDK for Package Backend executables.
//!
//! Package code implements [`Backend`]. The SDK owns framed standard I/O,
//! concurrent request dispatch, lifecycle barriers, capability correlation,
//! control-frame priority, cancellation, and the parent-stdin lifeline.

#![forbid(unsafe_code)]

mod capability;
mod dispatcher;
mod lifecycle;
mod parent_lifeline;
mod priority_writer;
mod reader;

use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub use capability::{CapabilityClient, CapabilitySubscription};
pub use lifecycle::LifecycleOperation;
use teamarness_package_protocol::{Frame, JsonValue};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};

/// Initialization values bound to the current process generation.
#[derive(Debug, Clone)]
pub struct Initialization {
    package: JsonValue,
    workspace: JsonValue,
    installation: JsonValue,
    data_directory: JsonValue,
    process_generation: String,
    capabilities: JsonValue,
}

impl Initialization {
    fn from_frame(frame: &Frame) -> Result<Self, SdkError> {
        let value = |name| {
            frame
                .field(name)
                .cloned()
                .ok_or(SdkError::Protocol("malformed_frame"))
        };
        Ok(Self {
            package: value("package")?,
            workspace: value("workspace")?,
            installation: value("installation")?,
            data_directory: value("dataDirectory")?,
            process_generation: frame
                .field("processGeneration")
                .and_then(JsonValue::as_str)
                .map(str::to_owned)
                .ok_or(SdkError::Protocol("malformed_frame"))?,
            capabilities: value("capabilities")?,
        })
    }

    /// Returns the Package descriptor from the initialize frame.
    #[must_use]
    pub const fn package(&self) -> &JsonValue {
        &self.package
    }

    /// Returns the Workspace context bound by the host.
    #[must_use]
    pub const fn workspace(&self) -> &JsonValue {
        &self.workspace
    }

    /// Returns the Installation context bound by the host.
    #[must_use]
    pub const fn installation(&self) -> &JsonValue {
        &self.installation
    }

    /// Returns the direct Package Data Directory grant.
    #[must_use]
    pub const fn data_directory(&self) -> &JsonValue {
        &self.data_directory
    }

    /// Returns the decimal process generation.
    #[must_use]
    pub fn process_generation(&self) -> &str {
        &self.process_generation
    }

    /// Returns the negotiated opaque capability descriptors.
    #[must_use]
    pub const fn capabilities(&self) -> &JsonValue {
        &self.capabilities
    }
}

/// One host request delivered without framing details.
#[derive(Debug, Clone)]
pub struct BackendRequest {
    pub operation: String,
    pub payload: JsonValue,
    pub initialization: Initialization,
    pub capabilities: CapabilityClient,
}

/// A bounded Package-owned terminal error.
#[derive(Debug, Clone)]
pub struct BackendError {
    code: String,
    retryable: bool,
    details: JsonValue,
}

impl BackendError {
    /// Creates an error. Invalid codes are replaced with `backend_error`.
    #[must_use]
    pub fn new(code: impl Into<String>, retryable: bool, details: JsonValue) -> Self {
        let code = code.into();
        let code = if !code.is_empty()
            && code.len() <= 128
            && code.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_.-".contains(&byte)
            }) {
            code
        } else {
            "backend_error".into()
        };
        Self {
            code,
            retryable,
            details: if details.is_object() {
                details
            } else {
                JsonValue::Object(Default::default())
            },
        }
    }

    /// Returns the Package-owned stable code.
    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    /// Reports whether the caller may retry the operation.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        self.retryable
    }
}

/// Sending half of a bounded Package-owned event adapter.
#[derive(Clone)]
pub struct EventSender {
    sender: mpsc::Sender<Result<JsonValue, BackendError>>,
}

impl EventSender {
    /// Sends one object event, waiting only on the SDK's bounded Package queue.
    pub async fn send(&self, payload: JsonValue) -> Result<(), SdkError> {
        if !payload.is_object() {
            return Err(SdkError::Protocol("malformed_frame"));
        }
        self.sender
            .send(Ok(payload))
            .await
            .map_err(|_| SdkError::ParentClosed)
    }

    /// Closes the event stream with a Package-owned error.
    pub async fn fail(&self, error: BackendError) -> Result<(), SdkError> {
        self.sender
            .send(Err(error))
            .await
            .map_err(|_| SdkError::ParentClosed)
    }
}

/// Receiving half consumed by the SDK dispatcher.
pub struct EventSubscription {
    pub(crate) receiver: mpsc::Receiver<Result<JsonValue, BackendError>>,
}

/// Creates a bounded event adapter. Capacity is clamped to 1 through 32.
#[must_use]
pub fn event_channel(capacity: usize) -> (EventSender, EventSubscription) {
    let (sender, receiver) = mpsc::channel(capacity.clamp(1, 32));
    (EventSender { sender }, EventSubscription { receiver })
}

/// Package-owned lifecycle, request, and event implementation.
pub trait Backend: Send + Sync + 'static {
    fn handle(
        &self,
        request: BackendRequest,
    ) -> Pin<Box<dyn Future<Output = Result<JsonValue, BackendError>> + Send + '_>>;

    fn subscribe(
        &self,
        _request: BackendRequest,
    ) -> Pin<Box<dyn Future<Output = Result<EventSubscription, BackendError>> + Send + '_>> {
        Box::pin(async {
            Err(BackendError::new(
                "unsupported_stream",
                false,
                JsonValue::Object(Default::default()),
            ))
        })
    }
}

/// Builder for a Package Backend process service.
pub struct Package {
    backend: Arc<dyn Backend>,
}

impl Package {
    /// Creates an SDK service around Package-owned behavior.
    #[must_use]
    pub fn builder<B>(backend: Arc<B>) -> Self
    where
        B: Backend,
    {
        Self { backend }
    }

    /// Serves the protocol over process stdin and stdout.
    pub async fn serve(self) -> Result<(), SdkError> {
        let input = tokio::fs::File::open("/dev/stdin")
            .await
            .map_err(|_| SdkError::ParentClosed)?;
        let output = tokio::fs::OpenOptions::new()
            .write(true)
            .open("/dev/stdout")
            .await
            .map_err(|_| SdkError::ParentClosed)?;
        self.serve_io(input, output).await
    }

    /// Serves the protocol over supplied I/O. This is useful for conformance tests.
    pub async fn serve_io<R, W>(self, input: R, output: W) -> Result<(), SdkError>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (writer, control, data) = priority_writer::PriorityWriter::channels();
        let writer = Arc::new(writer);
        let (stop_writer, writer_stopped) = oneshot::channel();
        let writer_task = tokio::spawn(priority_writer::run(output, control, data, writer_stopped));
        let (frames_tx, frames_rx) = mpsc::channel(64);
        let reader_task = tokio::spawn(reader::run(input, frames_tx));
        let pending: capability::PendingCapabilities =
            Arc::new(tokio::sync::Mutex::new(std::collections::BTreeMap::new()));
        let streams: capability::CapabilityStreams =
            Arc::new(tokio::sync::Mutex::new(std::collections::BTreeMap::new()));
        let result = dispatcher::run(
            self.backend,
            writer.clone(),
            frames_rx,
            pending.clone(),
            streams.clone(),
        )
        .await;
        reader_task.abort();
        // Nothing will answer a pending capability call once the dispatcher has
        // returned, so every caller is told so before the writer stops.
        capability::fail_all(&pending, &streams).await;
        drop(writer);
        // Package-owned tasks may still hold `CapabilityClient` clones, which keep
        // writer senders alive. Stop the writer explicitly so the session always
        // ends once the dispatcher has returned (shutdown frame or parent closure).
        let _ = stop_writer.send(());
        match writer_task.await {
            Ok(Ok(())) | Err(_) => result,
            Ok(Err(error)) => Err(error),
        }
    }
}

/// Stable SDK transport failures. Payloads and operating-system text are excluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SdkError {
    ParentClosed,
    Protocol(&'static str),
}

impl SdkError {
    fn protocol(error: teamarness_package_protocol::ProtocolError) -> Self {
        Self::Protocol(error.code())
    }
}

impl fmt::Display for SdkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ParentClosed => formatter.write_str("parent_closed"),
            Self::Protocol(code) => formatter.write_str(code),
        }
    }
}

impl Error for SdkError {}

fn object<const N: usize>(entries: [(&str, JsonValue); N]) -> JsonValue {
    let mut value = JsonValue::Object(Default::default());
    let fields = value.as_object_mut().expect("new object");
    for (name, entry) in entries {
        fields.insert(name.to_owned(), entry);
    }
    value
}
