use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use teamarness_package_protocol::{
    ContractState, Direction, Frame, FrameKind, INITIAL_STREAM_CREDIT, MAX_FRAME_BYTES,
    MAX_IN_FLIGHT_EACH_DIRECTION, MAX_WRITER_BYTES, Verdict, decode_frame, encode_frame,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch};

use crate::ui::{AdmittedUi, MAX_UI_PAYLOAD_BYTES};
use crate::{
    MAX_PACKAGE_ERROR_DETAILS_BYTES, MAX_PACKAGE_ERROR_DETAILS_DEPTH,
    MAX_UI_SUBSCRIPTIONS_PER_MOUNT, PackageErrorCode, PackageOwnedError, PackageValue,
};

type PendingRequests = Arc<Mutex<BTreeMap<String, oneshot::Sender<Result<Value, WireFailure>>>>>;
type UiEventRoutes = Arc<Mutex<BTreeMap<String, Arc<UiEventRoute>>>>;
type SharedState = Arc<Mutex<ContractState>>;

/// One UI event route holds the credit window the contract grants the stream.
const UI_EVENT_CAPACITY: usize = INITIAL_STREAM_CREDIT;
/// The route's byte budget: the whole writer budget one event stream may occupy.
const UI_EVENT_BYTES: usize = MAX_WRITER_BYTES;

/// Lifecycle, health and stream-control frames reserved on the control lane.
///
/// `initialize`, `ping` and `shutdown` are one frame each, and one
/// `stream_open`, `credit` and `stream_close` may be queued for each of the
/// subscriptions one mount may hold open.
const CONTROL_RESERVE: usize = 3 + 3 * MAX_UI_SUBSCRIPTIONS_PER_MOUNT;

/// Slots on the control lane.
///
/// Every one of the in-flight capability answers the contract permits the
/// Package travels here, and the reserved lifecycle traffic shares the lane. A
/// lane shallower than the contract's own concurrency turns legal traffic into
/// a refused frame, which the caller then reports as the process having exited.
const CONTROL_QUEUE: usize = MAX_IN_FLIGHT_EACH_DIRECTION + CONTROL_RESERVE;

/// Slots on the data lane.
///
/// The Host's own in-flight requests travel here, each with the one `cancel`
/// that may follow it, plus the events of Host-owned capability streams.
const DATA_QUEUE: usize = 2 * MAX_IN_FLIGHT_EACH_DIRECTION;

/// The control lane's byte budget: two contract-maximum frames, so one maximum
/// answer never waits on another that is already queued.
const CONTROL_BYTES: usize = 2 * MAX_FRAME_BYTES;

/// The data lane keeps the rest of the writer budget the contract publishes.
pub(crate) const DATA_BYTES: usize = MAX_WRITER_BYTES - CONTROL_BYTES;

pub(crate) struct UiEvent {
    pub(crate) sequence: u64,
    pub(crate) payload: PackageValue,
    _bytes: OwnedSemaphorePermit,
}

pub(crate) struct UiEventReceiver {
    events: mpsc::Receiver<UiEvent>,
    terminal: watch::Receiver<Option<StreamEnd>>,
    route: Arc<UiEventRoute>,
}

/// How a Package UI event stream ended.
///
/// A Package-owned end is the Package's own terminal error and reaches its subscriber; only a
/// host-owned end can fail the mount.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum StreamEnd {
    Completed,
    Package(PackageOwnedError),
    Host(PackageErrorCode),
}

pub(crate) struct UiEventRoute {
    events: mpsc::Sender<UiEvent>,
    terminal: watch::Sender<Option<StreamEnd>>,
    bytes: Arc<Semaphore>,
    state: Mutex<UiEventRouteState>,
    contract: Arc<AdmittedUi>,
    operation: String,
}

struct UiEventRouteState {
    next_expected: u64,
    highest_received: u64,
    highest_delivered: u64,
    highest_acknowledged: u64,
}

impl UiEventReceiver {
    pub(crate) fn terminal(&self) -> watch::Receiver<Option<StreamEnd>> {
        self.terminal.clone()
    }

    pub(crate) async fn changed(&mut self) -> Result<UiEvent, StreamEnd> {
        loop {
            let terminal = self.terminal.borrow().clone();
            if let Some(end) = terminal {
                // A Package-owned end, like a completion, drains what the Package already sent.
                if matches!(end, StreamEnd::Completed | StreamEnd::Package(_)) {
                    match self.events.try_recv() {
                        Ok(event) => {
                            self.route
                                .mark_delivered(event.sequence)
                                .await
                                .map_err(StreamEnd::Host)?;
                            return Ok(event);
                        }
                        Err(
                            mpsc::error::TryRecvError::Empty
                            | mpsc::error::TryRecvError::Disconnected,
                        ) => {}
                    }
                }
                return Err(end);
            }
            tokio::select! {
                biased;
                result = self.terminal.changed() => {
                    if result.is_err() {
                        return Err(StreamEnd::Host(PackageErrorCode::ProcessExited));
                    }
                }
                event = self.events.recv() => {
                    let event = event.ok_or(StreamEnd::Host(PackageErrorCode::ProcessExited))?;
                    self.route
                        .mark_delivered(event.sequence)
                        .await
                        .map_err(StreamEnd::Host)?;
                    return Ok(event);
                }
            }
        }
    }
}

impl UiEventRoute {
    fn new(contract: Arc<AdmittedUi>, operation: String) -> (Arc<Self>, UiEventReceiver) {
        let (events, receiver) = mpsc::channel(UI_EVENT_CAPACITY);
        let (terminal, terminal_receiver) = watch::channel(None);
        let route = Arc::new(Self {
            events,
            terminal,
            bytes: Arc::new(Semaphore::new(UI_EVENT_BYTES)),
            state: Mutex::new(UiEventRouteState {
                next_expected: 1,
                highest_received: 0,
                highest_delivered: 0,
                highest_acknowledged: 0,
            }),
            contract,
            operation,
        });
        let event_receiver = UiEventReceiver {
            events: receiver,
            terminal: terminal_receiver,
            route: route.clone(),
        };
        (route, event_receiver)
    }

    async fn event(&self, sequence: u64, value: Value) -> Result<(), PackageErrorCode> {
        let payload: PackageValue =
            serde_json::from_value(value).map_err(|_| PackageErrorCode::UiContractViolation)?;
        self.contract
            .contract
            .validate_event(&self.operation, &payload)
            .map_err(|_| PackageErrorCode::UiContractViolation)?;
        let encoded =
            serde_json::to_vec(&payload).map_err(|_| PackageErrorCode::UiContractViolation)?;
        if encoded.len() > MAX_UI_PAYLOAD_BYTES {
            return Err(PackageErrorCode::UiContractViolation);
        }
        let mut state = self.state.lock().await;
        if sequence != state.next_expected {
            return Err(PackageErrorCode::ProtocolViolation);
        }
        if state
            .highest_received
            .saturating_sub(state.highest_acknowledged)
            >= UI_EVENT_CAPACITY as u64
        {
            return Err(PackageErrorCode::EventLagged);
        }
        let permits =
            u32::try_from(encoded.len()).map_err(|_| PackageErrorCode::UiContractViolation)?;
        let bytes = self
            .bytes
            .clone()
            .try_acquire_many_owned(permits)
            .map_err(|_| PackageErrorCode::EventLagged)?;
        self.events
            .try_send(UiEvent {
                sequence,
                payload,
                _bytes: bytes,
            })
            .map_err(|_| PackageErrorCode::EventLagged)?;
        state.next_expected = state.next_expected.saturating_add(1);
        state.highest_received = sequence;
        Ok(())
    }

    async fn mark_delivered(&self, sequence: u64) -> Result<(), PackageErrorCode> {
        let mut state = self.state.lock().await;
        if sequence != state.highest_delivered.saturating_add(1) {
            return Err(PackageErrorCode::ProtocolViolation);
        }
        state.highest_delivered = sequence;
        Ok(())
    }

    pub(crate) async fn acknowledge(&self, through: u64) -> Result<u64, WireFailure> {
        let mut state = self.state.lock().await;
        if through <= state.highest_acknowledged
            || through > state.highest_delivered
            || through.saturating_sub(state.highest_acknowledged) > UI_EVENT_CAPACITY as u64
        {
            return Err(WireFailure::new(PackageErrorCode::ProtocolViolation));
        }
        let amount = through - state.highest_acknowledged;
        state.highest_acknowledged = through;
        Ok(amount)
    }

    fn terminate(&self, end: StreamEnd) {
        self.terminal.send_replace(Some(end));
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WireFailure {
    pub(crate) code: PackageErrorCode,
    pub(crate) outcome_uncertain: bool,
    /// The Package's own terminal error, when the Package owns the failure.
    pub(crate) package_error: Option<PackageOwnedError>,
}

impl WireFailure {
    pub(crate) const fn new(code: PackageErrorCode) -> Self {
        Self {
            code,
            outcome_uncertain: false,
            package_error: None,
        }
    }
}

/// Rebuilds a Package-owned error from an `error` frame, within the host's bounds.
///
/// The code and `retryable` always survive; `details` become `{}` when they exceed either bound,
/// which is lossy on purpose so one Package cannot push unbounded data at the Shell.
fn package_owned_error(frame: &Frame) -> PackageOwnedError {
    let code = frame
        .field("code")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let retryable = frame
        .field("retryable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let details = frame
        .field("details")
        .and_then(Value::as_object)
        .cloned()
        .filter(|details| {
            let value = Value::Object(details.clone());
            serde_json::to_vec(&value).is_ok_and(|bytes| {
                bytes.len() <= MAX_PACKAGE_ERROR_DETAILS_BYTES
                    && value_depth(&value) <= MAX_PACKAGE_ERROR_DETAILS_DEPTH
            })
        })
        .unwrap_or_default();
    PackageOwnedError {
        code,
        retryable,
        details,
    }
}

fn value_depth(value: &Value) -> usize {
    match value {
        Value::Array(values) => 1 + values.iter().map(value_depth).max().unwrap_or(0),
        Value::Object(values) => 1 + values.values().map(value_depth).max().unwrap_or(0),
        _ => 0,
    }
}

#[derive(Clone)]
pub(crate) struct ProcessClient {
    writer: Writer,
    pending: PendingRequests,
    state: SharedState,
    ui_event_routes: UiEventRoutes,
    initialized: Arc<Mutex<Option<oneshot::Receiver<Frame>>>>,
    shutdown_ack: watch::Receiver<bool>,
    pongs: watch::Receiver<Option<String>>,
    failure: watch::Receiver<Option<PackageErrorCode>>,
    failure_sender: watch::Sender<Option<PackageErrorCode>>,
}

#[derive(Clone)]
struct Writer {
    control: mpsc::Sender<QueuedFrame>,
    data: mpsc::Sender<QueuedFrame>,
    control_bytes: Arc<Semaphore>,
    data_bytes: Arc<Semaphore>,
}

/// The writing half of one Host-owned capability stream.
///
/// Every method runs on the stream's own task, so all three wait for writer
/// capacity rather than refusing a frame: a slow reader is backpressure, and
/// reporting it as a stream failure is what made ordinary congestion look like
/// a permanent capability outage.
#[derive(Clone)]
pub(crate) struct HostStreamSender {
    writer: Writer,
    state: SharedState,
}

impl HostStreamSender {
    pub(crate) async fn event(
        &self,
        stream_id: &str,
        sequence: u64,
        payload: Value,
    ) -> Result<(), WireFailure> {
        self.writer
            .send_waiting(
                json!({
                    "kind":"event", "streamId":stream_id,
                    "sequence":sequence.to_string(), "payload":payload
                }),
                false,
            )
            .await
    }

    /// Ends the stream, behind every event it has already written.
    ///
    /// The close travels the data lane with those events. On the control lane
    /// it would overtake them, and the Package would be told the stream ended
    /// before the last events of it arrived.
    pub(crate) async fn close(&self, stream_id: &str, code: &str) -> Result<(), WireFailure> {
        self.writer
            .send_waiting(
                json!({"kind":"stream_close","streamId":stream_id,"code":code}),
                false,
            )
            .await
    }

    /// Records that this stream stops being served now.
    ///
    /// Credit the Package released for events it had already accepted is still
    /// in flight and arrives after the close; from here it is ignored instead
    /// of read as the Package crediting a stream that does not exist.
    pub(crate) async fn retire(&self, stream_id: &str) {
        self.state.lock().await.retire_stream(stream_id);
    }
}

struct QueuedFrame {
    frame: Frame,
    _bytes: OwnedSemaphorePermit,
}

impl Writer {
    async fn control(&self, value: Value) -> Result<(), WireFailure> {
        self.send(value, true).await
    }

    async fn data(&self, value: Value) -> Result<(), WireFailure> {
        self.send(value, false).await
    }

    async fn send(&self, value: Value, control: bool) -> Result<(), WireFailure> {
        let frame =
            Frame::try_from(value).map_err(|error| WireFailure::new(protocol_code(error)))?;
        let size = encode_frame(&frame)
            .map_err(|error| WireFailure::new(protocol_code(error)))?
            .len();
        let permits =
            u32::try_from(size).map_err(|_| WireFailure::new(PackageErrorCode::Overloaded))?;
        let semaphore = if control {
            self.control_bytes.clone()
        } else {
            self.data_bytes.clone()
        };
        let permit = semaphore
            .try_acquire_many_owned(permits)
            .map_err(|_| WireFailure::new(PackageErrorCode::Overloaded))?;
        let queued = QueuedFrame {
            frame,
            _bytes: permit,
        };
        let sender = if control { &self.control } else { &self.data };
        sender.try_send(queued).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => WireFailure::new(PackageErrorCode::Overloaded),
            mpsc::error::TrySendError::Closed(_) => {
                WireFailure::new(PackageErrorCode::ProcessExited)
            }
        })
    }

    /// Queues a frame, waiting for lane capacity instead of refusing it.
    ///
    /// A full lane behind a slow reader is ordinary backpressure, not a
    /// failure: `writer_task` drains it as the child accepts bytes, and a peer
    /// that has stopped reading altogether is caught by the health probe, which
    /// fails the generation and closes these channels. Only a task that owns
    /// its own work may call this; the reader loop must never block on the peer
    /// for longer than one frame it is already answering.
    async fn send_waiting(&self, value: Value, control: bool) -> Result<(), WireFailure> {
        let frame =
            Frame::try_from(value).map_err(|error| WireFailure::new(protocol_code(error)))?;
        let size = encode_frame(&frame)
            .map_err(|error| WireFailure::new(protocol_code(error)))?
            .len();
        let permits =
            u32::try_from(size).map_err(|_| WireFailure::new(PackageErrorCode::Overloaded))?;
        let semaphore = if control {
            self.control_bytes.clone()
        } else {
            self.data_bytes.clone()
        };
        let permit = semaphore
            .acquire_many_owned(permits)
            .await
            .map_err(|_| WireFailure::new(PackageErrorCode::ProcessExited))?;
        let queued = QueuedFrame {
            frame,
            _bytes: permit,
        };
        let sender = if control { &self.control } else { &self.data };
        sender
            .send(queued)
            .await
            .map_err(|_| WireFailure::new(PackageErrorCode::ProcessExited))
    }
}

/// A failed capability call, and whether its state change may still land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CapabilityFailure {
    pub(crate) code: PackageErrorCode,
    pub(crate) outcome_uncertain: bool,
}

impl CapabilityFailure {
    pub(crate) const fn certain(code: PackageErrorCode) -> Self {
        Self {
            code,
            outcome_uncertain: false,
        }
    }
}

/// Injectable protocol bounds. Production uses [`ProtocolLimits::CURRENT`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProtocolLimits {
    pub(crate) capability_call: Duration,
}

impl ProtocolLimits {
    pub(crate) const CURRENT: Self = Self {
        capability_call: Duration::from_secs(10),
    };
}

pub(crate) trait CapabilityHandler: Send + Sync {
    fn call<'a>(
        &'a self,
        grant: Option<&'a str>,
        payload: &'a Value,
        started: Arc<std::sync::atomic::AtomicBool>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, CapabilityFailure>> + Send + 'a>,
    >;

    fn open_stream<'a>(
        &'a self,
        stream_id: String,
        topic: &'a str,
        initial_credit: u64,
        payload: &'a Value,
        output: HostStreamSender,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), PackageErrorCode>> + Send + 'a>,
    >;

    /// Routes one already-classified `credit` frame to its stream.
    ///
    /// It returns nothing: `ContractState` decides whether a credit frame is
    /// legal, late or fatal, and an adapter that answered that question again
    /// is how the two peers came to disagree about the same race.
    fn add_credit<'a>(
        &'a self,
        stream_id: &'a str,
        amount: u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;

    fn close_stream<'a>(
        &'a self,
        stream_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;
}

#[allow(dead_code)]
struct RejectCapabilities;

impl CapabilityHandler for RejectCapabilities {
    fn call<'a>(
        &'a self,
        _grant: Option<&'a str>,
        _payload: &'a Value,
        _started: Arc<std::sync::atomic::AtomicBool>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, CapabilityFailure>> + Send + 'a>,
    > {
        Box::pin(async {
            Err(CapabilityFailure::certain(
                PackageErrorCode::CapabilityRequestInvalid,
            ))
        })
    }

    fn open_stream<'a>(
        &'a self,
        _stream_id: String,
        _topic: &'a str,
        _initial_credit: u64,
        _payload: &'a Value,
        _output: HostStreamSender,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), PackageErrorCode>> + Send + 'a>,
    > {
        Box::pin(async { Err(PackageErrorCode::CapabilityRequestInvalid) })
    }

    fn add_credit<'a>(
        &'a self,
        _stream_id: &'a str,
        _amount: u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }

    fn close_stream<'a>(
        &'a self,
        _stream_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
}

#[allow(dead_code)]
pub(crate) fn start<R, W>(reader: R, output: W) -> ProcessClient
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    start_with_capabilities(
        reader,
        output,
        Arc::new(RejectCapabilities),
        ProtocolLimits::CURRENT,
    )
}

pub(crate) fn start_with_capabilities<R, W>(
    reader: R,
    output: W,
    capabilities: Arc<dyn CapabilityHandler>,
    limits: ProtocolLimits,
) -> ProcessClient
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (control_tx, control_rx) = mpsc::channel(CONTROL_QUEUE);
    let (data_tx, data_rx) = mpsc::channel(DATA_QUEUE);
    let writer = Writer {
        control: control_tx,
        data: data_tx,
        control_bytes: Arc::new(Semaphore::new(CONTROL_BYTES)),
        data_bytes: Arc::new(Semaphore::new(DATA_BYTES)),
    };
    let pending = Arc::new(Mutex::new(BTreeMap::new()));
    let ui_event_routes = Arc::new(Mutex::new(BTreeMap::new()));
    let state = Arc::new(Mutex::new(ContractState::default()));
    let (initialized_tx, initialized_rx) = oneshot::channel();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (pong_tx, pong_rx) = watch::channel(None);
    let (failure_tx, failure_rx) = watch::channel(None);
    tokio::spawn(writer_task(
        output,
        control_rx,
        data_rx,
        state.clone(),
        failure_tx.clone(),
    ));
    tokio::spawn(reader_task(
        reader,
        writer.clone(),
        pending.clone(),
        ui_event_routes.clone(),
        state.clone(),
        initialized_tx,
        shutdown_tx,
        pong_tx,
        failure_tx.clone(),
        capabilities,
        limits,
    ));
    ProcessClient {
        writer,
        pending,
        state,
        ui_event_routes,
        initialized: Arc::new(Mutex::new(Some(initialized_rx))),
        shutdown_ack: shutdown_rx,
        pongs: pong_rx,
        failure: failure_rx,
        failure_sender: failure_tx,
    }
}

async fn writer_task<W>(
    mut output: W,
    mut control: mpsc::Receiver<QueuedFrame>,
    mut data: mpsc::Receiver<QueuedFrame>,
    state: Arc<Mutex<ContractState>>,
    failure: watch::Sender<Option<PackageErrorCode>>,
) where
    W: AsyncWrite + Unpin,
{
    loop {
        let Some(queued) = (tokio::select! {
            biased;
            value = control.recv() => value,
            value = data.recv() => value,
        }) else {
            return;
        };
        let result = async {
            // The Host's own frames go through the same classifier as the
            // Package's, for the state they carry and for the one question it
            // asks. A `credit` or a `cancel` that crossed the state it names is
            // in contract and is written; the Package runs the same classifier
            // and ignores it. Only an out-of-contract frame fails the
            // generation the Host is serving.
            state
                .lock()
                .await
                .observe(Direction::HostToPackage, &queued.frame)
                .map_err(protocol_code)?;
            let bytes = encode_frame(&queued.frame).map_err(protocol_code)?;
            output
                .write_all(&bytes)
                .await
                .map_err(|_| PackageErrorCode::ProcessExited)?;
            output
                .flush()
                .await
                .map_err(|_| PackageErrorCode::ProcessExited)
        }
        .await;
        if let Err(code) = result {
            let _ = failure.send(Some(code));
            return;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn reader_task<R>(
    mut input: R,
    writer: Writer,
    pending: PendingRequests,
    ui_event_routes: UiEventRoutes,
    state: SharedState,
    initialized: oneshot::Sender<Frame>,
    shutdown: watch::Sender<bool>,
    pongs: watch::Sender<Option<String>>,
    failure: watch::Sender<Option<PackageErrorCode>>,
    capabilities: Arc<dyn CapabilityHandler>,
    limits: ProtocolLimits,
) where
    R: AsyncRead + Unpin,
{
    let mut initialized = Some(initialized);
    loop {
        let frame = match read_frame(&mut input).await {
            Ok(frame) => frame,
            Err(code) => {
                let _ = failure.send(Some(code));
                fail_pending(&pending, code).await;
                fail_ui_routes(&ui_event_routes, code).await;
                return;
            }
        };
        if frame.kind() == FrameKind::Event {
            let Some(stream_id) = frame.field("streamId").and_then(Value::as_str) else {
                let _ = failure.send(Some(PackageErrorCode::MalformedFrame));
                fail_ui_routes(&ui_event_routes, PackageErrorCode::MalformedFrame).await;
                return;
            };
            let ui_route = {
                let routes = ui_event_routes.lock().await;
                routes.get(stream_id).cloned()
            };
            if let Some(route) = ui_route {
                let sequence = frame
                    .field("sequence")
                    .and_then(Value::as_str)
                    .and_then(|value| value.parse::<u64>().ok());
                let payload = frame.field("payload").cloned().unwrap_or_else(empty_object);
                let result = match sequence {
                    Some(sequence) => route.event(sequence, payload).await,
                    None => Err(PackageErrorCode::ProtocolViolation),
                };
                if let Err(code) = result {
                    route.terminate(StreamEnd::Host(code));
                    ui_event_routes.lock().await.remove(stream_id);
                    state.lock().await.retire_stream(stream_id);
                    let _ = writer
                        .control(json!({"kind":"stream_close","streamId":stream_id,"code":ui_stream_code(code)}))
                        .await;
                }
                continue;
            }
        }
        // One classifier, before every branch. There is no side of `observe` on
        // which an adapter can hold its own idea of which races are tolerable.
        let verdict = match state.lock().await.observe(Direction::PackageToHost, &frame) {
            Ok(verdict) => verdict,
            Err(error) => {
                let code = protocol_code(error);
                let _ = failure.send(Some(code));
                fail_pending(&pending, code).await;
                fail_ui_routes(&ui_event_routes, code).await;
                return;
            }
        };
        if verdict == Verdict::Ignore {
            continue;
        }
        match frame.kind() {
            FrameKind::Initialized => {
                if let Some(sender) = initialized.take() {
                    let _ = sender.send(frame);
                } else {
                    let _ = failure.send(Some(PackageErrorCode::ProtocolViolation));
                    return;
                }
            }
            FrameKind::Response | FrameKind::Error => {
                let Some(id) = frame.field("id").and_then(Value::as_str).map(str::to_owned) else {
                    let _ = failure.send(Some(PackageErrorCode::MalformedFrame));
                    return;
                };
                let Some(sender) = pending.lock().await.remove(&id) else {
                    let _ = failure.send(Some(PackageErrorCode::ProtocolViolation));
                    return;
                };
                let result = if frame.kind() == FrameKind::Response {
                    Ok(frame.field("payload").cloned().unwrap_or_else(empty_object))
                } else {
                    let mut failure = match frame.field("code").and_then(Value::as_str) {
                        Some("cancelled") => WireFailure::new(PackageErrorCode::Cancelled),
                        Some("overloaded") => WireFailure::new(PackageErrorCode::Overloaded),
                        _ => {
                            let mut failure = WireFailure::new(PackageErrorCode::BackendError);
                            failure.package_error = Some(package_owned_error(&frame));
                            failure
                        }
                    };
                    failure.outcome_uncertain = frame
                        .field("outcomeUncertain")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    Err(failure)
                };
                let _ = sender.send(result);
            }
            FrameKind::Request => {
                let Some(id) = frame.field("id").and_then(Value::as_str).map(str::to_owned) else {
                    let _ = failure.send(Some(PackageErrorCode::MalformedFrame));
                    return;
                };
                if frame.field("operation").and_then(Value::as_str) != Some("capability.call") {
                    let _ = failure.send(Some(PackageErrorCode::ProtocolViolation));
                    return;
                }
                let grant = frame
                    .field("grant")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let payload = frame.field("payload").cloned().unwrap_or_else(empty_object);
                let writer = writer.clone();
                let capabilities = capabilities.clone();
                let failure = failure.clone();
                let answered = id.clone();
                // The method is read before the call, so a call that never
                // returns can still be classified.
                let method = payload
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                tokio::spawn(async move {
                    let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let result = tokio::time::timeout(
                        limits.capability_call,
                        capabilities.call(grant.as_deref(), &payload, started.clone()),
                    )
                    .await
                    .unwrap_or_else(|_| {
                        Err(CapabilityFailure {
                            code: PackageErrorCode::DeadlineExceeded,
                            // A state-changing call whose Core work had already
                            // begun may still commit. `append` is content
                            // idempotent, so the Package can retry it.
                            outcome_uncertain: method == "append"
                                && started.load(std::sync::atomic::Ordering::Acquire),
                        })
                    });
                    let response = match result {
                        Ok(payload) => json!({"kind":"response","id":id,"payload":payload}),
                        Err(failure) => {
                            let mut details = json!({
                                "kind":"error", "id":id,
                                "code":capability_code(failure.code),
                                "retryable":matches!(
                                    failure.code,
                                    PackageErrorCode::Overloaded
                                        | PackageErrorCode::DeadlineExceeded
                                ),
                                "details":{}
                            });
                            if failure.outcome_uncertain
                                && let Some(object) = details.as_object_mut()
                            {
                                object.insert("outcomeUncertain".to_owned(), Value::Bool(true));
                            }
                            details
                        }
                    };
                    // The answer waits for control-lane capacity: a congested
                    // lane is backpressure, and dropping the answer would leave
                    // this request with no terminal frame at all. Congestion
                    // that outlasts the call deadline is answered for that one
                    // request id with a typed retryable error, never by tearing
                    // the generation down.
                    let delivered = match tokio::time::timeout(
                        limits.capability_call,
                        writer.send_waiting(response, true),
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(_) => Err(WireFailure::new(PackageErrorCode::Overloaded)),
                    };
                    if let Err(wire) = delivered {
                        if wire.code == PackageErrorCode::Overloaded {
                            let overloaded = json!({
                                "kind":"error", "id":answered, "code":"overloaded",
                                "retryable":true, "details":{}
                            });
                            if writer.control(overloaded).await.is_err() {
                                let _ = failure.send(Some(PackageErrorCode::ProcessExited));
                            }
                        } else {
                            let _ = failure.send(Some(PackageErrorCode::ProcessExited));
                        }
                    }
                });
            }
            FrameKind::StreamOpen => {
                let Some(id) = frame.field("id").and_then(Value::as_str).map(str::to_owned) else {
                    let _ = failure.send(Some(PackageErrorCode::MalformedFrame));
                    return;
                };
                let topic = frame
                    .field("topic")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let initial_credit = frame
                    .field("initialCredit")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                let payload = frame.field("payload").cloned().unwrap_or_else(empty_object);
                if let Err(code) = capabilities
                    .open_stream(
                        id.clone(),
                        topic,
                        initial_credit,
                        &payload,
                        HostStreamSender {
                            writer: writer.clone(),
                            state: state.clone(),
                        },
                    )
                    .await
                {
                    let response = json!({
                        "kind":"stream_close", "streamId":id,
                        "code":capability_code(code)
                    });
                    // The Package is waiting for this stream to end, so the
                    // rejection waits for lane capacity rather than being
                    // dropped with the generation reported as exited.
                    if writer.send_waiting(response, true).await.is_err() {
                        let _ = failure.send(Some(PackageErrorCode::ProcessExited));
                        return;
                    }
                }
            }
            FrameKind::Credit => {
                let Some(stream_id) = frame.field("streamId").and_then(Value::as_str) else {
                    let _ = failure.send(Some(PackageErrorCode::MalformedFrame));
                    return;
                };
                let amount = frame
                    .field("amount")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                // The classifier has already decided this frame: it names a
                // live stream and sits inside the window the contract grants.
                // What is left is routing it to the stream's own task.
                capabilities.add_credit(stream_id, amount).await;
            }
            FrameKind::StreamClose => {
                let Some(stream_id) = frame.field("streamId").and_then(Value::as_str) else {
                    let _ = failure.send(Some(PackageErrorCode::MalformedFrame));
                    return;
                };
                if let Some(route) = ui_event_routes.lock().await.remove(stream_id) {
                    route.terminate(stream_end(&frame));
                } else {
                    capabilities.close_stream(stream_id).await;
                }
            }
            FrameKind::ShutdownAck => {
                let _ = shutdown.send(true);
            }
            FrameKind::Pong => {
                let nonce = frame
                    .field("nonce")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let _ = pongs.send(nonce);
            }
            FrameKind::Event => {}
            _ => {
                let _ = failure.send(Some(PackageErrorCode::ProtocolViolation));
                return;
            }
        }
    }
}

/// Classifies a Package `stream_close`.
///
/// No code, `completed` or `closed` is an ordinary completion; `cancelled` and `overloaded` stay
/// host codes; every other code is the Package's own termination code.
fn stream_end(frame: &Frame) -> StreamEnd {
    match frame.field("code").and_then(Value::as_str) {
        None | Some("completed") | Some("closed") => StreamEnd::Completed,
        Some("cancelled") => StreamEnd::Host(PackageErrorCode::Cancelled),
        Some("overloaded") => StreamEnd::Host(PackageErrorCode::Overloaded),
        Some(code) => StreamEnd::Package(PackageOwnedError {
            code: code.to_owned(),
            retryable: false,
            details: serde_json::Map::new(),
        }),
    }
}

async fn fail_ui_routes(routes: &UiEventRoutes, code: PackageErrorCode) {
    let mut routes = routes.lock().await;
    for (_, route) in std::mem::take(&mut *routes) {
        route.terminate(StreamEnd::Host(code));
    }
}

async fn fail_pending(
    pending: &Mutex<BTreeMap<String, oneshot::Sender<Result<Value, WireFailure>>>>,
    code: PackageErrorCode,
) {
    let mut pending = pending.lock().await;
    for (_, sender) in std::mem::take(&mut *pending) {
        let _ = sender.send(Err(WireFailure::new(code)));
    }
}

async fn read_frame<R: AsyncRead + Unpin>(input: &mut R) -> Result<Frame, PackageErrorCode> {
    let mut prefix = [0_u8; 4];
    input
        .read_exact(&mut prefix)
        .await
        .map_err(|_| PackageErrorCode::ProcessExited)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length > MAX_FRAME_BYTES {
        return Err(PackageErrorCode::FrameTooLarge);
    }
    if length == 0 {
        return Err(PackageErrorCode::MalformedFrame);
    }
    let mut payload = vec![0; length];
    input
        .read_exact(&mut payload)
        .await
        .map_err(|_| PackageErrorCode::ProcessExited)?;
    let mut bytes = prefix.to_vec();
    bytes.extend(payload);
    decode_frame(&bytes).map_err(protocol_code)
}

fn ui_stream_code(code: PackageErrorCode) -> &'static str {
    match code {
        PackageErrorCode::UiContractViolation => "ui_contract_violation",
        PackageErrorCode::EventLagged => "event_lagged",
        _ => "protocol_violation",
    }
}

fn capability_code(code: PackageErrorCode) -> &'static str {
    match code {
        PackageErrorCode::CapabilityUnavailable => "capability_unavailable",
        PackageErrorCode::CapabilityVersionUnsupported => "capability_version_unsupported",
        PackageErrorCode::DeadlineExceeded => "deadline_exceeded",
        PackageErrorCode::Overloaded => "overloaded",
        _ => "capability_request_invalid",
    }
}

fn protocol_code(error: teamarness_package_protocol::ProtocolError) -> PackageErrorCode {
    match error.code() {
        "frame_too_large" => PackageErrorCode::FrameTooLarge,
        "overloaded" => PackageErrorCode::Overloaded,
        "malformed_frame" | "malformed_json" | "invalid_utf8" | "zero_length" => {
            PackageErrorCode::MalformedFrame
        }
        _ => PackageErrorCode::ProtocolViolation,
    }
}

impl ProcessClient {
    pub(crate) async fn initialize(&self, value: Value) -> Result<Frame, WireFailure> {
        self.writer.control(value).await?;
        let receiver = self
            .initialized
            .lock()
            .await
            .take()
            .ok_or_else(|| WireFailure::new(PackageErrorCode::HandshakeMismatch))?;
        tokio::time::timeout(Duration::from_secs(5), receiver)
            .await
            .map_err(|_| WireFailure::new(PackageErrorCode::HandshakeTimeout))?
            .map_err(|_| WireFailure::new(PackageErrorCode::ProcessExited))
    }

    pub(crate) async fn request(
        &self,
        id: String,
        operation: &str,
        mount: Option<&str>,
        payload: PackageValue,
        deadline: Duration,
        state_changing: bool,
    ) -> Result<Value, WireFailure> {
        if self.pending.lock().await.len() >= MAX_IN_FLIGHT_EACH_DIRECTION {
            return Err(WireFailure::new(PackageErrorCode::Overloaded));
        }
        let (sender, mut receiver) = oneshot::channel();
        self.pending.lock().await.insert(id.clone(), sender);
        let deadline_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .saturating_add(deadline.as_millis())
            .to_string();
        let mut value = json!({
            "kind":"request", "id":id, "operation":operation,
            "deadlineUnixMs":deadline_unix_ms,
            "payload": package_value_to_json(payload)
        });
        if let Some(mount) = mount {
            value["mountId"] = Value::String(mount.to_owned());
        }
        self.writer.data(value).await?;
        match tokio::time::timeout(deadline, &mut receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(WireFailure::new(PackageErrorCode::ProcessExited)),
            Err(_) => {
                self.send_cancel(&id).await?;
                match tokio::time::timeout(Duration::from_millis(500), &mut receiver).await {
                    Ok(Ok(result)) => result,
                    _ => {
                        self.pending.lock().await.remove(&id);
                        // The `cancel` already entered the tombstone when the
                        // writer observed it; this covers the case where the
                        // grace expired before it was written.
                        self.state.lock().await.retire_request(&id);
                        let mut failure = WireFailure::new(if state_changing {
                            PackageErrorCode::OutcomeUncertain
                        } else {
                            PackageErrorCode::DeadlineExceeded
                        });
                        failure.outcome_uncertain = state_changing;
                        Err(failure)
                    }
                }
            }
        }
    }

    /// Cancels one in-flight request.
    ///
    /// The Host stops waiting here, so the request is retired here too: a first
    /// terminal frame that crossed the cancel is ignored, and a second is still
    /// fatal. A repeated cancel, or one for a request the answer already
    /// retired, is ignored by the same classifier when the writer observes it.
    pub(crate) async fn cancel(&self, target: &str) -> Result<(), WireFailure> {
        let removed = self.pending.lock().await.remove(target);
        if let Some(sender) = removed {
            let _ = sender.send(Err(WireFailure::new(PackageErrorCode::Cancelled)));
        }
        self.send_cancel(target).await
    }

    async fn send_cancel(&self, target: &str) -> Result<(), WireFailure> {
        self.writer
            .data(json!({
                "kind":"cancel", "id":format!("h:cancel-{}", target.trim_start_matches("h:")),
                "targetId":target
            }))
            .await
    }

    pub(crate) async fn open_ui_stream(
        &self,
        id: String,
        topic: &str,
        mount: &str,
        payload: PackageValue,
        contract: Arc<AdmittedUi>,
    ) -> Result<(UiEventReceiver, Arc<UiEventRoute>), WireFailure> {
        let (route, receiver) = UiEventRoute::new(contract, topic.to_owned());
        self.ui_event_routes
            .lock()
            .await
            .insert(id.clone(), route.clone());
        if let Err(error) = self
            .writer
            .control(json!({
                "kind":"stream_open", "id":id, "topic":topic, "mountId":mount,
                "initialCredit":32, "payload":package_value_to_json(payload)
            }))
            .await
        {
            self.ui_event_routes.lock().await.remove(&id);
            route.terminate(StreamEnd::Host(error.code));
            return Err(error);
        }
        Ok((receiver, route))
    }

    /// Releases credit for events the subscriber has acknowledged.
    ///
    /// A stream that ended while the acknowledgement was in flight makes this
    /// frame late, not wrong: the writer's classifier drops it.
    pub(crate) async fn credit(&self, stream_id: &str, amount: u64) -> Result<(), WireFailure> {
        self.writer
            .control(json!({
                "kind":"credit", "streamId":stream_id,
                "amount":amount.min(INITIAL_STREAM_CREDIT as u64)
            }))
            .await
    }

    pub(crate) async fn stream_close(&self, stream_id: &str) -> Result<(), WireFailure> {
        // The Host stops serving the stream here, so credit the Package
        // released for events it had already accepted is ignored from now on.
        self.state.lock().await.retire_stream(stream_id);
        if let Some(route) = self.ui_event_routes.lock().await.remove(stream_id) {
            route.terminate(StreamEnd::Host(PackageErrorCode::Cancelled));
        }
        self.writer
            .control(json!({"kind":"stream_close","streamId":stream_id,"code":"closed"}))
            .await
    }

    pub(crate) async fn shutdown(&self) {
        let _ = self
            .writer
            .control(json!({"kind":"shutdown","reason":"application_shutdown"}))
            .await;
        let mut ack = self.shutdown_ack.clone();
        if *ack.borrow() {
            return;
        }
        let _ = tokio::time::timeout(Duration::from_secs(3), ack.changed()).await;
    }

    pub(crate) fn failure(&self) -> Option<PackageErrorCode> {
        *self.failure.borrow()
    }

    pub(crate) async fn wait_for_failure(&self) -> PackageErrorCode {
        let mut failure = self.failure.clone();
        loop {
            if let Some(code) = *failure.borrow() {
                return code;
            }
            if failure.changed().await.is_err() {
                return PackageErrorCode::ProcessExited;
            }
        }
    }

    pub(crate) async fn health_probe(&self) -> Result<(), WireFailure> {
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random)
            .map_err(|_| WireFailure::new(PackageErrorCode::DeadlineExceeded))?;
        use base64::Engine as _;
        let nonce = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random);
        let mut pongs = self.pongs.clone();
        self.writer
            .control(json!({"kind":"ping","nonce":nonce}))
            .await?;
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if pongs.borrow().as_deref() == Some(nonce.as_str()) {
                    return;
                }
                if pongs.changed().await.is_err() {
                    return;
                }
            }
        })
        .await;
        if result.is_err() {
            let _ = self
                .failure_sender
                .send(Some(PackageErrorCode::DeadlineExceeded));
            return Err(WireFailure::new(PackageErrorCode::DeadlineExceeded));
        }
        Ok(())
    }
}

fn package_value_to_json(value: PackageValue) -> Value {
    serde_json::to_value(value).unwrap_or_else(|_| empty_object())
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

#[cfg(test)]
mod tests {
    use super::{
        CONTROL_BYTES, CONTROL_QUEUE, CONTROL_RESERVE, DATA_BYTES, DATA_QUEUE, StreamEnd,
        UI_EVENT_CAPACITY, package_owned_error, stream_end,
    };
    use crate::PackageErrorCode;
    use serde_json::{Value, json};
    use teamarness_package_protocol::{
        Frame, INITIAL_STREAM_CREDIT, MAX_FRAME_BYTES, MAX_IN_FLIGHT_EACH_DIRECTION,
        MAX_WRITER_BYTES, MAX_WRITER_FRAMES,
    };

    /// Every writer bound admits the worst case the contract permits.
    ///
    /// These are the floors the lanes were sized below: a control lane that
    /// cannot hold the answers to the requests the contract allows turns legal
    /// traffic into a refused frame.
    #[test]
    fn writer_lanes_admit_the_traffic_the_contract_permits() {
        const {
            assert!(CONTROL_QUEUE >= MAX_IN_FLIGHT_EACH_DIRECTION + CONTROL_RESERVE);
            assert!(DATA_QUEUE >= MAX_IN_FLIGHT_EACH_DIRECTION);
            assert!(CONTROL_QUEUE <= MAX_WRITER_FRAMES);
            assert!(DATA_QUEUE <= MAX_WRITER_FRAMES);
            assert!(CONTROL_BYTES >= 2 * MAX_FRAME_BYTES);
            assert!(DATA_BYTES >= 2 * MAX_FRAME_BYTES);
            assert!(CONTROL_BYTES + DATA_BYTES == MAX_WRITER_BYTES);
            assert!(UI_EVENT_CAPACITY == INITIAL_STREAM_CREDIT);
        }
    }

    fn frame(value: Value) -> Frame {
        Frame::try_from(value).expect("valid frame")
    }

    fn error_frame(details: Value) -> Frame {
        frame(json!({
            "kind": "error",
            "id": "h:error-1",
            "code": "fixture.rejected",
            "retryable": true,
            "details": details,
        }))
    }

    #[test]
    fn package_error_details_over_bound_are_dropped() {
        let error = package_owned_error(&error_frame(json!({"big": "x".repeat(5 * 1024)})));
        assert_eq!(error.code, "fixture.rejected");
        assert!(error.retryable);
        assert!(error.details.is_empty());
    }

    #[test]
    fn package_error_details_too_deep_are_dropped() {
        let mut nested = json!("leaf");
        for _ in 0..9 {
            nested = json!({ "next": nested });
        }
        let error = package_owned_error(&error_frame(nested));
        assert_eq!(error.code, "fixture.rejected");
        assert!(error.details.is_empty());
    }

    #[test]
    fn package_error_details_within_bounds_survive() {
        let error = package_owned_error(&error_frame(json!({"field": "value"})));
        assert_eq!(error.details["field"], json!("value"));
    }

    #[test]
    fn cancelled_and_overloaded_stay_host_codes() {
        for (code, expected) in [
            ("cancelled", PackageErrorCode::Cancelled),
            ("overloaded", PackageErrorCode::Overloaded),
        ] {
            let closed = frame(json!({"kind": "stream_close", "streamId": "h:s-1", "code": code}));
            assert_eq!(stream_end(&closed), StreamEnd::Host(expected));
        }
        for code in ["completed", "closed"] {
            let closed = frame(json!({"kind": "stream_close", "streamId": "h:s-1", "code": code}));
            assert_eq!(stream_end(&closed), StreamEnd::Completed);
        }
        let closed = frame(json!({"kind": "stream_close", "streamId": "h:s-1"}));
        assert_eq!(stream_end(&closed), StreamEnd::Completed);
        let closed =
            frame(json!({"kind": "stream_close", "streamId": "h:s-1", "code": "fixture.rejected"}));
        let StreamEnd::Package(error) = stream_end(&closed) else {
            panic!("a Package code is Package-owned");
        };
        assert_eq!(error.code, "fixture.rejected");
        assert!(!error.retryable);
    }
}
