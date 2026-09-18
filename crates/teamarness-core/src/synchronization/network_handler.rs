use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::synchronization::exchange::{ExchangeContext, ExchangeReport, Role, run_exchange};
use crate::synchronization::frames::{
    FrameError, FramePhase, FrameReader, LogSyncWire, ProtectionBundleWire, frame_sink,
};
use crate::synchronization::limits::SessionLimits;
use crate::synchronization::parking::ParkReason;
use crate::synchronization::peer_sessions::{self, PeerSessions};
use crate::synchronization::{SyncServices, scope_digest};
use crate::{PublicIdentity, WorkspaceId};
use futures_util::SinkExt;
use p2panda_core::Operation;

use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use tokio::sync::Semaphore;
use tokio::time::timeout;

pub(crate) const LOG_SYNC_ALPN: &[u8] = b"teamarness/package-sync/2";

const MAX_PACKAGE_IDENTITY: usize = 256;
const APPLICATION_ACK_REQUEST: &[u8] = b"teamarness/log-sync/applied/2";
const APPLICATION_ACK: &[u8] = b"teamarness/log-sync/applied/2/ok";

/// QUIC close codes for `teamarness/package-sync/2` (design §5.6). A `Refused`
/// frame reason is a different value space and is never used as a close code.
const CLOSE_UNAVAILABLE: u8 = 1;
const CLOSE_OVERLOADED: u8 = 2;
const CLOSE_PEER_BUSY: u8 = 3;
const CLOSE_INVALID: u8 = 4;
const CLOSE_NOT_AUTHORIZED: u8 = 5;
const CLOSE_KEY_MISMATCH: u8 = 6;
const CLOSE_BUDGET_EXCEEDED: u8 = 7;

/// One operation as LogSync serves it: the operation and its header bytes.
pub(crate) type ReplicationEntry = (Operation<()>, Vec<u8>);

/// What one session advertises about the log it can serve.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SessionSeed {
    pub(crate) authors: Vec<PublicIdentity>,
    pub(crate) heights: std::collections::BTreeMap<PublicIdentity, u32>,
}

/// Why one page of a served log could not be loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RangeLoadError {
    Unavailable,
    /// The stored projection no longer authenticates, so the scope is
    /// quarantined and this session ends.
    ScopeCorrupt,
}

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

    /// The design's close-code table (§5.6), read back. The `Refused` frame
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
    /// The order key claimed a causal height far beyond this scope's own.
    HeightOutOfRange,
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
    /// The local gate refused before anything was dialled, so the peer was
    /// never contacted.
    LocalNotAuthorized,
    Cancelled,
    /// The peer is already running as many sessions as it allows, globally or
    /// with this Installation. Trying again later is the right response.
    PeerBusy,
    /// The peer sent more than its session budget allows.
    BudgetExceeded,
    /// The peer claimed a frame larger than the phase allows.
    FrameTooLarge,
    Transport(String),
    Protocol(String),
}

impl std::fmt::Display for LogSessionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(refusal) => write!(formatter, "package sync refused: {refusal:?}"),
            Self::LocalNotAuthorized => {
                formatter.write_str("package sync is not authorized locally")
            }
            Self::Cancelled => formatter.write_str("package sync session cancelled"),
            Self::PeerBusy => formatter.write_str("package sync peer is busy"),
            Self::BudgetExceeded => formatter.write_str("package sync session budget exceeded"),
            Self::FrameTooLarge => formatter.write_str("package sync frame exceeds its bound"),
            Self::Transport(error) => write!(formatter, "package sync transport: {error}"),
            Self::Protocol(error) => write!(formatter, "package sync protocol: {error}"),
        }
    }
}

impl LogSessionError {
    /// The close code this failure ends the connection with.
    const fn close_code(&self) -> u8 {
        match self {
            Self::BudgetExceeded => CLOSE_BUDGET_EXCEEDED,
            _ => CLOSE_INVALID,
        }
    }

    const fn close_reason(&self) -> &'static [u8] {
        match self {
            Self::BudgetExceeded => b"package sync session budget exceeded",
            Self::Cancelled => b"session cancelled",
            _ => b"log sync invalid or timed out",
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

    /// The author list and heights one session advertises, loaded once.
    ///
    /// This re-checks the membership gate, and reports an author's height as
    /// one below its lowest quarantined sequence, so a peer is never offered a
    /// log with a hole in it.
    fn session_seed(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
    ) -> Result<SessionSeed, Refusal>;

    /// The sizes of one author's operations in sequence order, for planning
    /// what fits in this session's send budget.
    #[allow(clippy::too_many_arguments)]
    fn range_sizes(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
        author: PublicIdentity,
        after: Option<u32>,
        until: Option<u32>,
        limit: usize,
    ) -> Result<Vec<(u32, usize)>, ()>;

    /// One page of an author's operations, authenticated against the stored
    /// projection as it is read.
    #[allow(clippy::too_many_arguments)]
    fn load_range_page(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
        author: PublicIdentity,
        from: u32,
        until: u32,
        rows: usize,
    ) -> Result<Vec<ReplicationEntry>, RangeLoadError>;

    fn receive_operation(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
        operation: Operation<()>,
    ) -> Result<OperationOutcome, FatalSyncError>;

    /// Applies a batch of operations in one lock hold, in order.
    ///
    /// The default is the per-operation path, so fakes stay small; the runtime
    /// overrides it to take the state lock once per batch.
    fn receive_operations(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
        operations: Vec<Operation<()>>,
    ) -> Result<Vec<OperationOutcome>, FatalSyncError> {
        operations
            .into_iter()
            .map(|operation| self.receive_operation(peer, workspace, package, operation))
            .collect()
    }
}

/// Requests the application acknowledgement and learns whether the peer had
/// more to send than its own budget allowed.
async fn request_application_ack(
    connection: &Connection,
    truncated: bool,
) -> Result<bool, LogSessionError> {
    let (mut send, mut receive) = connection
        .open_bi()
        .await
        .map_err(|error| transport_error(connection, &error))?;
    let mut request = APPLICATION_ACK_REQUEST.to_vec();
    request.push(u8::from(truncated));
    send.write_all(&request)
        .await
        .map_err(|error| transport_error(connection, &error))?;
    send.finish()
        .map_err(|error| transport_error(connection, &error))?;
    let ack = receive
        .read_to_end(APPLICATION_ACK.len() + 1)
        .await
        .map_err(|error| transport_error(connection, &error))?;
    parse_truncated_suffix(&ack, APPLICATION_ACK)
}

/// Answers the application acknowledgement and reports this side's own state.
async fn acknowledge_application(
    connection: &Connection,
    truncated: bool,
) -> Result<bool, LogSessionError> {
    let (mut send, mut receive) = connection
        .accept_bi()
        .await
        .map_err(|error| transport_error(connection, &error))?;
    let request = receive
        .read_to_end(APPLICATION_ACK_REQUEST.len() + 1)
        .await
        .map_err(|error| transport_error(connection, &error))?;
    let peer_truncated = parse_truncated_suffix(&request, APPLICATION_ACK_REQUEST)?;
    let mut reply = APPLICATION_ACK.to_vec();
    reply.push(u8::from(truncated));
    send.write_all(&reply)
        .await
        .map_err(|error| transport_error(connection, &error))?;
    send.finish()
        .map_err(|error| transport_error(connection, &error))?;
    send.stopped()
        .await
        .map_err(|error| transport_error(connection, &error))?;
    Ok(peer_truncated)
}

/// Parses an acknowledgement of exactly `prefix` followed by one boolean byte.
fn parse_truncated_suffix(bytes: &[u8], prefix: &[u8]) -> Result<bool, LogSessionError> {
    if bytes.len() != prefix.len() + 1 || &bytes[..prefix.len()] != prefix {
        return Err(LogSessionError::Protocol(
            "invalid log sync application acknowledgement".to_owned(),
        ));
    }
    match bytes[prefix.len()] {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(LogSessionError::Protocol(
            "invalid log sync truncation flag".to_owned(),
        )),
    }
}

/// Loads what this side will offer and runs the exchange.
#[allow(clippy::too_many_arguments)]
async fn exchange_and_acknowledge(
    connection: &Connection,
    reader: &mut FrameReader,
    sink: &mut (impl futures_util::Sink<LogSyncWire, Error = FrameError> + Unpin + Send),
    receiver: &Arc<dyn SyncReceiver>,
    health: &Arc<crate::synchronization::SyncHealthRegistry>,
    context: ExchangeContext,
    limits: SessionLimits,
) -> Result<ExchangeReport, LogSessionError> {
    let peer = context.peer;
    let workspace = context.workspace;
    let scope = context.package.clone();
    // The author list and heights are read once; the operations themselves are
    // read lazily, in pages, only for what fits this session's send budget.
    let seed = crate::blocking::receiver_call(receiver, move |receiver| {
        receiver.session_seed(peer, workspace, &scope)
    })
    .await
    .map_err(|_| LogSessionError::Transport("runtime state is unavailable".to_owned()))?
    .map_err(LogSessionError::Refused)?;
    let log_id =
        crate::synchronization::operation_store::log_id(scope_digest(&context.package, workspace));
    let mut logs = std::collections::BTreeMap::new();
    for author in &seed.authors {
        let author = p2panda_core::VerifyingKey::from_bytes(author.as_bytes())
            .map_err(|error| LogSessionError::Protocol(error.to_string()))?;
        logs.insert(author, vec![log_id]);
    }
    let view = crate::synchronization::log_view::SqliteLogView::new(
        Arc::clone(receiver),
        crate::synchronization::log_view::ViewContext {
            peer,
            workspace,
            package: context.package.clone(),
            log_id,
        },
        seed,
        limits,
    );
    let role = context.role;
    let scope = scope_digest(&context.package, workspace);
    let outcomes = run_exchange(
        reader,
        sink,
        view.clone(),
        logs,
        Arc::clone(receiver),
        context,
        limits,
        Some(health.session_incoming(scope)),
    )
    .await?;
    let sent_truncated = view.truncated();
    let peer_truncated = match role {
        Role::Initiator => request_application_ack(connection, sent_truncated).await?,
        Role::Responder => acknowledge_application(connection, sent_truncated).await?,
    };
    Ok(ExchangeReport {
        outcomes,
        sent_truncated,
        peer_truncated,
    })
}

/// The initiator half of `teamarness/package-sync/2`.
///
/// The local gate runs before dialling, so an Installation never opens a
/// session for a Workspace it or the peer has left.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn outbound_log_session(
    endpoint: &iroh::Endpoint,
    address: iroh_base::EndpointAddr,
    receiver: Arc<dyn SyncReceiver>,
    sessions: Arc<PeerSessions>,
    health: Arc<crate::synchronization::SyncHealthRegistry>,
    workspace: WorkspaceId,
    package: String,
    peer: PublicIdentity,
    limits: SessionLimits,
) -> Result<ExchangeReport, LogSessionError> {
    let mut cancellation = sessions.subscribe(peer);
    let scope = package.clone();
    let grant = crate::blocking::receiver_call(&receiver, move |receiver| {
        receiver.authorize_session(peer, workspace, &scope)
    })
    .await
    .map_err(|_| LogSessionError::Transport("runtime state is unavailable".to_owned()))?
    .map_err(|_| LogSessionError::LocalNotAuthorized)?;
    tokio::select! {
        result = outbound_session_body(
            endpoint, address, &receiver, &health, grant, workspace, &package, peer, limits,
        ) => result,
        () = peer_sessions::cancelled(&mut cancellation) => Err(LogSessionError::Cancelled),
    }
}

#[allow(clippy::too_many_arguments)]
async fn outbound_session_body(
    endpoint: &iroh::Endpoint,
    address: iroh_base::EndpointAddr,
    receiver: &Arc<dyn SyncReceiver>,
    health: &Arc<crate::synchronization::SyncHealthRegistry>,
    grant: SessionGrant,
    workspace: WorkspaceId,
    package: &str,
    peer: PublicIdentity,
    limits: SessionLimits,
) -> Result<ExchangeReport, LogSessionError> {
    // A peer that has gone away must be discovered well inside the session
    // budget, so the dial has its own, shorter deadline.
    let connection = timeout(
        limits.connect_timeout,
        endpoint.connect(address, LOG_SYNC_ALPN),
    )
    .await
    .map_err(|_| LogSessionError::Transport("package sync dial timed out".to_owned()))?
    .map_err(connect_error)?;
    let (send, receive) = connection
        .open_bi()
        .await
        .map_err(|error| transport_error(&connection, &error))?;
    let mut reader = FrameReader::spawn(receive, limits);
    // The initiator's next inbound frame is the `Welcome`.
    reader.set_phase(FramePhase::Handshake);
    let mut sink = frame_sink(send, limits);
    sink.send(LogSyncWire::Hello {
        workspace: *workspace.as_bytes(),
        package: package.to_owned(),
        membership_version: grant.membership_version,
        content_key_id: grant.content_key_id,
    })
    .await
    .map_err(|error| transport_error(&connection, &error))?;
    let welcome = match reader.next_wire().await {
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
        let _ = sink
            .send(LogSyncWire::Refused {
                reason: Refusal::KeyMismatch.reason(),
            })
            .await;
        connection.close(CLOSE_KEY_MISMATCH.into(), b"key mismatch");
        return Err(LogSessionError::Refused(Refusal::KeyMismatch));
    }
    if let Some(bundle) = welcome_bundle {
        let bundle = bundle.validated().map_err(LogSessionError::Protocol)?;
        let scope = package.to_owned();
        let _ = crate::blocking::receiver_call(receiver, move |receiver| {
            receiver.import_protection_bundle(peer, workspace, &scope, bundle)
        })
        .await;
    }
    if remote_version != grant.membership_version {
        let _ = crate::blocking::receiver_call(receiver, move |receiver| {
            receiver.note_membership_mismatch(workspace, peer);
        })
        .await;
    }
    let offer = if grant.content_key_id.is_some() && remote_key.is_none() {
        let scope = package.to_owned();
        crate::blocking::receiver_call(receiver, move |receiver| {
            receiver.protection_bundle_for(workspace, &scope, peer)
        })
        .await
        .unwrap_or(Ok(None))
        .unwrap_or(None)
    } else {
        None
    };
    sink.send(LogSyncWire::Ready {
        protection: offer.map(ProtectionBundleWire::from_bundle),
    })
    .await
    .map_err(|error| transport_error(&connection, &error))?;
    reader.set_phase(FramePhase::Session);
    let result = exchange_and_acknowledge(
        &connection,
        &mut reader,
        &mut sink,
        receiver,
        health,
        ExchangeContext {
            peer,
            workspace,
            package: package.to_owned(),
            role: Role::Initiator,
        },
        limits,
    )
    .await;
    match result {
        Ok(report) => Ok(report),
        // The peer's close code says why it stopped; the local stream error is
        // only the symptom.
        Err(error) => Err(peer_close_reason(&connection).unwrap_or(error)),
    }
}

/// Reads the reason a peer gave for closing, when it gave one this side knows.
fn peer_close_reason(connection: &Connection) -> Option<LogSessionError> {
    let iroh::endpoint::ConnectionError::ApplicationClosed(close) = connection.close_reason()?
    else {
        return None;
    };
    close_code_reason(u64::from(close.error_code))
}

/// Maps a stream failure onto the reason the peer gave for closing, when it
/// gave one. A write or read that fails because the peer closed the connection
/// says nothing useful on its own.
fn transport_error(connection: &Connection, error: &impl std::fmt::Display) -> LogSessionError {
    peer_close_reason(connection).unwrap_or_else(|| LogSessionError::Transport(error.to_string()))
}

/// Maps a dial that the responder refused onto the reason it gave.
///
/// A responder that is at its concurrency bound closes the connection during
/// the dial, so the reason arrives as a connect failure rather than as a frame.
fn connect_error(error: iroh::endpoint::ConnectError) -> LogSessionError {
    if let iroh::endpoint::ConnectError::Connection {
        source: iroh::endpoint::ConnectionError::ApplicationClosed(close),
        ..
    } = &error
        && let Some(reason) = close_code_reason(u64::from(close.error_code))
    {
        return reason;
    }
    LogSessionError::Transport(error.to_string())
}

/// Names the failure a peer's close code stands for, when this side knows it.
fn close_code_reason(code: u64) -> Option<LogSessionError> {
    if let Some(refusal) = Refusal::from_close_code(code) {
        return Some(LogSessionError::Refused(refusal));
    }
    match u8::try_from(code).ok()? {
        CLOSE_OVERLOADED | CLOSE_PEER_BUSY => Some(LogSessionError::PeerBusy),
        CLOSE_BUDGET_EXCEEDED => Some(LogSessionError::BudgetExceeded),
        _ => None,
    }
}

/// Maps a stream that ended before `Welcome` onto the responder's close code.
///
/// A refusal is sent as a frame and as a close code; the close code is the
/// reliable one, because closing may drop stream data the peer has not read.
async fn refusal_from_close(connection: &Connection, error: FrameError) -> LogSessionError {
    let closed = match connection.close_reason() {
        Some(reason) => Some(reason),
        None => timeout(std::time::Duration::from_secs(2), connection.closed())
            .await
            .ok(),
    };
    if let Some(iroh::endpoint::ConnectionError::ApplicationClosed(close)) = closed
        && let Some(reason) = close_code_reason(u64::from(close.error_code))
    {
        return reason;
    }
    LogSessionError::Transport(error.to_string())
}

/// Releases one inbound slot and one per-peer slot when a session ends.
struct InboundSlot {
    per_peer: Arc<Mutex<HashMap<PublicIdentity, usize>>>,
    active: Arc<AtomicUsize>,
    peer: PublicIdentity,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl Drop for InboundSlot {
    fn drop(&mut self) {
        let mut peers = self.per_peer.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = peers.get_mut(&self.peer) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                peers.remove(&self.peer);
            }
        }
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(crate) struct LogSyncHandler {
    stopping: AtomicBool,
    inbound: Arc<Semaphore>,
    per_peer: Arc<Mutex<HashMap<PublicIdentity, usize>>>,
    active: Arc<AtomicUsize>,
    services: SyncServices,
    limits: SessionLimits,
}

impl std::fmt::Debug for LogSyncHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LogSyncHandler")
            .finish_non_exhaustive()
    }
}

impl LogSyncHandler {
    pub(crate) fn new(services: SyncServices, limits: SessionLimits) -> Self {
        Self {
            stopping: AtomicBool::new(false),
            inbound: Arc::new(Semaphore::new(limits.inbound_sessions)),
            per_peer: Arc::new(Mutex::new(HashMap::new())),
            active: Arc::new(AtomicUsize::new(0)),
            services,
            limits,
        }
    }

    pub(crate) fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
    }

    /// How many inbound sessions hold a slot right now.
    #[cfg(test)]
    pub(crate) fn active_for_test(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }

    /// Takes an inbound slot and a per-peer slot, or names which bound refused.
    fn take_slot(&self, peer: PublicIdentity) -> Result<InboundSlot, (u8, &'static [u8])> {
        let permit = Arc::clone(&self.inbound)
            .try_acquire_owned()
            .map_err(|_| (CLOSE_OVERLOADED, b"package sync overloaded".as_slice()))?;
        {
            let mut peers = self.per_peer.lock().unwrap_or_else(PoisonError::into_inner);
            let count = peers.entry(peer).or_insert(0);
            if *count >= self.limits.inbound_sessions_per_peer {
                return Err((CLOSE_PEER_BUSY, b"package sync peer busy".as_slice()));
            }
            *count += 1;
        }
        self.active.fetch_add(1, Ordering::AcqRel);
        Ok(InboundSlot {
            per_peer: Arc::clone(&self.per_peer),
            active: Arc::clone(&self.active),
            peer,
            _permit: permit,
        })
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
    limits: SessionLimits,
) -> Result<(), LogSessionError> {
    let mut cancellation = services.sessions.subscribe(peer);
    tokio::select! {
        result = run_log_session_body(&connection, &services, peer, limits) => result,
        () = peer_sessions::cancelled(&mut cancellation) => Err(LogSessionError::Cancelled),
    }
}

async fn run_log_session_body(
    connection: &Connection,
    services: &SyncServices,
    peer: PublicIdentity,
    limits: SessionLimits,
) -> Result<(), LogSessionError> {
    let receiver = &services.receiver;
    let (send, receive) = connection
        .accept_bi()
        .await
        .map_err(|error| transport_error(connection, &error))?;
    let mut reader = FrameReader::spawn(receive, limits);
    let mut sink = frame_sink(send, limits);
    // A peer that says nothing must not hold an inbound slot for the whole
    // session, so the opening frame has its own short deadline.
    let hello = timeout(limits.hello_timeout, reader.next_wire())
        .await
        .map_err(|_| LogSessionError::Protocol("package sync hello timed out".to_owned()))?
        .map_err(|error| transport_error(connection, &error))?;
    let (workspace, package, remote_version, remote_key) = match hello {
        LogSyncWire::Hello {
            workspace,
            package,
            membership_version,
            content_key_id,
        } => {
            if package.is_empty() || package.len() > MAX_PACKAGE_IDENTITY || !package.is_ascii() {
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
    let scope = package.clone();
    let authorized = crate::blocking::receiver_call(receiver, move |receiver| {
        receiver.authorize_session(peer, workspace, &scope)
    })
    .await
    .unwrap_or(Err(Refusal::Unavailable));
    let grant = match authorized {
        Ok(grant) => grant,
        Err(refusal) => return Err(refuse(&mut sink, connection, refusal).await),
    };
    // Past the gate the peer may send the larger handshake frames.
    reader.set_phase(FramePhase::Handshake);
    if let (Some(remote_key), Some(local_key)) = (remote_key, grant.content_key_id)
        && remote_key != local_key
    {
        return Err(refuse(&mut sink, connection, Refusal::KeyMismatch).await);
    }
    let bundle = if grant.content_key_id.is_some() && remote_key.is_none() {
        let scope = package.clone();
        crate::blocking::receiver_call(receiver, move |receiver| {
            receiver.protection_bundle_for(workspace, &scope, peer)
        })
        .await
        .unwrap_or(Ok(None))
        .unwrap_or(None)
    } else {
        None
    };
    sink.send(LogSyncWire::Welcome {
        membership_version: grant.membership_version,
        content_key_id: grant.content_key_id,
        protection: bundle.map(ProtectionBundleWire::from_bundle),
    })
    .await
    .map_err(|error| transport_error(connection, &error))?;
    if remote_version != grant.membership_version {
        let _ = crate::blocking::receiver_call(receiver, move |receiver| {
            receiver.note_membership_mismatch(workspace, peer);
        })
        .await;
    }
    match reader
        .next_wire()
        .await
        .map_err(|error| transport_error(connection, &error))?
    {
        LogSyncWire::Ready { protection } => {
            if let Some(bundle) = protection {
                let bundle = bundle.validated().map_err(LogSessionError::Protocol)?;
                let scope = package.clone();
                let _ = crate::blocking::receiver_call(receiver, move |receiver| {
                    receiver.import_protection_bundle(peer, workspace, &scope, bundle)
                })
                .await;
            }
        }
        LogSyncWire::Refused { reason } => {
            return Err(LogSessionError::Refused(
                Refusal::from_reason(reason).unwrap_or(Refusal::Unavailable),
            ));
        }
        _ => return Err(LogSessionError::Protocol("expected ready".to_owned())),
    }
    reader.set_phase(FramePhase::Session);
    exchange_and_acknowledge(
        connection,
        &mut reader,
        &mut sink,
        receiver,
        &services.health,
        ExchangeContext {
            peer,
            workspace,
            package,
            role: Role::Responder,
        },
        limits,
    )
    .await
    .map(|_| ())
}

/// Sends the refusal frame and closes with the matching code.
async fn refuse(
    sink: &mut (impl futures_util::Sink<LogSyncWire, Error = FrameError> + Unpin),
    connection: &Connection,
    refusal: Refusal,
) -> LogSessionError {
    let _ = sink
        .send(LogSyncWire::Refused {
            reason: refusal.reason(),
        })
        .await;
    match refusal {
        Refusal::NotAuthorized => connection.close(CLOSE_NOT_AUTHORIZED.into(), b"not authorized"),
        Refusal::Unavailable => connection.close(CLOSE_UNAVAILABLE.into(), b"unavailable"),
        Refusal::KeyMismatch => connection.close(CLOSE_KEY_MISMATCH.into(), b"key mismatch"),
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
        // Concurrency is bounded before a single byte is read from the peer.
        let slot = match self.take_slot(peer) {
            Ok(slot) => slot,
            Err((code, reason)) => {
                connection.close(code.into(), reason);
                return Ok(());
            }
        };
        let result = timeout(
            self.limits.session_timeout,
            run_log_session(connection.clone(), self.services.clone(), peer, self.limits),
        )
        .await;
        match result {
            Ok(Ok(())) => {}
            // A refusal has already closed the connection with its own code.
            Ok(Err(LogSessionError::Refused(_))) => {}
            Ok(Err(error)) => {
                connection.close(error.close_code().into(), error.close_reason());
            }
            Err(_) => {
                connection.close(CLOSE_INVALID.into(), b"log sync invalid or timed out");
            }
        }
        drop(slot);
        Ok(())
    }

    async fn shutdown(&self) {
        self.stop();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use p2panda_sync::protocols::LogSyncEvent;
    use std::sync::Mutex as StdMutex;

    /// A scripted Core seam. It exists only to inject a rejection: everything
    /// under test — the skipped-author set and the batching — is real.
    #[derive(Default)]
    pub(crate) struct ScriptedReceiver {
        rejected_author: StdMutex<Option<p2panda_core::VerifyingKey>>,
        seen: StdMutex<Vec<(p2panda_core::VerifyingKey, u32)>>,
        batches: StdMutex<Vec<usize>>,
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

        fn session_seed(
            &self,
            _peer: PublicIdentity,
            _workspace: WorkspaceId,
            _package: &str,
        ) -> Result<SessionSeed, Refusal> {
            Ok(SessionSeed::default())
        }

        fn range_sizes(
            &self,
            _peer: PublicIdentity,
            _workspace: WorkspaceId,
            _package: &str,
            _author: PublicIdentity,
            _after: Option<u32>,
            _until: Option<u32>,
            _limit: usize,
        ) -> Result<Vec<(u32, usize)>, ()> {
            Ok(Vec::new())
        }

        fn load_range_page(
            &self,
            _peer: PublicIdentity,
            _workspace: WorkspaceId,
            _package: &str,
            _author: PublicIdentity,
            _from: u32,
            _until: u32,
            _rows: usize,
        ) -> Result<Vec<ReplicationEntry>, RangeLoadError> {
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
            self.seen
                .lock()
                .unwrap()
                .push((author, operation.header.seq_num));
            if *self.rejected_author.lock().unwrap() == Some(author) {
                Ok(OperationOutcome::Rejected(RejectReason::Conflict))
            } else {
                Ok(OperationOutcome::Applied)
            }
        }

        fn receive_operations(
            &self,
            peer: PublicIdentity,
            workspace: WorkspaceId,
            package: &str,
            operations: Vec<Operation<()>>,
        ) -> Result<Vec<OperationOutcome>, FatalSyncError> {
            self.batches.lock().unwrap().push(operations.len());
            operations
                .into_iter()
                .map(|operation| self.receive_operation(peer, workspace, package, operation))
                .collect()
        }
    }

    pub(crate) fn received(operation: Operation<()>) -> LogSyncEvent<()> {
        LogSyncEvent::OperationReceived {
            operation: Box::new(operation),
            metrics: p2panda_sync::protocols::LogSyncMetrics::default(),
        }
    }

    pub(crate) fn test_peer(seed: u8) -> PublicIdentity {
        PublicIdentity::from_bytes(
            *iroh_base::SecretKey::from_bytes(&[seed; 32])
                .public()
                .as_bytes(),
        )
        .unwrap()
    }

    impl ScriptedReceiver {
        pub(crate) fn reject(&self, author: p2panda_core::VerifyingKey) {
            *self.rejected_author.lock().unwrap() = Some(author);
        }

        pub(crate) fn seen(&self) -> Vec<(p2panda_core::VerifyingKey, u32)> {
            self.seen.lock().unwrap().clone()
        }

        pub(crate) fn batches(&self) -> Vec<usize> {
            self.batches.lock().unwrap().clone()
        }
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
        assert_eq!(
            Refusal::from_close_code(u64::from(CLOSE_BUDGET_EXCEEDED)),
            None
        );
    }

    #[test]
    fn a_budget_refusal_closes_with_its_own_code() {
        assert_eq!(
            LogSessionError::BudgetExceeded.close_code(),
            CLOSE_BUDGET_EXCEEDED
        );
        assert_eq!(
            LogSessionError::Protocol("x".to_owned()).close_code(),
            CLOSE_INVALID
        );
    }

    #[test]
    fn application_acknowledgements_carry_a_truncation_byte() {
        let mut request = APPLICATION_ACK_REQUEST.to_vec();
        request.push(1);
        assert_eq!(
            parse_truncated_suffix(&request, APPLICATION_ACK_REQUEST),
            Ok(true)
        );
        let mut reply = APPLICATION_ACK.to_vec();
        reply.push(0);
        assert_eq!(parse_truncated_suffix(&reply, APPLICATION_ACK), Ok(false));
        assert!(parse_truncated_suffix(APPLICATION_ACK, APPLICATION_ACK).is_err());
        let mut invalid = APPLICATION_ACK.to_vec();
        invalid.push(2);
        assert!(parse_truncated_suffix(&invalid, APPLICATION_ACK).is_err());
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
        assert!(bytes.len() <= SessionLimits::CURRENT.hello_bytes);
        assert!(matches!(
            postcard::from_bytes::<LogSyncWire>(&bytes).unwrap(),
            LogSyncWire::Hello { package, .. } if package == "teamarness.test"
        ));
    }
}
