use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use crate::synchronization::parking::ParkReason;
use crate::synchronization::peer_sessions::{self, PeerSessions};
use crate::synchronization::{SyncServices, scope_digest};
use crate::{PublicIdentity, WorkspaceId};
use futures_util::{Sink, Stream, StreamExt};
use p2panda_core::Operation;
use p2panda_sync::protocols::{LogSyncEvent, LogSyncMessage};
use serde::{Deserialize, Serialize};

use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use tokio::time::timeout;

pub(crate) const MAX_SYNC_OPERATIONS: usize = 4_096;
pub(crate) const MAX_SYNC_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const SYNC_SESSION_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const LOG_SYNC_ALPN: &[u8] = b"teamarness/package-sync/2";

/// The first frame a responder reads, before any authorization, is a `Hello`.
/// Nothing larger is read from an unauthorized peer.
const MAX_HELLO_BYTES: usize = 1024;
/// A version-2 protection control is exactly 354 bytes; the frame cap leaves
/// room without admitting an unbounded handshake.
const MAX_PROTECTION_CONTROL_BYTES: usize = 4096;
const MAX_PACKAGE_IDENTITY: usize = 256;
const APPLICATION_ACK_REQUEST: &[u8] = b"teamarness/log-sync/applied/1";
const APPLICATION_ACK: &[u8] = b"teamarness/log-sync/applied/1/ok";

/// QUIC close codes for `teamarness/package-sync/2` (design §5.10). A `Refused`
/// frame reason is a different value space and is never used as a close code.
const CLOSE_UNAVAILABLE: u8 = 1;
const CLOSE_INVALID: u8 = 4;
const CLOSE_NOT_AUTHORIZED: u8 = 5;
const CLOSE_KEY_MISMATCH: u8 = 6;

type ReplicationOperation = ([u8; 32], Operation<()>);

/// Why a responder, or the local gate, refuses a package synchronization
/// session. These are frame reasons, not QUIC close codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    NotAuthorized = 1,
    Unavailable = 2,
    KeyMismatch = 3,
}

impl Refusal {
    const fn reason(self) -> u8 {
        self as u8
    }

    const fn from_reason(reason: u8) -> Option<Self> {
        match reason {
            1 => Some(Self::NotAuthorized),
            2 => Some(Self::Unavailable),
            3 => Some(Self::KeyMismatch),
            _ => None,
        }
    }

    /// The design's close-code table (§5.10), read back. The `Refused` frame
    /// reason is a different value space and never doubles as a close code.
    const fn from_close_code(code: u64) -> Option<Self> {
        match code {
            5 => Some(Self::NotAuthorized),
            1 => Some(Self::Unavailable),
            6 => Some(Self::KeyMismatch),
            _ => None,
        }
    }
}

/// Why one received operation was refused outright.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RejectReason {
    Invalid,
    KeyMismatch,
    Conflict,
    WrongScope,
}

/// What happened to one operation received in a session.
///
/// Only [`FatalSyncError`] ends a session: a rejected or unparkable operation
/// skips its author for the rest of this session and is redelivered later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OperationOutcome {
    Applied,
    Duplicate,
    Parked(ParkReason),
    ParkRefused,
    Rejected(RejectReason),
}

/// The only two conditions that end a session instead of skipping an author.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FatalSyncError {
    PeerRevoked,
    Storage,
}

/// What the membership gate grants one authorized session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SessionGrant {
    pub(crate) membership_version: [u8; 32],
    pub(crate) content_key_id: Option<[u8; 32]>,
}

/// A content-key grant together with the Workspace creator's signed origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProtectionBundle {
    pub(crate) origin: Vec<u8>,
    pub(crate) grant: Vec<u8>,
}

/// How one package synchronization session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LogSessionError {
    Refused(Refusal),
    Cancelled,
    Transport(String),
    Protocol(String),
}

impl std::fmt::Display for LogSessionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(refusal) => write!(formatter, "package sync refused: {refusal:?}"),
            Self::Cancelled => formatter.write_str("package sync session cancelled"),
            Self::Transport(error) => write!(formatter, "package sync transport: {error}"),
            Self::Protocol(error) => write!(formatter, "package sync protocol: {error}"),
        }
    }
}

/// The Core seam every package synchronization session speaks to. The session
/// itself never names runtime state, storage, or membership types.
pub(crate) trait SyncReceiver: Send + Sync {
    /// Decides whether this peer may synchronize this Package scope at all.
    ///
    /// Both the local Installation and the remote peer must be current Members
    /// of the named Workspace. Nothing is loaded or advertised before this.
    fn authorize_session(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
    ) -> Result<SessionGrant, Refusal>;

    fn protection_bundle_for(
        &self,
        workspace: WorkspaceId,
        package: &str,
        recipient: PublicIdentity,
    ) -> Result<Option<ProtectionBundle>, ()>;

    fn import_protection_bundle(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
        bundle: ProtectionBundle,
    ) -> Result<usize, ()>;

    /// Records that the peer's membership view differs from the local one, so
    /// the worker can run a membership session with it.
    fn note_membership_mismatch(&self, _workspace: WorkspaceId, _peer: PublicIdentity) {}

    fn authors(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
    ) -> Result<Vec<PublicIdentity>, ()>;

    fn operations(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
    ) -> Result<Vec<ReplicationOperation>, ()>;

    fn receive_operation(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
        operation: Operation<()>,
    ) -> Result<OperationOutcome, FatalSyncError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ProtectionBundleWire {
    origin: Vec<u8>,
    grant: Vec<u8>,
}

impl ProtectionBundleWire {
    fn validated(self) -> Result<ProtectionBundle, String> {
        if self.origin.len() > MAX_PROTECTION_CONTROL_BYTES
            || self.grant.len() > MAX_PROTECTION_CONTROL_BYTES
        {
            return Err("protection bundle exceeds bound".to_owned());
        }
        Ok(ProtectionBundle {
            origin: self.origin,
            grant: self.grant,
        })
    }

    fn from_bundle(bundle: ProtectionBundle) -> Self {
        Self {
            origin: bundle.origin,
            grant: bundle.grant,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
enum LogSyncWire {
    Hello {
        workspace: [u8; 16],
        package: String,
        membership_version: [u8; 32],
        content_key_id: Option<[u8; 32]>,
    },
    Welcome {
        membership_version: [u8; 32],
        content_key_id: Option<[u8; 32]>,
        protection: Option<ProtectionBundleWire>,
    },
    Ready {
        protection: Option<ProtectionBundleWire>,
    },
    Refused {
        reason: u8,
    },
    Message(LogSyncMessage<[u8; 32]>),
}

/// Classifies one received operation and reports whether it was applied.
///
/// After an author's first rejection or refused park, later operations from
/// that author are dropped for the rest of this session: they would fail the
/// sequence check anyway, and LogSync redelivers them next time.
fn apply_log_sync_event(
    receiver: &Arc<dyn SyncReceiver>,
    peer: PublicIdentity,
    workspace: WorkspaceId,
    package: &str,
    event: LogSyncEvent<()>,
    skipped: &mut BTreeSet<p2panda_core::VerifyingKey>,
) -> Result<bool, String> {
    let LogSyncEvent::OperationReceived { operation, .. } = event else {
        return Ok(false);
    };
    let author = operation.header.verifying_key;
    if skipped.contains(&author) {
        return Ok(false);
    }
    match receiver.receive_operation(peer, workspace, package, *operation) {
        Ok(OperationOutcome::Applied) => Ok(true),
        Ok(OperationOutcome::Duplicate | OperationOutcome::Parked(_)) => Ok(false),
        Ok(OperationOutcome::Rejected(_) | OperationOutcome::ParkRefused) => {
            skipped.insert(author);
            Ok(false)
        }
        Err(FatalSyncError::PeerRevoked) => Err("peer was revoked mid-session".to_owned()),
        Err(FatalSyncError::Storage) => Err("failed to store synchronized operation".to_owned()),
    }
}

async fn request_application_ack(connection: &Connection) -> Result<(), String> {
    let (mut send, mut receive) = connection
        .open_bi()
        .await
        .map_err(|error| error.to_string())?;
    send.write_all(APPLICATION_ACK_REQUEST)
        .await
        .map_err(|error| error.to_string())?;
    send.finish().map_err(|error| error.to_string())?;
    let ack = receive
        .read_to_end(APPLICATION_ACK.len())
        .await
        .map_err(|error| error.to_string())?;
    if ack != APPLICATION_ACK {
        return Err("missing log sync application acknowledgement".to_owned());
    }
    Ok(())
}

async fn acknowledge_application(connection: &Connection) -> Result<(), String> {
    let (mut send, mut receive) = connection
        .accept_bi()
        .await
        .map_err(|error| error.to_string())?;
    let request = receive
        .read_to_end(APPLICATION_ACK_REQUEST.len())
        .await
        .map_err(|error| error.to_string())?;
    if request != APPLICATION_ACK_REQUEST {
        return Err("invalid log sync application acknowledgement request".to_owned());
    }
    send.write_all(APPLICATION_ACK)
        .await
        .map_err(|error| error.to_string())?;
    send.finish().map_err(|error| error.to_string())?;
    send.stopped().await.map_err(|error| error.to_string())?;
    Ok(())
}

#[derive(Default)]
struct LogSyncWriterState {
    pending: AtomicUsize,
    failed: AtomicBool,
    changed: tokio::sync::Notify,
}

impl LogSyncWriterState {
    fn complete(&self, success: bool) {
        if !success {
            self.failed.store(true, Ordering::Release);
        }
        self.pending.fetch_sub(1, Ordering::AcqRel);
        self.changed.notify_waiters();
    }

    async fn flushed(&self) -> Result<(), String> {
        loop {
            let changed = self.changed.notified();
            if self.pending.load(Ordering::Acquire) == 0 {
                return if self.failed.load(Ordering::Acquire) {
                    Err("log sync writer failed".to_owned())
                } else {
                    Ok(())
                };
            }
            changed.await;
        }
    }
}

struct LogSyncIo {
    outbound: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    writer_state: Arc<LogSyncWriterState>,
    inbound: tokio::sync::mpsc::UnboundedReceiver<Result<LogSyncWire, String>>,
}

impl LogSyncIo {
    /// `first_frame_max` bounds the very first inbound frame. A responder caps
    /// it at [`MAX_HELLO_BYTES`], so nothing larger is read before the gate.
    fn new(
        mut send: iroh::endpoint::SendStream,
        mut receive: iroh::endpoint::RecvStream,
        first_frame_max: usize,
    ) -> Self {
        let (outbound, mut outbound_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let writer_state = Arc::new(LogSyncWriterState::default());
        let writer_status = Arc::clone(&writer_state);
        let (inbound_tx, inbound) =
            tokio::sync::mpsc::unbounded_channel::<Result<LogSyncWire, String>>();
        tokio::spawn(async move {
            while let Some(bytes) = outbound_rx.recv().await {
                let success = if let Ok(length) = u32::try_from(bytes.len()) {
                    send.write_all(&length.to_be_bytes()).await.is_ok()
                        && send.write_all(&bytes).await.is_ok()
                } else {
                    false
                };
                writer_status.complete(success);
                if !success {
                    while outbound_rx.try_recv().is_ok() {
                        writer_status.complete(false);
                    }
                    break;
                }
            }
            let _ = send.finish();
        });
        tokio::spawn(async move {
            let mut limit = first_frame_max;
            loop {
                let mut length = [0; 4];
                if receive.read_exact(&mut length).await.is_err() {
                    break;
                }
                let length = u32::from_be_bytes(length) as usize;
                if length > limit {
                    let _ = inbound_tx.send(Err("log sync frame exceeds bound".to_owned()));
                    break;
                }
                limit = MAX_SYNC_BYTES;
                let mut bytes = vec![0; length];
                if receive.read_exact(&mut bytes).await.is_err() {
                    break;
                }
                let message = postcard::from_bytes(&bytes).map_err(|error| error.to_string());
                if inbound_tx.send(message).is_err() {
                    break;
                }
            }
        });
        Self {
            outbound,
            writer_state,
            inbound,
        }
    }

    fn send_wire(&mut self, wire: LogSyncWire) -> Result<(), String> {
        let bytes = postcard::to_stdvec(&wire).map_err(|error| error.to_string())?;
        if bytes.len() > MAX_SYNC_BYTES {
            return Err("log sync frame exceeds bound".to_owned());
        }
        self.writer_state.pending.fetch_add(1, Ordering::AcqRel);
        self.outbound.send(bytes).map_err(|_| {
            self.writer_state.complete(false);
            "log sync writer closed".to_owned()
        })
    }

    async fn next_wire(&mut self) -> Result<LogSyncWire, String> {
        match self.inbound.recv().await {
            Some(Ok(wire)) => Ok(wire),
            Some(Err(error)) => Err(error),
            None => Err("log sync stream closed".to_owned()),
        }
    }
}

impl Sink<LogSyncMessage<[u8; 32]>> for LogSyncIo {
    type Error = String;

    fn poll_ready(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn start_send(
        self: std::pin::Pin<&mut Self>,
        item: LogSyncMessage<[u8; 32]>,
    ) -> Result<(), Self::Error> {
        self.get_mut().send_wire(LogSyncWire::Message(item))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }
}

impl Stream for LogSyncIo {
    type Item = Result<LogSyncMessage<[u8; 32]>, String>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.get_mut().inbound.poll_recv(cx).map(|item| {
            item.map(|wire| match wire {
                Ok(LogSyncWire::Message(message)) => Ok(message),
                Ok(_) => Err("unexpected log sync handshake frame".to_owned()),
                Err(error) => Err(error),
            })
        })
    }
}

/// Runs the p2panda LogSync exchange once both sides have been authorized.
///
/// Returns how many operations were applied locally.
async fn run_log_exchange(
    connection: &Connection,
    io: LogSyncIo,
    receiver: &Arc<dyn SyncReceiver>,
    peer: PublicIdentity,
    workspace: WorkspaceId,
    package: &str,
    initiator: bool,
) -> Result<usize, String> {
    let operations = receiver
        .operations(peer, workspace, package)
        .map_err(|()| "failed to load local operations".to_owned())?;
    let store = crate::synchronization::p2panda_adapter::TeamarnessLogStore::from_operations(
        operations.clone(),
    );
    let mut logs = std::collections::BTreeMap::new();
    let package_log =
        crate::synchronization::operation_store::log_id(scope_digest(package, workspace));
    for author in receiver
        .authors(peer, workspace)
        .map_err(|()| "failed to load workspace authors".to_owned())?
    {
        let author = p2panda_core::VerifyingKey::from_bytes(author.as_bytes())
            .map_err(|error| error.to_string())?;
        logs.entry(author)
            .or_insert_with(Vec::new)
            .push(package_log);
    }
    for (log_id, operation) in operations {
        logs.entry(operation.header.verifying_key)
            .or_insert_with(Vec::new)
            .push(log_id);
    }
    for ids in logs.values_mut() {
        ids.sort();
        ids.dedup();
    }
    let (events, mut event_rx) =
        tokio::sync::broadcast::channel::<LogSyncEvent<()>>(MAX_SYNC_OPERATIONS + 1);
    let protocol = p2panda_sync::protocols::LogSync::new(store, logs, events);
    let writer_state = Arc::clone(&io.writer_state);
    let (mut sink, mut stream) = io.split();
    let mut running = Box::pin(crate::synchronization::p2panda_adapter::run_finite(
        protocol,
        &mut sink,
        &mut stream,
    ));
    let mut accepted = 0;
    // One skipped-author set per session, shared by both drain paths.
    let mut skipped = BTreeSet::new();
    loop {
        tokio::select! {
            result = &mut running => {
                result.map_err(|error| error.to_string())?;
                drop(running);
                writer_state.flushed().await?;
                loop {
                    match event_rx.try_recv() {
                        Ok(event) => {
                            if apply_log_sync_event(receiver, peer, workspace, package, event, &mut skipped)? {
                                accepted += 1;
                            }
                        }
                        Err(tokio::sync::broadcast::error::TryRecvError::Empty
                            | tokio::sync::broadcast::error::TryRecvError::Closed) => break,
                        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {
                            return Err("log sync event delivery lagged".to_owned());
                        }
                    }
                }
                drop(sink);
                drop(stream);
                if initiator {
                    request_application_ack(connection).await?;
                } else {
                    acknowledge_application(connection).await?;
                }
                return Ok(accepted);
            }
            event = event_rx.recv() => match event {
                Ok(event) => {
                    if apply_log_sync_event(receiver, peer, workspace, package, event, &mut skipped)? {
                        accepted += 1;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    return Err("log sync event delivery lagged".to_owned());
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    return Err("log sync event delivery closed".to_owned());
                }
            }
        }
    }
}

/// The initiator half of `teamarness/package-sync/2`.
///
/// The local gate runs before dialling, so an Installation never opens a
/// session for a Workspace it or the peer has left.
pub(crate) async fn outbound_log_session(
    endpoint: &iroh::Endpoint,
    address: iroh_base::EndpointAddr,
    receiver: Arc<dyn SyncReceiver>,
    sessions: Arc<PeerSessions>,
    workspace: WorkspaceId,
    package: String,
    peer: PublicIdentity,
) -> Result<usize, LogSessionError> {
    let mut cancellation = sessions.subscribe(peer);
    let grant = receiver
        .authorize_session(peer, workspace, &package)
        .map_err(LogSessionError::Refused)?;
    tokio::select! {
        result = outbound_session_body(endpoint, address, &receiver, grant, workspace, &package, peer) => result,
        () = peer_sessions::cancelled(&mut cancellation) => Err(LogSessionError::Cancelled),
    }
}

async fn outbound_session_body(
    endpoint: &iroh::Endpoint,
    address: iroh_base::EndpointAddr,
    receiver: &Arc<dyn SyncReceiver>,
    grant: SessionGrant,
    workspace: WorkspaceId,
    package: &str,
    peer: PublicIdentity,
) -> Result<usize, LogSessionError> {
    let connection = endpoint
        .connect(address, LOG_SYNC_ALPN)
        .await
        .map_err(|error| LogSessionError::Transport(error.to_string()))?;
    let (send, receive) = connection
        .open_bi()
        .await
        .map_err(|error| LogSessionError::Transport(error.to_string()))?;
    let mut io = LogSyncIo::new(send, receive, MAX_SYNC_BYTES);
    io.send_wire(LogSyncWire::Hello {
        workspace: *workspace.as_bytes(),
        package: package.to_owned(),
        membership_version: grant.membership_version,
        content_key_id: grant.content_key_id,
    })
    .map_err(LogSessionError::Transport)?;
    let welcome = match io.next_wire().await {
        Ok(wire) => wire,
        Err(error) => return Err(refusal_from_close(&connection, error).await),
    };
    let (remote_version, remote_key, welcome_bundle) = match welcome {
        LogSyncWire::Welcome {
            membership_version,
            content_key_id,
            protection,
        } => (membership_version, content_key_id, protection),
        LogSyncWire::Refused { reason } => {
            return Err(LogSessionError::Refused(
                Refusal::from_reason(reason).unwrap_or(Refusal::Unavailable),
            ));
        }
        _ => return Err(LogSessionError::Protocol("expected a welcome".to_owned())),
    };
    if let (Some(local_key), Some(remote_key)) = (grant.content_key_id, remote_key)
        && local_key != remote_key
    {
        let _ = io.send_wire(LogSyncWire::Refused {
            reason: Refusal::KeyMismatch.reason(),
        });
        connection.close(6_u8.into(), b"key mismatch");
        return Err(LogSessionError::Refused(Refusal::KeyMismatch));
    }
    if let Some(bundle) = welcome_bundle {
        let bundle = bundle.validated().map_err(LogSessionError::Protocol)?;
        let _ = receiver.import_protection_bundle(peer, workspace, package, bundle);
    }
    if remote_version != grant.membership_version {
        receiver.note_membership_mismatch(workspace, peer);
    }
    let offer = if grant.content_key_id.is_some() && remote_key.is_none() {
        receiver
            .protection_bundle_for(workspace, package, peer)
            .unwrap_or(None)
    } else {
        None
    };
    io.send_wire(LogSyncWire::Ready {
        protection: offer.map(ProtectionBundleWire::from_bundle),
    })
    .map_err(LogSessionError::Transport)?;
    run_log_exchange(&connection, io, receiver, peer, workspace, package, true)
        .await
        .map_err(LogSessionError::Transport)
}

/// Maps a stream that ended before `Welcome` onto the responder's close code.
///
/// A refusal is sent as a frame and as a close code; the close code is the
/// reliable one, because closing may drop stream data the peer has not read.
async fn refusal_from_close(connection: &Connection, error: String) -> LogSessionError {
    let closed = match connection.close_reason() {
        Some(reason) => Some(reason),
        None => timeout(Duration::from_secs(2), connection.closed())
            .await
            .ok(),
    };
    if let Some(iroh::endpoint::ConnectionError::ApplicationClosed(close)) = closed
        && let Some(refusal) = Refusal::from_close_code(u64::from(close.error_code))
    {
        return LogSessionError::Refused(refusal);
    }
    LogSessionError::Transport(error)
}

pub(crate) struct LogSyncHandler {
    stopping: AtomicBool,
    services: SyncServices,
}

impl std::fmt::Debug for LogSyncHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LogSyncHandler")
            .finish_non_exhaustive()
    }
}

impl LogSyncHandler {
    pub(crate) fn new(services: SyncServices) -> Self {
        Self {
            stopping: AtomicBool::new(false),
            services,
        }
    }

    pub(crate) fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
    }
}

/// The responder half of `teamarness/package-sync/2`.
///
/// Nothing is served — not even the Workspace author list — until both this
/// Installation and the authenticated remote peer are current Members.
async fn run_log_session(
    connection: Connection,
    services: SyncServices,
    peer: PublicIdentity,
) -> Result<(), LogSessionError> {
    let mut cancellation = services.sessions.subscribe(peer);
    tokio::select! {
        result = run_log_session_body(&connection, &services.receiver, peer) => result,
        () = peer_sessions::cancelled(&mut cancellation) => Err(LogSessionError::Cancelled),
    }
}

async fn run_log_session_body(
    connection: &Connection,
    receiver: &Arc<dyn SyncReceiver>,
    peer: PublicIdentity,
) -> Result<(), LogSessionError> {
    let (send, receive) = connection
        .accept_bi()
        .await
        .map_err(|error| LogSessionError::Transport(error.to_string()))?;
    let mut io = LogSyncIo::new(send, receive, MAX_HELLO_BYTES);
    let (workspace, package, remote_version, remote_key) =
        match io.next_wire().await.map_err(LogSessionError::Transport)? {
            LogSyncWire::Hello {
                workspace,
                package,
                membership_version,
                content_key_id,
            } => {
                if package.is_empty() || package.len() > MAX_PACKAGE_IDENTITY || !package.is_ascii()
                {
                    return Err(LogSessionError::Protocol(
                        "invalid package identity".to_owned(),
                    ));
                }
                (
                    WorkspaceId::from_bytes(workspace),
                    package,
                    membership_version,
                    content_key_id,
                )
            }
            _ => return Err(LogSessionError::Protocol("missing hello".to_owned())),
        };
    let grant = match receiver.authorize_session(peer, workspace, &package) {
        Ok(grant) => grant,
        Err(refusal) => return Err(refuse(&mut io, connection, refusal).await),
    };
    if let (Some(remote_key), Some(local_key)) = (remote_key, grant.content_key_id)
        && remote_key != local_key
    {
        return Err(refuse(&mut io, connection, Refusal::KeyMismatch).await);
    }
    let bundle = if grant.content_key_id.is_some() && remote_key.is_none() {
        receiver
            .protection_bundle_for(workspace, &package, peer)
            .unwrap_or(None)
    } else {
        None
    };
    io.send_wire(LogSyncWire::Welcome {
        membership_version: grant.membership_version,
        content_key_id: grant.content_key_id,
        protection: bundle.map(ProtectionBundleWire::from_bundle),
    })
    .map_err(LogSessionError::Transport)?;
    if remote_version != grant.membership_version {
        receiver.note_membership_mismatch(workspace, peer);
    }
    match io.next_wire().await.map_err(LogSessionError::Transport)? {
        LogSyncWire::Ready { protection } => {
            if let Some(bundle) = protection {
                let bundle = bundle.validated().map_err(LogSessionError::Protocol)?;
                let _ = receiver.import_protection_bundle(peer, workspace, &package, bundle);
            }
        }
        LogSyncWire::Refused { reason } => {
            return Err(LogSessionError::Refused(
                Refusal::from_reason(reason).unwrap_or(Refusal::Unavailable),
            ));
        }
        _ => return Err(LogSessionError::Protocol("expected ready".to_owned())),
    }
    run_log_exchange(connection, io, receiver, peer, workspace, &package, false)
        .await
        .map(|_| ())
        .map_err(LogSessionError::Transport)
}

/// Sends the refusal frame, flushes it, and closes with the matching code.
async fn refuse(io: &mut LogSyncIo, connection: &Connection, refusal: Refusal) -> LogSessionError {
    let _ = io.send_wire(LogSyncWire::Refused {
        reason: refusal.reason(),
    });
    let _ = io.writer_state.flushed().await;
    match refusal {
        Refusal::NotAuthorized => connection.close(5_u8.into(), b"not authorized"),
        Refusal::Unavailable => connection.close(1_u8.into(), b"unavailable"),
        Refusal::KeyMismatch => connection.close(6_u8.into(), b"key mismatch"),
    }
    LogSessionError::Refused(refusal)
}

impl ProtocolHandler for LogSyncHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        if self.stopping.load(Ordering::Acquire) {
            connection.close(CLOSE_UNAVAILABLE.into(), b"package sync unavailable");
            return Ok(());
        }
        let peer = PublicIdentity::from_bytes(*connection.remote_id().as_bytes())
            .map_err(|_| AcceptError::from(std::io::Error::other("invalid peer")))?;
        let result = timeout(
            SYNC_SESSION_TIMEOUT,
            run_log_session(connection.clone(), self.services.clone(), peer),
        )
        .await;
        match result {
            Ok(Ok(())) => {}
            // A refusal has already closed the connection with its own code.
            Ok(Err(LogSessionError::Refused(_))) => {}
            Ok(Err(LogSessionError::Cancelled)) => {
                connection.close(CLOSE_INVALID.into(), b"session cancelled");
            }
            Ok(Err(_)) | Err(_) => {
                connection.close(CLOSE_INVALID.into(), b"log sync invalid or timed out");
            }
        }
        Ok(())
    }

    async fn shutdown(&self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    /// A scripted Core seam. It exists only to inject a rejection: everything
    /// under test — the skipped-author set and the session loop — is real.
    #[derive(Default)]
    struct ScriptedReceiver {
        rejected_author: Mutex<Option<p2panda_core::VerifyingKey>>,
        seen: Mutex<Vec<p2panda_core::VerifyingKey>>,
    }

    impl SyncReceiver for ScriptedReceiver {
        fn authorize_session(
            &self,
            _peer: PublicIdentity,
            _workspace: WorkspaceId,
            _package: &str,
        ) -> Result<SessionGrant, Refusal> {
            Ok(SessionGrant {
                membership_version: [0; 32],
                content_key_id: None,
            })
        }

        fn protection_bundle_for(
            &self,
            _workspace: WorkspaceId,
            _package: &str,
            _recipient: PublicIdentity,
        ) -> Result<Option<ProtectionBundle>, ()> {
            Ok(None)
        }

        fn import_protection_bundle(
            &self,
            _peer: PublicIdentity,
            _workspace: WorkspaceId,
            _package: &str,
            _bundle: ProtectionBundle,
        ) -> Result<usize, ()> {
            Ok(0)
        }

        fn authors(
            &self,
            _peer: PublicIdentity,
            _workspace: WorkspaceId,
        ) -> Result<Vec<PublicIdentity>, ()> {
            Ok(Vec::new())
        }

        fn operations(
            &self,
            _peer: PublicIdentity,
            _workspace: WorkspaceId,
            _package: &str,
        ) -> Result<Vec<ReplicationOperation>, ()> {
            Ok(Vec::new())
        }

        fn receive_operation(
            &self,
            _peer: PublicIdentity,
            _workspace: WorkspaceId,
            _package: &str,
            operation: Operation<()>,
        ) -> Result<OperationOutcome, FatalSyncError> {
            let author = operation.header.verifying_key;
            self.seen.lock().unwrap().push(author);
            if *self.rejected_author.lock().unwrap() == Some(author) {
                Ok(OperationOutcome::Rejected(RejectReason::Conflict))
            } else {
                Ok(OperationOutcome::Applied)
            }
        }
    }

    fn signed_operation(seed: u8, sequence: u32) -> Operation<()> {
        let key = p2panda_core::SigningKey::from_bytes(&[seed; 32]);
        let payload = vec![seed, u8::try_from(sequence).unwrap()];
        let header = p2panda_core::Header::<()>::builder()
            .body(payload.as_slice())
            .seq_num(sequence)
            .build(&key, ());
        Operation::from_parts(header, Some(p2panda_core::Body::from_bytes(&payload)))
    }

    fn received(operation: Operation<()>) -> LogSyncEvent<()> {
        LogSyncEvent::OperationReceived {
            operation: Box::new(operation),
            metrics: p2panda_sync::protocols::LogSyncMetrics::default(),
        }
    }

    #[test]
    fn session_skips_an_author_after_rejection_and_continues_with_others() {
        let scripted = Arc::new(ScriptedReceiver::default());
        let rejected = signed_operation(1, 0);
        *scripted.rejected_author.lock().unwrap() = Some(rejected.header.verifying_key);
        let receiver: Arc<dyn SyncReceiver> = scripted.clone();
        let workspace = WorkspaceId::from_bytes([4; 16]);
        let peer = PublicIdentity::from_bytes(
            *iroh_base::SecretKey::from_bytes(&[9; 32])
                .public()
                .as_bytes(),
        )
        .unwrap();
        let mut skipped = BTreeSet::new();

        let mut applied = 0;
        for event in [
            received(rejected.clone()),
            received(signed_operation(1, 1)),
            received(signed_operation(2, 0)),
        ] {
            if apply_log_sync_event(
                &receiver,
                peer,
                workspace,
                "teamarness.test",
                event,
                &mut skipped,
            )
            .unwrap()
            {
                applied += 1;
            }
        }

        assert_eq!(applied, 1, "only the other author's operation applied");
        let seen = scripted.seen.lock().unwrap().clone();
        assert_eq!(
            seen.iter()
                .filter(|author| **author == rejected.header.verifying_key)
                .count(),
            1,
            "the rejected author's later operation never reached the receiver"
        );
        assert_eq!(seen.len(), 2);
    }

    #[test]
    fn refusal_reasons_and_close_codes_are_distinct_value_spaces() {
        assert_eq!(Refusal::NotAuthorized.reason(), 1);
        assert_eq!(Refusal::Unavailable.reason(), 2);
        assert_eq!(Refusal::KeyMismatch.reason(), 3);
        for refusal in [
            Refusal::NotAuthorized,
            Refusal::Unavailable,
            Refusal::KeyMismatch,
        ] {
            assert_eq!(Refusal::from_reason(refusal.reason()), Some(refusal));
        }
        assert_eq!(
            Refusal::from_close_code(u64::from(CLOSE_NOT_AUTHORIZED)),
            Some(Refusal::NotAuthorized)
        );
        assert_eq!(
            Refusal::from_close_code(u64::from(CLOSE_UNAVAILABLE)),
            Some(Refusal::Unavailable)
        );
        assert_eq!(
            Refusal::from_close_code(u64::from(CLOSE_KEY_MISMATCH)),
            Some(Refusal::KeyMismatch)
        );
        assert_eq!(Refusal::from_reason(9), None);
        assert_eq!(Refusal::from_close_code(u64::from(CLOSE_INVALID)), None);
    }

    #[test]
    fn handshake_frames_round_trip_within_the_hello_bound() {
        let hello = LogSyncWire::Hello {
            workspace: [3; 16],
            package: "teamarness.test".to_owned(),
            membership_version: [4; 32],
            content_key_id: Some([5; 32]),
        };
        let bytes = postcard::to_stdvec(&hello).unwrap();
        assert!(bytes.len() <= MAX_HELLO_BYTES);
        assert!(matches!(
            postcard::from_bytes::<LogSyncWire>(&bytes).unwrap(),
            LogSyncWire::Hello { package, .. } if package == "teamarness.test"
        ));
        let welcome = LogSyncWire::Welcome {
            membership_version: [6; 32],
            content_key_id: None,
            protection: Some(ProtectionBundleWire {
                origin: vec![1; 354],
                grant: vec![2; 354],
            }),
        };
        let bytes = postcard::to_stdvec(&welcome).unwrap();
        assert!(matches!(
            postcard::from_bytes::<LogSyncWire>(&bytes).unwrap(),
            LogSyncWire::Welcome {
                protection: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn oversized_protection_bundles_are_refused() {
        assert!(
            ProtectionBundleWire {
                origin: vec![0; MAX_PROTECTION_CONTROL_BYTES + 1],
                grant: Vec::new(),
            }
            .validated()
            .is_err()
        );
        assert!(
            ProtectionBundleWire {
                origin: Vec::new(),
                grant: vec![0; MAX_PROTECTION_CONTROL_BYTES],
            }
            .validated()
            .is_ok()
        );
    }
}
