use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use teamarness_package_protocol::{
    ContractState, Direction, Frame, FrameKind, MAX_FRAME_BYTES, MAX_IN_FLIGHT_EACH_DIRECTION,
    MAX_WRITER_BYTES, decode_frame, encode_frame,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch};

use crate::{PackageErrorCode, PackageValue};

type PendingRequests = Arc<Mutex<BTreeMap<String, oneshot::Sender<Result<Value, WireFailure>>>>>;

#[derive(Debug, Clone)]
pub(crate) struct WireFailure {
    pub(crate) code: PackageErrorCode,
    pub(crate) outcome_uncertain: bool,
}

impl WireFailure {
    pub(crate) const fn new(code: PackageErrorCode) -> Self {
        Self {
            code,
            outcome_uncertain: false,
        }
    }
}

#[derive(Clone)]
pub(crate) struct ProcessClient {
    writer: Writer,
    pending: PendingRequests,
    tombstones: Arc<Mutex<BTreeSet<String>>>,
    closed_streams: Arc<Mutex<BTreeSet<String>>>,
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
        sender
            .send(queued)
            .await
            .map_err(|_| WireFailure::new(PackageErrorCode::ProcessExited))
    }
}

pub(crate) fn start<R, W>(reader: R, output: W) -> ProcessClient
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (control_tx, control_rx) = mpsc::channel(16);
    let (data_tx, data_rx) = mpsc::channel(48);
    let writer = Writer {
        control: control_tx,
        data: data_tx,
        control_bytes: Arc::new(Semaphore::new(MAX_WRITER_BYTES / 8)),
        data_bytes: Arc::new(Semaphore::new(MAX_WRITER_BYTES * 7 / 8)),
    };
    let pending = Arc::new(Mutex::new(BTreeMap::new()));
    let tombstones = Arc::new(Mutex::new(BTreeSet::new()));
    let closed_streams = Arc::new(Mutex::new(BTreeSet::new()));
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
        tombstones.clone(),
        closed_streams.clone(),
        state,
        initialized_tx,
        shutdown_tx,
        pong_tx,
        failure_tx.clone(),
    ));
    ProcessClient {
        writer,
        pending,
        tombstones,
        closed_streams,
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
    tombstones: Arc<Mutex<BTreeSet<String>>>,
    closed_streams: Arc<Mutex<BTreeSet<String>>>,
    state: Arc<Mutex<ContractState>>,
    initialized: oneshot::Sender<Frame>,
    shutdown: watch::Sender<bool>,
    pongs: watch::Sender<Option<String>>,
    failure: watch::Sender<Option<PackageErrorCode>>,
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
                return;
            }
        };
        let is_late_stream_frame =
            if matches!(frame.kind(), FrameKind::Event | FrameKind::StreamClose) {
                match frame.field("streamId").and_then(Value::as_str) {
                    Some(id) => closed_streams.lock().await.contains(id),
                    None => false,
                }
            } else {
                false
            };
        if is_late_stream_frame {
            continue;
        }
        if let Err(error) = state.lock().await.observe(Direction::PackageToHost, &frame) {
            let code = protocol_code(error);
            let _ = failure.send(Some(code));
            fail_pending(&pending, code).await;
            return;
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
                if tombstones.lock().await.remove(&id) {
                    continue;
                }
                let Some(sender) = pending.lock().await.remove(&id) else {
                    let _ = failure.send(Some(PackageErrorCode::ProtocolViolation));
                    return;
                };
                let result = if frame.kind() == FrameKind::Response {
                    Ok(frame.field("payload").cloned().unwrap_or_else(empty_object))
                } else {
                    let code = match frame.field("code").and_then(Value::as_str) {
                        Some("cancelled") => PackageErrorCode::Cancelled,
                        Some("overloaded") => PackageErrorCode::Overloaded,
                        _ => PackageErrorCode::BackendError,
                    };
                    let mut failure = WireFailure::new(code);
                    failure.outcome_uncertain = frame
                        .field("outcomeUncertain")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    Err(failure)
                };
                let _ = sender.send(result);
            }
            FrameKind::Request => {
                let Some(id) = frame.field("id").and_then(Value::as_str) else {
                    let _ = failure.send(Some(PackageErrorCode::MalformedFrame));
                    return;
                };
                let response = json!({"kind":"response","id":id,"payload":{"ok":true}});
                if writer.control(response).await.is_err() {
                    let _ = failure.send(Some(PackageErrorCode::ProcessExited));
                    return;
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
            FrameKind::Event | FrameKind::StreamClose => {}
            _ => {
                let _ = failure.send(Some(PackageErrorCode::ProtocolViolation));
                return;
            }
        }
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
                        self.tombstones.lock().await.insert(id);
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

    pub(crate) async fn cancel(&self, target: &str) -> Result<(), WireFailure> {
        let removed = self.pending.lock().await.remove(target);
        if let Some(sender) = removed {
            let _ = sender.send(Err(WireFailure::new(PackageErrorCode::Cancelled)));
        } else if self.tombstones.lock().await.contains(target) {
            return Ok(());
        }
        self.tombstones.lock().await.insert(target.to_owned());
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

    pub(crate) async fn stream_open(
        &self,
        id: String,
        topic: &str,
        mount: &str,
        payload: PackageValue,
    ) -> Result<(), WireFailure> {
        self.closed_streams.lock().await.remove(&id);
        self.writer
            .control(json!({
                "kind":"stream_open", "id":id, "topic":topic, "mountId":mount,
                "initialCredit":32, "payload":package_value_to_json(payload)
            }))
            .await
    }

    pub(crate) async fn credit(&self, stream_id: &str, amount: u64) -> Result<(), WireFailure> {
        self.writer
            .control(json!({"kind":"credit","streamId":stream_id,"amount":amount.min(32)}))
            .await
    }

    pub(crate) async fn stream_close(&self, stream_id: &str) -> Result<(), WireFailure> {
        let mut closed = self.closed_streams.lock().await;
        closed.insert(stream_id.to_owned());
        while closed.len() > 64 {
            closed.pop_first();
        }
        drop(closed);
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
