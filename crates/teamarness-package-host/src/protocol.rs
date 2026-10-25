use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use teamarness_package_protocol::{
    ContractState, Direction, Frame, FrameKind, INITIAL_STREAM_CREDIT, MAX_FRAME_BYTES,
    MAX_ID_BYTES, MAX_IN_FLIGHT_EACH_DIRECTION, MAX_WRITER_BYTES, MAX_WRITER_FRAMES, Verdict,
    decode_frame, encode_frame,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex, MutexGuard, OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch};

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
/// The route's byte budget.
///
/// Worst case: every event the credit window admits, each at the largest UI
/// payload the UI contract allows, queued at once. The classifier is what
/// bounds the count, so a budget derived from anything else would refuse an
/// event the classifier had already admitted.
const UI_EVENT_BYTES: usize = INITIAL_STREAM_CREDIT * MAX_UI_PAYLOAD_BYTES;
// The fixed transport envelope adds only its field names and punctuation to a Port value.
const PORT_ENVELOPE_OVERHEAD: usize =
    r#"{"$teamarnessPortEnvelope":1,"$teamarnessPortValue":}"#.len();

/// Events the Shell acknowledges with one `ack`.
///
/// `apps/teamarness-desktop/src/package-frame/port-client.ts` posts one `ack`
/// for every event it hands its listener, and
/// `apps/teamarness-desktop/src/package-frame/protocol.ts` publishes that
/// granularity as `EVENTS_PER_ACKNOWLEDGEMENT`.
/// `scripts/check-protocol-bounds.mjs` reads both, so a Shell that started
/// batching acknowledgements would have to move this constant with it.
///
/// This is the behaviour the control lane's worst case is measured in, and
/// until this round it existed only in TypeScript. A bound derived from the
/// protocol's vocabulary instead — "one `credit` per subscription" — was wrong
/// by a factor of 32 and no floor could say so.
pub(crate) const EVENTS_PER_ACKNOWLEDGEMENT: usize = 1;

/// `credit` frames one subscription can owe at once.
///
/// Not one. A subscription does not hold one `credit` outstanding; it holds as
/// many as its window has events the subscriber has read and not yet had
/// credited, which is the whole window when the Shell acknowledges one event
/// at a time.
const CREDIT_FRAMES_PER_SUBSCRIPTION: usize =
    INITIAL_STREAM_CREDIT.div_ceil(EVENTS_PER_ACKNOWLEDGEMENT);

/// Every control frame the Host can owe at once, counted from the behaviour
/// that produces them rather than from the protocol's vocabulary.
///
/// `initialize`, `ping` and `shutdown` are one frame each; every subscription
/// one mount may hold open can owe a `stream_open`, a `stream_close` and a
/// whole window of `credit`.
const CONTROL_WORST_CASE: usize =
    3 + MAX_UI_SUBSCRIPTIONS_PER_MOUNT * (2 + CREDIT_FRAMES_PER_SUBSCRIPTION);

/// Whether `ProcessClient::credit` waits for a control slot instead of
/// refusing when the lane is full.
///
/// `CONTROL_WORST_CASE` is 275 against the 64 queued frames the contract
/// publishes for the whole writer, so no split of that budget admits it and
/// the lane cannot be widened to satisfy the floor below. Waiting is the only
/// way the worst case is served at all.
/// `a_full_control_lane_never_destroys_acknowledged_credit` is what proves
/// this claim is true of the code rather than of this comment.
const CREDIT_WAITS_FOR_A_SLOT: bool = true;

/// Control frames a *refusing* send may have outstanding.
///
/// `initialize`, `ping` and `shutdown` are one frame each, and one
/// `stream_open` and one `stream_close` may be queued for each subscription a
/// mount may hold open. `credit` is not counted here: it waits, because the
/// window it releases does not fit any lane.
const CONTROL_RESERVE: usize = 3 + 2 * MAX_UI_SUBSCRIPTIONS_PER_MOUNT;

/// Slots on the data lane.
///
/// Worst case: every in-flight request the contract permits, queued at once.
/// The one `cancel` that may follow a request is sent after that request's
/// deadline expires, by which time its frame has left the lane.
const DATA_QUEUE: usize = MAX_IN_FLIGHT_EACH_DIRECTION;

/// Slots on the control lane: the rest of the writer's published frame budget.
///
/// The two lanes share the one writer budget the contract publishes and the
/// handshake transmits, so their depths are a split of `MAX_WRITER_FRAMES`,
/// exactly as their byte budgets are a split of `MAX_WRITER_BYTES`. The data
/// lane takes its own worst case and the control lane keeps the rest, which
/// must still cover `CONTROL_RESERVE` — the frames a refusing send queues, and
/// a lane shallower than them turns legal traffic into a refused frame. A
/// capability answer needs no slot of its own here, and neither does `credit`:
/// both wait, because the tasks that write them own their work.
const CONTROL_QUEUE: usize = MAX_WRITER_FRAMES - DATA_QUEUE;

/// The control lane's shared byte budget: two contract-maximum frames, so one
/// maximum answer never waits on another that is already queued.
const CONTROL_BYTES: usize = 2 * MAX_FRAME_BYTES;

/// One typed terminal error frame: `error`, an id of the schema's maximum
/// length, a stable code, `retryable`, and an empty details object, plus the
/// length prefix. `an_overloaded_answer_fits_its_reserve` is the floor.
const TERMINAL_ERROR_BYTES: usize = MAX_ID_BYTES + 128;

/// Control-lane bytes only a request's typed terminal answer may use.
///
/// Rule 1's third verdict has to be deliverable under exactly the saturation
/// that produces it. An `overloaded` answer that competed for the budget whose
/// exhaustion triggered it would be refused precisely when it is owed, so one
/// frame is reserved for each in-flight request the contract permits.
const TERMINAL_RESERVE_BYTES: usize = MAX_IN_FLIGHT_EACH_DIRECTION * TERMINAL_ERROR_BYTES;

/// The data lane keeps the rest of the writer budget the contract publishes.
pub(crate) const DATA_BYTES: usize = MAX_WRITER_BYTES - CONTROL_BYTES - TERMINAL_RESERVE_BYTES;

// Rule 2's floors for this crate, asserted beside the bounds they guard so a
// bound that stops admitting its worst case fails the build rather than a
// suite. ADR 0019 §3 names them.
//
// Each left-hand side is written from the traffic, independently of the bound
// it guards. A floor that restates its own bound's definition cannot fail on a
// change to any input, and that is how the control lane came to be sized for
// one `credit` per subscription while the Shell acknowledged one event at a
// time: the floor re-read the same wrong worst case and agreed with it.
const _: () = {
    // The anchor. Its left-hand side names `INITIAL_STREAM_CREDIT` through
    // `CREDIT_FRAMES_PER_SUBSCRIPTION`, so a change to the credit window moves
    // it, and it cannot be satisfied by widening the lane: 275 does not fit
    // the 64 frames the contract publishes for the whole writer. The only way
    // to meet it is the waiting send, which
    // `a_full_control_lane_never_destroys_acknowledged_credit` proves is what
    // the code does.
    assert!(CONTROL_QUEUE >= CONTROL_WORST_CASE || CREDIT_WAITS_FOR_A_SLOT);
    // The frames a refusing send queues must fit the lane they refuse on.
    // `CONTROL_RESERVE` is counted from those senders, not from
    // `CONTROL_QUEUE`, so a ninth subscription per mount fails here.
    assert!(CONTROL_QUEUE >= CONTROL_RESERVE);
    assert!(DATA_QUEUE >= MAX_IN_FLIGHT_EACH_DIRECTION);
    // Both lanes' independently counted worst cases must fit the one budget
    // the handshake publishes. This is the assertion an SF2-001 recurrence
    // breaks: each lane sized against the contract's concurrency on its own,
    // together past the budget.
    assert!(DATA_QUEUE + CONTROL_RESERVE <= MAX_WRITER_FRAMES);
    assert!(CONTROL_QUEUE + DATA_QUEUE == MAX_WRITER_FRAMES);
    assert!(CONTROL_BYTES >= 2 * MAX_FRAME_BYTES);
    assert!(DATA_BYTES >= 2 * MAX_FRAME_BYTES);
    assert!(TERMINAL_RESERVE_BYTES == MAX_IN_FLIGHT_EACH_DIRECTION * TERMINAL_ERROR_BYTES);
    assert!(CONTROL_BYTES + TERMINAL_RESERVE_BYTES + DATA_BYTES == MAX_WRITER_BYTES);
};

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
    contract: EventContract,
    operation: String,
    lease: Option<Arc<PortStreamLease>>,
}

enum EventContract {
    Ui(Arc<AdmittedUi>),
    Port {
        schemas: Arc<teamarness_package_contract::port_contract::CompiledPortContracts>,
        contract: String,
    },
}

/// Enqueue and revocation share a lock; only already accepted events may drain.
pub(crate) struct PortStreamLease {
    state: std::sync::Mutex<Option<PackageErrorCode>>,
    terminal: watch::Sender<Option<PackageErrorCode>>,
}
impl Default for PortStreamLease {
    fn default() -> Self {
        Self {
            state: std::sync::Mutex::new(None),
            terminal: watch::channel(None).0,
        }
    }
}
impl PortStreamLease {
    pub(crate) fn watch(&self) -> watch::Receiver<Option<PackageErrorCode>> {
        self.terminal.subscribe()
    }
    pub(crate) fn enqueue<T>(&self, work: impl FnOnce() -> T) -> Result<T, PackageErrorCode> {
        let state = self.state.lock().expect("subscription lease poisoned");
        if let Some(code) = *state {
            return Err(code);
        }
        Ok(work())
    }
    pub(crate) fn revoke(&self, code: PackageErrorCode) {
        let mut state = self.state.lock().expect("subscription lease poisoned");
        if state.is_none() {
            *state = Some(code);
            self.terminal.send_replace(Some(code));
        }
    }
}

pub(crate) struct PortQueryStream {
    pub(crate) generation: u64,
    pub(crate) receiver: UiEventReceiver,
    pub(crate) client: ProcessClient,
    pub(crate) id: String,
    pub(crate) _route: Box<dyn Send + Sync>,
    pub(crate) _provider: OwnedSemaphorePermit,
}

/// What the route owes its subscriber, and nothing about the wire.
///
/// Sequence and credit are the classifier's decisions. These two counters
/// answer the one question the classifier cannot: how far the subscriber has
/// read, which is what an acknowledgement is measured against.
struct UiEventRouteState {
    highest_delivered: u64,
    highest_acknowledged: u64,
}

impl UiEventReceiver {
    pub(crate) fn revocation(&self) -> Option<watch::Receiver<Option<PackageErrorCode>>> {
        self.route.lease.as_ref().map(|lease| lease.watch())
    }

    pub(crate) fn terminal(&self) -> watch::Receiver<Option<StreamEnd>> {
        self.terminal.clone()
    }

    pub(crate) async fn changed(&mut self) -> Result<UiEvent, StreamEnd> {
        let mut revocation = self.route.lease.as_ref().map(|lease| lease.watch());
        loop {
            if let Some(code) = revocation.as_ref().and_then(|watch| *watch.borrow()) {
                self.route.terminate(StreamEnd::Host(code));
            }
            let terminal = self.terminal.borrow().clone();
            if let Some(end) = terminal {
                // A Package-owned end, like a completion, drains what the Package already sent.
                if matches!(end, StreamEnd::Completed | StreamEnd::Package(_))
                    || matches!(self.route.contract, EventContract::Port { .. })
                {
                    match self.events.try_recv() {
                        Ok(event) => {
                            self.route.mark_delivered(event.sequence).await;
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
                _ = async {
                    if let Some(watch) = &mut revocation { let _ = watch.changed().await; }
                    else { std::future::pending::<()>().await; }
                } => {},
                result = self.terminal.changed() => {
                    if result.is_err() {
                        return Err(StreamEnd::Host(PackageErrorCode::ProcessExited));
                    }
                }
                event = self.events.recv() => {
                    let event = event.ok_or(StreamEnd::Host(PackageErrorCode::ProcessExited))?;
                    self.route.mark_delivered(event.sequence).await;
                    return Ok(event);
                }
            }
        }
    }
}

impl UiEventRoute {
    fn new_port(
        schemas: Arc<teamarness_package_contract::port_contract::CompiledPortContracts>,
        contract: String,
        operation: String,
        lease: Option<Arc<PortStreamLease>>,
    ) -> (Arc<Self>, UiEventReceiver) {
        Self::new_internal(EventContract::Port { schemas, contract }, operation, lease)
    }

    fn new(contract: Arc<AdmittedUi>, operation: String) -> (Arc<Self>, UiEventReceiver) {
        Self::new_internal(EventContract::Ui(contract), operation, None)
    }

    fn new_internal(
        contract: EventContract,
        operation: String,
        lease: Option<Arc<PortStreamLease>>,
    ) -> (Arc<Self>, UiEventReceiver) {
        let (events, receiver) = mpsc::channel(UI_EVENT_CAPACITY);
        let (terminal, terminal_receiver) = watch::channel(None);
        let port_overhead = if matches!(&contract, EventContract::Port { .. }) {
            INITIAL_STREAM_CREDIT * PORT_ENVELOPE_OVERHEAD
        } else {
            0
        };
        let route = Arc::new(Self {
            events,
            terminal,
            bytes: Arc::new(Semaphore::new(UI_EVENT_BYTES + port_overhead)),
            state: Mutex::new(UiEventRouteState {
                highest_delivered: 0,
                highest_acknowledged: 0,
            }),
            contract,
            operation,
            lease,
        });
        let event_receiver = UiEventReceiver {
            events: receiver,
            terminal: terminal_receiver,
            route: route.clone(),
        };
        (route, event_receiver)
    }

    /// Hands the classifier's next event to the subscriber.
    ///
    /// The route decides nothing about the wire. The classifier has already
    /// ruled that this event names a live stream, carries the next sequence and
    /// sits inside the window the contract grants; what is left is the UI
    /// contract the payload must satisfy and the bounds this queue holds, both
    /// of which end this one stream rather than the generation.
    ///
    /// The bounds cannot be reached by a conforming Package: the queue and its
    /// byte budget both hold the whole credit window, and credit returns only
    /// for an event the subscriber has acknowledged.
    fn event(&self, sequence: u64, value: Value) -> Result<(), PackageErrorCode> {
        if let Some(lease) = &self.lease {
            return lease.enqueue(|| self.enqueue_event(sequence, value))?;
        }
        self.enqueue_event(sequence, value)
    }

    fn enqueue_event(&self, sequence: u64, value: Value) -> Result<(), PackageErrorCode> {
        let payload: PackageValue = serde_json::from_value(value.clone())
            .map_err(|_| PackageErrorCode::UiContractViolation)?;
        match &self.contract {
            EventContract::Ui(contract) => contract
                .contract
                .validate_event(&self.operation, &payload)
                .map_err(|_| PackageErrorCode::UiContractViolation)?,
            EventContract::Port { schemas, contract } => schemas
                .validate(
                    contract,
                    &self.operation,
                    "event",
                    decode_port_value(&value),
                )
                .map_err(|_| PackageErrorCode::PortEventInvalid)?,
        }
        let encoded =
            serde_json::to_vec(&payload).map_err(|_| PackageErrorCode::UiContractViolation)?;
        let (logical_bytes, wire_limit) = if matches!(&self.contract, EventContract::Port { .. }) {
            (
                serde_json::to_vec(decode_port_value(&value))
                    .map_err(|_| PackageErrorCode::PortEventInvalid)?
                    .len(),
                MAX_UI_PAYLOAD_BYTES + PORT_ENVELOPE_OVERHEAD,
            )
        } else {
            (encoded.len(), MAX_UI_PAYLOAD_BYTES)
        };
        if logical_bytes > MAX_UI_PAYLOAD_BYTES || encoded.len() > wire_limit {
            return Err(match &self.contract {
                EventContract::Ui(_) => PackageErrorCode::UiContractViolation,
                EventContract::Port { .. } => PackageErrorCode::PortEventInvalid,
            });
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
        Ok(())
    }

    /// Records how far the subscriber has read.
    ///
    /// The queue is first in, first out and the classifier admits one sequence
    /// at a time, so this is a record, not a check.
    async fn mark_delivered(&self, sequence: u64) {
        self.state.lock().await.highest_delivered = sequence;
    }

    /// Counts the credit an acknowledgement releases, without recording it.
    ///
    /// The record only moves when the `credit` frame that accounts for it is
    /// on the wire. Advancing first is what made a refused frame permanent:
    /// the Host believed it had released credit the Package never received, a
    /// repeat was refused by this method's own first guard, and the only other
    /// producer of `credit` is this one path. The window stayed shut and
    /// nothing said so.
    ///
    /// The guard holds the route's state for the length of the send, so one
    /// stream's acknowledgements reach the wire in the order they were counted.
    /// That wait is unbounded from this method's side — the health probe, not
    /// the writer, bounds it — which widens the window in which a second
    /// acknowledgement for the same subscription takes the lock first.
    ///
    /// `None` is the ignore verdict: this acknowledgement names events the
    /// route has already credited. Each acknowledgement is its own Tauri task
    /// and the Shell does not await them, so two are routinely in flight and
    /// the earlier one can arrive behind the later. The contract calls that
    /// ordinary in-flight traffic — credit for events already accepted is not a
    /// violation — and the verdict table ignores a frame whose state the
    /// receiver has moved past. It releases no credit, because crediting the
    /// same events twice is what would carry the stream past its window.
    ///
    /// Fatality stays where the contract puts it: an acknowledgement for events
    /// this route never delivered names state that never existed, and one
    /// reaching past the window is a budget violation.
    pub(crate) async fn acknowledge(
        &self,
        through: u64,
    ) -> Result<Option<PendingAcknowledgement<'_>>, WireFailure> {
        let state = self.state.lock().await;
        if through > state.highest_delivered
            || through.saturating_sub(state.highest_acknowledged) > UI_EVENT_CAPACITY as u64
        {
            return Err(WireFailure::new(PackageErrorCode::ProtocolViolation));
        }
        if through <= state.highest_acknowledged {
            return Ok(None);
        }
        let amount = through - state.highest_acknowledged;
        Ok(Some(PendingAcknowledgement {
            state,
            through,
            amount,
        }))
    }

    fn terminate(&self, end: StreamEnd) {
        self.terminal.send_replace(Some(end));
    }
}

/// Credit an acknowledgement has released but whose frame is not yet queued.
///
/// Dropping it without [`PendingAcknowledgement::commit`] leaves the route
/// exactly where it was, so the next acknowledgement covers the same events
/// again and the credit is not lost.
pub(crate) struct PendingAcknowledgement<'route> {
    state: MutexGuard<'route, UiEventRouteState>,
    through: u64,
    amount: u64,
}

impl PendingAcknowledgement<'_> {
    /// Queues this acknowledgement's `credit` frame, then records it.
    ///
    /// The send is taken rather than the amount handed out, so the order
    /// cannot be got wrong at a call site: there is no way to reach the amount
    /// without giving this method the send it accounts for, and the record
    /// moves only after that send has returned `Ok`.
    pub(crate) async fn commit<Send, Sent>(mut self, send: Send) -> Result<(), WireFailure>
    where
        Send: FnOnce(u64) -> Sent,
        Sent: Future<Output = Result<(), WireFailure>>,
    {
        send(self.amount).await?;
        self.state.highest_acknowledged = self.through;
        Ok(())
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
    terminal_bytes: Arc<Semaphore>,
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

    /// Queues one request's typed terminal error on its own reserve.
    ///
    /// The shared control budget is what the answer this replaces could not
    /// get, so this frame never asks for it. The reserve holds one such frame
    /// for every in-flight request the contract permits, which is the most
    /// that can be owed at once.
    async fn send_terminal_error(&self, value: Value) -> Result<(), WireFailure> {
        let frame =
            Frame::try_from(value).map_err(|error| WireFailure::new(protocol_code(error)))?;
        let size = encode_frame(&frame)
            .map_err(|error| WireFailure::new(protocol_code(error)))?
            .len();
        let permits =
            u32::try_from(size).map_err(|_| WireFailure::new(PackageErrorCode::Overloaded))?;
        let permit = self
            .terminal_bytes
            .clone()
            .acquire_many_owned(permits)
            .await
            .map_err(|_| WireFailure::new(PackageErrorCode::ProcessExited))?;
        self.control
            .send(QueuedFrame {
                frame,
                _bytes: permit,
            })
            .await
            .map_err(|_| WireFailure::new(PackageErrorCode::ProcessExited))
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CapabilityFailure {
    pub(crate) code: PackageErrorCode,
    pub(crate) outcome_uncertain: bool,
    pub(crate) package_error: Option<PackageOwnedError>,
}

impl CapabilityFailure {
    pub(crate) const fn certain(code: PackageErrorCode) -> Self {
        Self {
            code,
            outcome_uncertain: false,
            package_error: None,
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
        consumer_id: &'a str,
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
        _consumer_id: &'a str,
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
        terminal_bytes: Arc::new(Semaphore::new(TERMINAL_RESERVE_BYTES)),
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

struct PendingStreamOpen {
    task: tokio::task::JoinHandle<()>,
    closed: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Default)]
struct PendingStreamOpens(BTreeMap<String, PendingStreamOpen>);

impl Drop for PendingStreamOpens {
    fn drop(&mut self) {
        // Provider activation is not cancellation-safe: an open already in
        // lifecycle startup must settle and reap/register its child. Its task
        // owns a permit and closes any route it creates after this reader ends.
        for open in self.0.values() {
            open.closed
                .store(true, std::sync::atomic::Ordering::Release);
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
    let mut opening_streams = PendingStreamOpens::default();
    let pending_open_slots = Arc::new(Semaphore::new(MAX_IN_FLIGHT_EACH_DIRECTION));
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
        // Completed opens no longer need an abort handle. Live pending opens are
        // bounded by the process stream classifier; a close aborts its own task.
        opening_streams.0.retain(|_, open| !open.task.is_finished());
        // One classifier, before every branch. There is no side of `observe` on
        // which an adapter can hold its own idea of which races are tolerable,
        // and no frame kind that reaches its adapter without being classified.
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
                        capabilities.call(grant.as_deref(), &payload, started.clone(), &id),
                    )
                    .await
                    .unwrap_or_else(|_| {
                        Err(CapabilityFailure {
                            code: PackageErrorCode::DeadlineExceeded,
                            // A state-changing call whose Core work had already
                            // begun may still commit. `append` is content
                            // idempotent, so the Package can retry it.
                            outcome_uncertain: (method == "invoke"
                                && payload.get("kind").and_then(Value::as_str)
                                    == Some("command"))
                                // Port work may own a detached task that can dispatch
                                // *after* this outer timeout. Without a fence, a
                                // certain timeout would invite a duplicate write.
                                || (method == "append"
                                    && started.load(std::sync::atomic::Ordering::Acquire)),
                            package_error: None,
                        })
                    });
                    let response = match result {
                        Ok(payload) => json!({"kind":"response","id":id,"payload":payload}),
                        Err(failure) => {
                            let mut details = json!({
                                "kind":"error", "id":id,
                                "code":if failure.outcome_uncertain && method == "invoke"
                                    && failure.code == PackageErrorCode::DeadlineExceeded {
                                    "outcome_unknown"
                                } else { capability_code(failure.code) },
                                "retryable":matches!(
                                    failure.code,
                                    PackageErrorCode::Overloaded
                                        | PackageErrorCode::DeadlineExceeded
                                ),
                                "details":{}
                            });
                            if let Some(provider) = failure.package_error
                                && let Some(object) = details.as_object_mut()
                            {
                                object.insert("details".to_owned(), json!({"provider":provider}));
                            }
                            if failure.outcome_uncertain
                                && let Some(object) = details.as_object_mut()
                            {
                                object.insert("outcomeUncertain".to_owned(), Value::Bool(true));
                                if method == "invoke" {
                                    object.insert("retryable".to_owned(), Value::Bool(false));
                                }
                            }
                            details
                        }
                    };
                    // The answer waits for control-lane capacity: a congested
                    // Lane congestion may replace the computed answer. Preserve
                    // uncertainty if the Provider already accepted a command.
                    let accepted_command = method == "invoke"
                        && payload.get("kind").and_then(Value::as_str) == Some("command")
                        && (started.load(std::sync::atomic::Ordering::Acquire)
                            || response
                                .get("outcomeUncertain")
                                .and_then(Value::as_bool)
                                .unwrap_or(false));
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
                            let overloaded = if accepted_command {
                                json!({
                                    "kind":"error", "id":answered, "code":"outcome_unknown",
                                    "retryable":false, "outcomeUncertain":true, "details":{}
                                })
                            } else {
                                json!({
                                    "kind":"error", "id":answered, "code":"overloaded",
                                    "retryable":true, "details":{}
                                })
                            };
                            // The typed error is still this request's one
                            // terminal frame, so it is owed. It goes on the
                            // reserve rather than on the budget the answer it
                            // replaces could not get: a refusing send, or a
                            // second wait for the same exhausted budget, drops
                            // the answer under exactly the saturation that
                            // produced it, which is the outcome this whole
                            // path exists to replace.
                            if writer.send_terminal_error(overloaded).await.is_err() {
                                let _ = failure.send(Some(PackageErrorCode::ProcessExited));
                            }
                        } else {
                            let _ = failure.send(Some(PackageErrorCode::ProcessExited));
                        }
                    }
                });
            }
            FrameKind::Cancel => {
                // The classifier retired the caller's wait. Port dispatch has its own task and
                // keeps the request/byte permits until Provider work ends; no command is replayed.
                // A late answer is ignored by the classifier on both peers.
            }
            FrameKind::StreamOpen => {
                let Some(id) = frame.field("id").and_then(Value::as_str).map(str::to_owned) else {
                    let _ = failure.send(Some(PackageErrorCode::MalformedFrame));
                    return;
                };
                let topic = frame
                    .field("topic")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let initial_credit = frame
                    .field("initialCredit")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                let payload = frame.field("payload").cloned().unwrap_or_else(empty_object);
                let Ok(slot) = pending_open_slots.clone().try_acquire_owned() else {
                    // The classifier accepts distinct stream IDs without a
                    // per-process count ceiling. Refuse excess adapter work
                    // without waiting on a slow Provider from the reader.
                    if writer
                        .control(json!({
                            "kind":"stream_close", "streamId":id, "code":"overloaded"
                        }))
                        .await
                        .is_err()
                    {
                        let _ = failure.send(Some(PackageErrorCode::ProcessExited));
                        return;
                    }
                    continue;
                };
                let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let task_closed = closed.clone();
                let handler = capabilities.clone();
                let task_writer = writer.clone();
                let task_state = state.clone();
                let task_failure = failure.clone();
                let stream_id = id.clone();
                // Provider activation and stream setup can wait much longer than
                // this Consumer's health probe. The classified open owns one
                // bounded stream task, never the process reader.
                let task = tokio::spawn(async move {
                    let _slot = slot;
                    let result = handler
                        .open_stream(
                            stream_id.clone(),
                            &topic,
                            initial_credit,
                            &payload,
                            HostStreamSender {
                                writer: task_writer.clone(),
                                state: task_state,
                            },
                        )
                        .await;
                    if task_closed.load(std::sync::atomic::Ordering::Acquire) {
                        handler.close_stream(&stream_id).await;
                    } else if let Err(code) = result {
                        let response = json!({
                            "kind":"stream_close", "streamId":stream_id,
                            "code":capability_code(code)
                        });
                        if task_writer.send_waiting(response, true).await.is_err() {
                            let _ = task_failure.send(Some(PackageErrorCode::ProcessExited));
                        }
                    }
                });
                opening_streams
                    .0
                    .insert(id, PendingStreamOpen { task, closed });
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
                    if let Some(open) = opening_streams.0.remove(stream_id) {
                        // A close does not cancel Provider activation. If the
                        // open registers a route later, its owned task closes
                        // it after startup settles. Its permit remains held.
                        open.closed
                            .store(true, std::sync::atomic::Ordering::Release);
                    }
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
            FrameKind::Event => {
                let Some(stream_id) = frame.field("streamId").and_then(Value::as_str) else {
                    let _ = failure.send(Some(PackageErrorCode::MalformedFrame));
                    return;
                };
                let route = {
                    let routes = ui_event_routes.lock().await;
                    routes.get(stream_id).cloned()
                };
                // A live stream whose route has gone is the subscriber having
                // left; the stream ends on its own path and the classifier
                // ignores what follows the close.
                let Some(route) = route else { continue };
                let Some(sequence) = frame
                    .field("sequence")
                    .and_then(Value::as_str)
                    .and_then(|value| value.parse::<u64>().ok())
                else {
                    // Unreachable: the classifier read this field as a decimal.
                    let _ = failure.send(Some(PackageErrorCode::ProtocolViolation));
                    return;
                };
                let payload = frame.field("payload").cloned().unwrap_or_else(empty_object);
                if let Err(code) = route.event(sequence, payload) {
                    // The UI contract and the route's own bounds end this one
                    // stream. The frame was in contract, so the generation
                    // lives and the Package is told the stream is over.
                    // Retire in the classifier before the adapter forgets the
                    // route, so the two can never disagree about whether this
                    // stream is still served.
                    state.lock().await.retire_stream(stream_id);
                    ui_event_routes.lock().await.remove(stream_id);
                    route.terminate(StreamEnd::Host(code));
                    let _ = writer
                        .control(json!({
                            "kind":"stream_close", "streamId":stream_id,
                            "code":ui_stream_code(code)
                        }))
                        .await;
                }
            }
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
        PackageErrorCode::PortEventInvalid => "port_event_invalid",
        PackageErrorCode::PortNotEnabled => "port_not_enabled",
        PackageErrorCode::AgentNotBound => "agent_not_bound",
        PackageErrorCode::AgentBindingChanged => "agent_binding_changed",
        PackageErrorCode::ProcessExited => "provider_exited",
        PackageErrorCode::Cancelled => "cancelled",
        _ => "protocol_violation",
    }
}

// Process 1.0 requires object payloads. Escape the reserved envelope shape
// itself as well as non-object Port values, so decoding changes no valid value.
pub(crate) fn encode_port_value(value: Value) -> Value {
    if value.is_object() && port_value_content(&value).is_none() {
        value
    } else {
        json!({"$teamarnessPortEnvelope":1,"$teamarnessPortValue":value})
    }
}

fn port_value_content(value: &Value) -> Option<&Value> {
    let fields = value.as_object()?;
    (fields.len() == 2 && fields.get("$teamarnessPortEnvelope") == Some(&Value::from(1)))
        .then(|| fields.get("$teamarnessPortValue"))
        .flatten()
}

pub(crate) fn decode_port_value(value: &Value) -> &Value {
    port_value_content(value).unwrap_or(value)
}

pub(crate) fn capability_code(code: PackageErrorCode) -> &'static str {
    match code {
        PackageErrorCode::CapabilityUnavailable => "capability_unavailable",
        PackageErrorCode::Cancelled => "cancelled",
        PackageErrorCode::CapabilityVersionUnsupported => "capability_version_unsupported",
        PackageErrorCode::DeadlineExceeded => "deadline_exceeded",
        PackageErrorCode::Overloaded => "overloaded",
        PackageErrorCode::PortUnavailable => "port_unavailable",
        PackageErrorCode::PortCatalogChanged => "port_catalog_changed",
        PackageErrorCode::PortNotEnabled => "port_not_enabled",
        PackageErrorCode::AgentNotBound => "agent_not_bound",
        PackageErrorCode::AgentBindingChanged => "agent_binding_changed",
        PackageErrorCode::PortVersionMismatch => "port_version_mismatch",
        PackageErrorCode::PortUngranted => "ungranted_port",
        PackageErrorCode::PortRequestInvalid => "port_request_invalid",
        PackageErrorCode::PortResponseInvalid => "port_response_invalid",
        PackageErrorCode::PortEventInvalid => "port_event_invalid",
        PackageErrorCode::OutcomeUncertain => "outcome_unknown",
        PackageErrorCode::ProcessExited => "provider_exited",
        PackageErrorCode::BackendError => "provider_error",
        PackageErrorCode::PrincipalNotBound => "principal_not_bound",
        PackageErrorCode::AgentNotStewarded => "agent_not_stewarded",
        PackageErrorCode::AgentNotFound => "agent_not_found",
        PackageErrorCode::UnsupportedMediaType => "unsupported_media_type",
        PackageErrorCode::ContentTooLarge => "content_too_large",
        PackageErrorCode::InvalidContentPath => "invalid_content_path",
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
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn open_port_stream(
        &self,
        id: String,
        topic: &str,
        payload: PackageValue,
        schemas: Arc<teamarness_package_contract::port_contract::CompiledPortContracts>,
        contract: String,
        port: String,
        lease: Option<Arc<PortStreamLease>>,
    ) -> Result<UiEventReceiver, WireFailure> {
        let (route, receiver) = UiEventRoute::new_port(schemas, contract, port, lease);
        self.ui_event_routes
            .lock()
            .await
            .insert(id.clone(), route.clone());
        if let Err(error) = self
            .writer
            .control(json!({
                "kind":"stream_open", "id":id, "topic":topic,
                "initialCredit":INITIAL_STREAM_CREDIT, "payload":package_value_to_json(payload)
            }))
            .await
        {
            self.ui_event_routes.lock().await.remove(&id);
            route.terminate(StreamEnd::Host(error.code));
            return Err(error);
        }
        Ok(receiver)
    }

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
        self.request_inner(
            id,
            operation,
            mount,
            payload,
            deadline,
            state_changing,
            None,
            None,
        )
        .await
    }

    pub(crate) async fn request_observed(
        &self,
        id: String,
        operation: &str,
        payload: PackageValue,
        on_dispatch: Box<dyn FnOnce() + Send>,
    ) -> Result<Value, WireFailure> {
        self.request_inner(
            id,
            operation,
            None,
            payload,
            Duration::from_secs(8),
            false,
            None,
            Some(on_dispatch),
        )
        .await
    }

    pub(crate) async fn request_port(
        &self,
        id: String,
        operation: &str,
        payload: PackageValue,
        dispatched: Arc<std::sync::atomic::AtomicBool>,
        on_dispatch: Box<dyn FnOnce() + Send>,
    ) -> Result<Value, WireFailure> {
        self.request_inner(
            id,
            operation,
            None,
            payload,
            Duration::from_secs(8),
            true,
            Some(dispatched),
            Some(on_dispatch),
        )
        .await
    }

    // The legacy mount request and a port call share one writer-acceptance boundary.
    #[allow(clippy::too_many_arguments)]
    async fn request_inner(
        &self,
        id: String,
        operation: &str,
        mount: Option<&str>,
        payload: PackageValue,
        deadline: Duration,
        state_changing: bool,
        dispatched: Option<Arc<std::sync::atomic::AtomicBool>>,
        on_dispatch: Option<Box<dyn FnOnce() + Send>>,
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
        if let Err(error) = self.writer.data(value).await {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        if let Some(dispatched) = dispatched {
            dispatched.store(true, std::sync::atomic::Ordering::Release);
        }
        if let Some(on_dispatch) = on_dispatch {
            on_dispatch();
        }
        match tokio::time::timeout(deadline, &mut receiver).await {
            Ok(Ok(result)) => result.map_err(|mut error| {
                if state_changing
                    && !matches!(
                        error.code,
                        PackageErrorCode::BackendError | PackageErrorCode::Overloaded
                    )
                {
                    error.outcome_uncertain = true;
                }
                error
            }),
            Ok(Err(_)) => {
                let mut error = WireFailure::new(PackageErrorCode::ProcessExited);
                error.outcome_uncertain = state_changing;
                Err(error)
            }
            Err(_) => {
                if let Err(mut error) = self.send_cancel(&id).await {
                    // The command crossed the writer before cancellation failed. Retire
                    // its id and do not claim that the failed cancel undid its effect.
                    self.state.lock().await.retire_request(&id);
                    self.pending.lock().await.remove(&id);
                    error.outcome_uncertain |= state_changing;
                    return Err(error);
                }
                match tokio::time::timeout(Duration::from_millis(500), &mut receiver).await {
                    Ok(Ok(result)) => result.map_err(|mut error| {
                        // Even a Provider-owned or overloaded error arriving after the
                        // deadline cannot prove that a dispatched command did not commit.
                        error.outcome_uncertain |= state_changing;
                        error
                    }),
                    _ => {
                        // Retire before the adapter entry goes, in that order
                        // and for the same reason as `cancel`: between the two
                        // the classifier and the adapter must never disagree
                        // about whether this request is still live.
                        self.state.lock().await.retire_request(&id);
                        self.pending.lock().await.remove(&id);
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
        // Retire in the classifier before the adapter forgets the request. The
        // `cancel` frame is only queued here, and the writer may be blocked on
        // a slow pipe for as long as it takes to drain one; until it observes
        // the frame the classifier still calls the request live, so an answer
        // arriving in that window would be delivered to an adapter that no
        // longer holds it and read as a violation.
        self.state.lock().await.retire_request(target);
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
    /// The amount is the acknowledged delta and nothing else. Clamping it here
    /// would put a frame on the wire that says something the Host did not
    /// account for; the classifier reads the frame the adapter asked for and
    /// fails the generation if the Host ever asks for one out of contract.
    ///
    /// A stream that ended while the acknowledgement was in flight makes this
    /// frame late, not wrong: the writer's classifier ignores it.
    ///
    /// This send waits for a control slot rather than refusing one
    /// (`CREDIT_WAITS_FOR_A_SLOT`). A subscription at a full window owes a
    /// whole window of `credit` frames, which is more than the writer's whole
    /// published frame budget, so a refusing send here destroys credit on
    /// ordinary legal traffic. The wait is bounded: the caller is the UI
    /// command's own task, never the reader or the writer loop, and a Package
    /// that has stopped reading is failed by the five-second health probe,
    /// which closes these channels.
    pub(crate) async fn credit(&self, stream_id: &str, amount: u64) -> Result<(), WireFailure> {
        self.writer
            .send_waiting(
                json!({"kind":"credit", "streamId":stream_id, "amount":amount}),
                true,
            )
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
        Duration, MAX_ID_BYTES, MAX_UI_PAYLOAD_BYTES, StreamEnd, TERMINAL_ERROR_BYTES, WireFailure,
        package_owned_error, start, stream_end,
    };
    use crate::{MAX_UI_SUBSCRIPTIONS_PER_MOUNT, PackageErrorCode};
    use serde_json::{Value, json};
    use teamarness_package_protocol::{Direction, Frame, INITIAL_STREAM_CREDIT};
    use tokio::io::AsyncReadExt;

    /// One route admits every event the credit window grants, at the largest
    /// payload the UI contract allows, without refusing one.
    ///
    /// This replaces `UI_EVENT_CAPACITY == INITIAL_STREAM_CREDIT` and
    /// `UI_EVENT_BYTES >= INITIAL_STREAM_CREDIT * MAX_UI_PAYLOAD_BYTES`, which
    /// restated the definitions two lines above them. The route is what the
    /// classifier hands events to, so a capacity or a byte budget that stopped
    /// holding the window fails here — with `EventLagged`, the code a
    /// subscriber would actually see — instead of agreeing with itself.
    #[tokio::test]
    async fn a_route_admits_a_full_window_of_largest_events() {
        let (route, _receiver) = super::UiEventRoute::new(ui_contract(), "fixture.events".into());
        // `{"text":"` and `"}` are the eleven bytes around the filler.
        let filler = "a".repeat(MAX_UI_PAYLOAD_BYTES - 11);
        for sequence in 1..=INITIAL_STREAM_CREDIT as u64 {
            let admitted = route.event(sequence, json!({ "text": filler }));
            assert!(
                admitted.is_ok(),
                "the route refused event {sequence} of the {INITIAL_STREAM_CREDIT} the window grants: {admitted:?}"
            );
        }
    }

    /// A `credit` frame that never reached the writer leaves the
    /// acknowledgement unrecorded, so the next one covers the same events.
    ///
    /// The route advanced first and the Host had no way back: `acknowledge`'s
    /// own first guard refuses a repeat, and this path is the only producer of
    /// `credit` there is. `commit` takes the send rather than handing out the
    /// amount, so a call site cannot put the record first.
    #[tokio::test]
    async fn an_uncommitted_acknowledgement_leaves_its_credit_to_claim_again() {
        let (route, _receiver) = super::UiEventRoute::new(ui_contract(), "fixture.events".into());
        route.mark_delivered(2).await;
        let refused = route
            .acknowledge(2)
            .await
            .expect("acknowledge to be servable")
            .expect("two events to credit")
            .commit(|amount| {
                assert_eq!(amount, 2);
                std::future::ready(Err(WireFailure::new(PackageErrorCode::ProcessExited)))
            })
            .await;
        assert!(refused.is_err(), "the frame never reached the writer");

        let mut credited = None;
        route
            .acknowledge(2)
            .await
            .expect("acknowledge to be servable")
            .expect("the same two events are still owed credit")
            .commit(|amount| {
                credited = Some(amount);
                std::future::ready(Ok(()))
            })
            .await
            .expect("the second attempt is queued");
        assert_eq!(credited, Some(2));
        assert!(
            matches!(route.acknowledge(2).await, Ok(None)),
            "a committed acknowledgement is not owed twice: the repeat takes \
             the ignore verdict (CF-002), releasing no credit"
        );
    }

    /// A late cancellation acknowledgement does not undo an accepted command.
    #[tokio::test]
    async fn timed_out_command_with_grace_error_is_uncertain() {
        let (input, _peer) = tokio::io::duplex(64);
        let client = start(input, tokio::io::sink());
        let request = tokio::spawn({
            let client = client.clone();
            async move {
                client
                    .request_inner(
                        "h:late-command".into(),
                        "teamarness.port.command.v1",
                        None,
                        crate::PackageValue::Object(Default::default()),
                        Duration::from_millis(20),
                        true,
                        None,
                        None,
                    )
                    .await
            }
        });
        // The injected terminal arrives after the request deadline, inside the
        // explicit 500 ms cancellation grace. This delay tests that timing edge.
        tokio::time::sleep(Duration::from_millis(80)).await;
        let sender = client
            .pending
            .lock()
            .await
            .remove("h:late-command")
            .expect("the dispatched request is still awaiting grace");
        sender
            .send(Err(WireFailure::new(PackageErrorCode::Cancelled)))
            .expect("the request is still waiting");
        let failure = request.await.unwrap().unwrap_err();
        assert_eq!(failure.code, PackageErrorCode::Cancelled);
        assert!(failure.outcome_uncertain);
    }

    #[tokio::test]
    async fn refused_cancel_after_command_dispatch_is_uncertain_and_retires_id() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let (input, _peer) = tokio::io::duplex(64);
        let client = start(input, tokio::io::sink());
        let dispatched = Arc::new(AtomicBool::new(false));
        let request = tokio::spawn({
            let client = client.clone();
            let dispatched = dispatched.clone();
            async move {
                client
                    .request_inner(
                        "h:cancel-refused".into(),
                        "teamarness.port.command.v1",
                        None,
                        crate::PackageValue::Object(Default::default()),
                        Duration::from_millis(100),
                        true,
                        Some(dispatched),
                        None,
                    )
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !dispatched.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("writer accepted the command");
        let _block = tokio::time::timeout(
            Duration::from_secs(2),
            client
                .writer
                .data_bytes
                .clone()
                .acquire_many_owned(super::DATA_BYTES as u32),
        )
        .await
        .expect("writer releases accepted request bytes")
        .unwrap();
        let failure = request.await.unwrap().unwrap_err();
        assert_eq!(failure.code, PackageErrorCode::Overloaded);
        assert!(failure.outcome_uncertain);
        assert!(!client.pending.lock().await.contains_key("h:cancel-refused"));
    }

    /// A refused writer enqueue is certain: it must not publish a Provider join.
    #[tokio::test]
    async fn refused_port_writer_does_not_publish_dispatch_for_command_or_query() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let (input, _peer) = tokio::io::duplex(64);
        let client = start(input, tokio::io::sink());
        let _block = client
            .writer
            .data_bytes
            .clone()
            .try_acquire_many_owned(super::DATA_BYTES as u32)
            .unwrap();
        for kind in ["command", "query"] {
            let observed = Arc::new(AtomicBool::new(false));
            let flag = observed.clone();
            let on_dispatch: Box<dyn FnOnce() + Send> = Box::new(move || {
                flag.store(true, Ordering::Release);
            });
            let result = if kind == "command" {
                client
                    .request_port(
                        format!("h:refused-{kind}"),
                        "teamarness.port.command.v1",
                        crate::PackageValue::Object(Default::default()),
                        Arc::new(AtomicBool::new(false)),
                        on_dispatch,
                    )
                    .await
            } else {
                client
                    .request_observed(
                        format!("h:refused-{kind}"),
                        "teamarness.port.query.v1",
                        crate::PackageValue::Object(Default::default()),
                        on_dispatch,
                    )
                    .await
            };
            assert_eq!(result.unwrap_err().code, PackageErrorCode::Overloaded);
            assert!(
                !observed.load(Ordering::Acquire),
                "{kind} published a refused route"
            );
        }
    }

    fn ui_contract() -> std::sync::Arc<crate::ui::AdmittedUi> {
        std::sync::Arc::new(crate::ui::AdmittedUi {
            cache: crate::ui::UiCacheRecord {
                admitted_root: std::path::PathBuf::new(),
                entrypoint: String::new(),
                resources: std::collections::BTreeMap::new(),
                contract_digest: String::new(),
            },
            contract: crate::ui::compile_contract(
                br#"{"schemaVersion":1,"$defs":{},"requests":{},"subscriptions":{"fixture.events":{"request":{"type":"object","additionalProperties":false},"event":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false}}}}"#,
            )
            .expect("a compiled UI contract"),
        })
    }

    /// A mount's legal acknowledgement traffic never loses a `credit` frame,
    /// however full the control lane is.
    ///
    /// The Shell acknowledges one event at a time, so the eight subscriptions
    /// one mount may hold, each at a full window, owe
    /// `MAX_UI_SUBSCRIPTIONS_PER_MOUNT * INITIAL_STREAM_CREDIT` = 256 `credit`
    /// frames at once — more than any split of the 64 frames the contract
    /// publishes for the whole writer can hold. A refusing send destroys the
    /// ones that do not fit, and the Host has no second producer of `credit`:
    /// the window stays shut, a conforming Package stops sending, and the
    /// instance is still reported `Active`.
    #[tokio::test]
    async fn a_full_control_lane_never_destroys_acknowledged_credit() {
        // A child that has taken the pipe and stopped reading, which is the
        // congestion the lane exists to absorb.
        let (child, mut pipe) = tokio::io::duplex(1);
        let client = start(tokio::io::empty(), child);
        let streams = (1..=MAX_UI_SUBSCRIPTIONS_PER_MOUNT)
            .map(|index| format!("h:ui-{index}"))
            .collect::<Vec<_>>();
        {
            let mut state = client.state.lock().await;
            for id in &streams {
                state
                    .observe(
                        Direction::HostToPackage,
                        &frame(json!({
                            "kind":"stream_open", "id":id, "topic":"t", "mountId":"bW91bnQ",
                            "initialCredit":INITIAL_STREAM_CREDIT, "payload":{}
                        })),
                    )
                    .expect("the UI stream opens");
                for sequence in 1..=INITIAL_STREAM_CREDIT {
                    state
                        .observe(
                            Direction::PackageToHost,
                            &frame(json!({
                                "kind":"event", "streamId":id,
                                "sequence":sequence.to_string(), "payload":{}
                            })),
                        )
                        .expect("the window admits the event");
                }
            }
        }

        let acknowledgements = tokio::spawn({
            let client = client.clone();
            async move {
                let mut outcomes = Vec::new();
                for id in &streams {
                    for _ in 0..INITIAL_STREAM_CREDIT {
                        outcomes.push(client.credit(id, 1).await);
                    }
                }
                outcomes
            }
        });
        // Long enough for the lane to fill behind the stalled child.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let drain = tokio::spawn(async move {
            let mut buffer = [0_u8; 4096];
            while matches!(pipe.read(&mut buffer).await, Ok(count) if count > 0) {}
        });

        let outcomes = tokio::time::timeout(Duration::from_secs(20), acknowledgements)
            .await
            .expect("every acknowledgement settles")
            .expect("the acknowledging task");
        drain.abort();
        let refused = outcomes
            .iter()
            .filter_map(|outcome| outcome.as_ref().err())
            .map(|failure| failure.code)
            .collect::<Vec<_>>();
        assert!(
            refused.is_empty(),
            "{} of {} acknowledgements were destroyed by the control lane, first {:?}",
            refused.len(),
            outcomes.len(),
            refused.first()
        );
    }

    /// The reserve holds the frame it exists for: a typed terminal `error` for
    /// an id of the contract's maximum length.
    #[test]
    fn terminal_answers_fit_their_reserve() {
        let longest = format!("p:{}", "a".repeat(MAX_ID_BYTES - 2));
        assert_eq!(longest.len(), MAX_ID_BYTES);
        for value in [
            json!({"kind":"error", "id":longest, "code":"overloaded", "retryable":true, "details":{}}),
            json!({"kind":"error", "id":longest, "code":"outcome_unknown", "retryable":false,
                "outcomeUncertain":true, "details":{}}),
        ] {
            let encoded = teamarness_package_protocol::encode_frame(&frame(value))
                .expect("encoded frame")
                .len();
            assert!(
                encoded <= TERMINAL_ERROR_BYTES,
                "the reserve holds {TERMINAL_ERROR_BYTES} bytes per answer, the frame needs {encoded}"
            );
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
