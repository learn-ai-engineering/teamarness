use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::{PublicIdentity, WorkspaceId};
use futures_util::{Sink, Stream, StreamExt};
use p2panda_core::Operation;
use p2panda_sync::protocols::{LogSyncEvent, LogSyncMessage};
use serde::{Deserialize, Serialize};

use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh_base::PublicKey;
use tokio::sync::Semaphore;
use tokio::time::timeout;

pub(crate) const MAX_SYNC_OPERATIONS: usize = 4_096;
pub(crate) const MAX_SYNC_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const SYNC_SESSION_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const MAX_SYNC_HANDLERS: usize = 8;
pub(crate) const LOG_SYNC_ALPN: &[u8] = b"teamarness/package-sync/1";

const VERSION: u8 = 1;
const BATCH_TAG: u8 = 1;
const ACK_TAG: u8 = 2;
const MAX_PACKAGE_IDENTITY: usize = 256;

/// A bounded package-sync batch.  The wire representation intentionally carries
/// opaque sealed records only; P2Panda and endpoint types do not cross this seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncBatch {
    pub(crate) workspace: [u8; 16],
    pub(crate) membership: Vec<Vec<u8>>,
    pub(crate) package: String,
    pub(crate) records: Vec<Vec<u8>>,
}

pub(crate) fn encode_batch(batch: &SyncBatch) -> Result<Vec<u8>, ()> {
    validate_batch(batch)?;
    let package = batch.package.as_bytes();
    let mut body = Vec::new();
    body.extend_from_slice(&[VERSION, BATCH_TAG]);
    body.extend_from_slice(&batch.workspace);
    body.extend_from_slice(&(u16::try_from(batch.membership.len()).map_err(|_| ())?).to_be_bytes());
    for operation in &batch.membership {
        body.extend_from_slice(&(u32::try_from(operation.len()).map_err(|_| ())?).to_be_bytes());
        body.extend_from_slice(operation);
    }
    body.extend_from_slice(&(package.len() as u16).to_be_bytes());
    body.extend_from_slice(package);
    body.extend_from_slice(&(batch.records.len() as u16).to_be_bytes());
    for record in &batch.records {
        body.extend_from_slice(&(u32::try_from(record.len()).map_err(|_| ())?).to_be_bytes());
        body.extend_from_slice(record);
    }
    frame(body)
}

pub(crate) fn decode_batch(frame_bytes: &[u8]) -> Result<SyncBatch, ()> {
    if frame_bytes.len() < 4 {
        return Err(());
    }
    let length = usize::try_from(u32::from_be_bytes(
        frame_bytes[..4].try_into().map_err(|_| ())?,
    ))
    .map_err(|_| ())?;
    if length > MAX_SYNC_BYTES || frame_bytes.len() != 4 + length {
        return Err(());
    }
    let body = &frame_bytes[4..];
    if body.len() < 2 + 16 + 2 + 2 || body[0] != VERSION || body[1] != BATCH_TAG {
        return Err(());
    }
    let mut at = 2;
    let workspace = take_array::<16>(body, &mut at)?;
    let membership_count = usize::from(take_array::<2>(body, &mut at).map(u16::from_be_bytes)?);
    if membership_count > MAX_SYNC_OPERATIONS {
        return Err(());
    }
    let mut membership = Vec::with_capacity(membership_count);
    for _ in 0..membership_count {
        let size =
            usize::try_from(u32::from_be_bytes(take_array::<4>(body, &mut at)?)).map_err(|_| ())?;
        membership.push(take(body, &mut at, size)?.to_vec());
    }
    let package_len = usize::from(take_array::<2>(body, &mut at).map(u16::from_be_bytes)?);
    if package_len > MAX_PACKAGE_IDENTITY {
        return Err(());
    }
    let package = String::from_utf8(take(body, &mut at, package_len)?.to_vec()).map_err(|_| ())?;
    let count = usize::from(take_array::<2>(body, &mut at).map(u16::from_be_bytes)?);
    if count > MAX_SYNC_OPERATIONS {
        return Err(());
    }
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        let size =
            usize::try_from(u32::from_be_bytes(take_array::<4>(body, &mut at)?)).map_err(|_| ())?;
        records.push(take(body, &mut at, size)?.to_vec());
    }
    if at != body.len() {
        return Err(());
    }
    let batch = SyncBatch {
        workspace,
        membership,
        package,
        records,
    };
    validate_batch(&batch)?;
    Ok(batch)
}

fn validate_batch(batch: &SyncBatch) -> Result<(), ()> {
    if batch.package.is_empty()
        || batch.package.len() > MAX_PACKAGE_IDENTITY
        || !batch.package.is_ascii()
    {
        return Err(());
    }
    if batch.records.len() > MAX_SYNC_OPERATIONS || batch.membership.len() > MAX_SYNC_OPERATIONS {
        return Err(());
    }
    let total = batch
        .records
        .iter()
        .chain(batch.membership.iter())
        .try_fold(0usize, |total, record| {
            total.checked_add(record.len()).ok_or(())
        })?;
    if total > MAX_SYNC_BYTES {
        return Err(());
    }
    Ok(())
}

fn frame(body: Vec<u8>) -> Result<Vec<u8>, ()> {
    if body.len() > MAX_SYNC_BYTES {
        return Err(());
    }
    let mut output = Vec::with_capacity(body.len() + 4);
    output.extend_from_slice(&(u32::try_from(body.len()).map_err(|_| ())?).to_be_bytes());
    output.extend_from_slice(&body);
    Ok(output)
}

fn take<'a>(bytes: &'a [u8], at: &mut usize, length: usize) -> Result<&'a [u8], ()> {
    let end = at.checked_add(length).ok_or(())?;
    let value = bytes.get(*at..end).ok_or(())?;
    *at = end;
    Ok(value)
}

fn take_array<const N: usize>(bytes: &[u8], at: &mut usize) -> Result<[u8; N], ()> {
    take(bytes, at, N)?.try_into().map_err(|_| ())
}

/// The initial synchronization protocol handler.  It provides bounded,
/// isolated sessions and an acknowledgement frame; record application is
/// deliberately performed by the Core synchronization service in later calls.
type ReplicationOperation = ([u8; 32], Operation<()>);

pub(crate) trait SyncReceiver: Send + Sync {
    fn receive(&self, peer: PublicIdentity, batch: SyncBatch) -> Result<usize, ()>;

    fn operations(
        &self,
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
        _operation: Operation<()>,
    ) -> Result<(), ()> {
        Err(())
    }
}

pub(crate) struct PackageSyncHandler {
    stopping: AtomicBool,
    inbound: Arc<Semaphore>,
    peers: Arc<Mutex<HashSet<PublicKey>>>,
    receiver: Arc<dyn SyncReceiver>,
}

impl std::fmt::Debug for PackageSyncHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PackageSyncHandler")
            .finish_non_exhaustive()
    }
}

impl PackageSyncHandler {
    pub(crate) fn new(receiver: Arc<dyn SyncReceiver>) -> Self {
        Self {
            stopping: AtomicBool::new(false),
            inbound: Arc::new(Semaphore::new(MAX_SYNC_HANDLERS)),
            peers: Arc::new(Mutex::new(HashSet::new())),
            receiver,
        }
    }

    pub(crate) fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
    }
}

impl ProtocolHandler for PackageSyncHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        if self.stopping.load(Ordering::Acquire) {
            connection.close(1_u8.into(), b"package sync unavailable");
            return Ok(());
        }
        let Some(permit) = self.inbound.clone().try_acquire_owned().ok() else {
            connection.close(2_u8.into(), b"package sync overloaded");
            return Ok(());
        };
        let peer = connection.remote_id();
        {
            let mut peers = self
                .peers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !peers.insert(peer) {
                connection.close(3_u8.into(), b"package sync peer busy");
                return Ok(());
            }
        }
        let result = timeout(SYNC_SESSION_TIMEOUT, async {
            let (mut send, mut receive) = connection.accept_bi().await.map_err(|_| ())?;
            let bytes = receive
                .read_to_end(MAX_SYNC_BYTES + 4)
                .await
                .map_err(|_| ())?;
            let batch = decode_batch(&bytes).map_err(|_| ())?;
            let receiver = Arc::clone(&self.receiver);
            let count = receiver
                .receive(
                    PublicIdentity::from_bytes(*peer.as_bytes()).map_err(|_| ())?,
                    batch,
                )
                .map_err(|_| ())?;
            let mut ack = vec![0, 0, 0, 4, VERSION, ACK_TAG];
            ack.extend_from_slice(&(u16::try_from(count).map_err(|_| ())?).to_be_bytes());
            send.write_all(&ack).await.map_err(|_| ())?;
            send.finish().map_err(|_| ())?;
            send.stopped().await.map_err(|_| ())?;
            Ok::<(), ()>(())
        })
        .await;
        drop(permit);
        self.peers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&peer);
        if !matches!(result, Ok(Ok(()))) {
            connection.close(4_u8.into(), b"package sync invalid or timed out");
        }
        Ok(())
    }

    async fn shutdown(&self) {
        self.stop();
    }
}

#[derive(Debug, Serialize, Deserialize)]
enum LogSyncWire {
    Hello {
        workspace: [u8; 16],
        package: String,
    },
    Message(LogSyncMessage<[u8; 32]>),
}

struct LogSyncIo {
    outbound: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    inbound: tokio::sync::mpsc::UnboundedReceiver<Result<LogSyncWire, String>>,
}

impl LogSyncIo {
    fn new(mut send: iroh::endpoint::SendStream, mut receive: iroh::endpoint::RecvStream) -> Self {
        let (outbound, mut outbound_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let (inbound_tx, inbound) =
            tokio::sync::mpsc::unbounded_channel::<Result<LogSyncWire, String>>();
        tokio::spawn(async move {
            while let Some(bytes) = outbound_rx.recv().await {
                let Ok(length) = u32::try_from(bytes.len()) else {
                    break;
                };
                if send.write_all(&length.to_be_bytes()).await.is_err()
                    || send.write_all(&bytes).await.is_err()
                {
                    break;
                }
            }
            let _ = send.finish();
        });
        tokio::spawn(async move {
            loop {
                let mut length = [0; 4];
                if receive.read_exact(&mut length).await.is_err() {
                    break;
                }
                let length = u32::from_be_bytes(length) as usize;
                if length > MAX_SYNC_BYTES {
                    let _ = inbound_tx.send(Err("log sync frame exceeds bound".to_owned()));
                    break;
                }
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
        Self { outbound, inbound }
    }

    pub(crate) fn send_wire(&mut self, wire: LogSyncWire) -> Result<(), String> {
        let bytes = postcard::to_stdvec(&wire).map_err(|error| error.to_string())?;
        if bytes.len() > MAX_SYNC_BYTES {
            return Err("log sync frame exceeds bound".to_owned());
        }
        self.outbound
            .send(bytes)
            .map_err(|_| "log sync writer closed".to_owned())
    }

    async fn hello(&mut self) -> Result<(WorkspaceId, String), String> {
        match self.inbound.recv().await {
            Some(Ok(LogSyncWire::Hello { workspace, package })) => {
                if package.is_empty() || package.len() > MAX_PACKAGE_IDENTITY {
                    return Err("invalid package identity".to_owned());
                }
                Ok((WorkspaceId::from_bytes(workspace), package))
            }
            _ => Err("missing log sync hello".to_owned()),
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
                Ok(LogSyncWire::Hello { .. }) => Err("unexpected log sync hello".to_owned()),
                Err(error) => Err(error),
            })
        })
    }
}

pub(crate) async fn outbound_log_session(
    endpoint: &iroh::Endpoint,
    address: iroh_base::EndpointAddr,
    receiver: Arc<dyn SyncReceiver>,
    workspace: WorkspaceId,
    package: String,
    peer: PublicIdentity,
) -> Result<usize, String> {
    let connection = endpoint
        .connect(address, LOG_SYNC_ALPN)
        .await
        .map_err(|error| error.to_string())?;
    let (send, receive) = connection
        .open_bi()
        .await
        .map_err(|error| error.to_string())?;
    let mut io = LogSyncIo::new(send, receive);
    io.send_wire(LogSyncWire::Hello {
        workspace: *workspace.as_bytes(),
        package: package.clone(),
    })?;
    let operations = receiver
        .operations(workspace, &package)
        .map_err(|_| "failed to load local operations".to_owned())?;
    let store = crate::synchronization::p2panda_adapter::TeamarnessLogStore::from_operations(
        operations.clone(),
    );
    let mut logs = std::collections::BTreeMap::new();
    for (log_id, operation) in operations {
        logs.entry(operation.header.verifying_key)
            .or_insert_with(Vec::new)
            .push(log_id);
    }
    for ids in logs.values_mut() {
        ids.sort();
        ids.dedup();
    }
    let (events, mut event_rx) = tokio::sync::broadcast::channel::<LogSyncEvent<()>>(64);
    let protocol = p2panda_sync::protocols::LogSync::new(store, logs, events);
    let (mut sink, mut stream) = io.split();
    let mut running = Box::pin(crate::synchronization::p2panda_adapter::run_finite(
        protocol,
        &mut sink,
        &mut stream,
    ));
    let mut accepted = 0;
    loop {
        tokio::select! {
            result = &mut running => {
                result.map_err(|error| error.to_string())?;
                while let Ok(LogSyncEvent::OperationReceived { operation, .. }) = event_rx.try_recv() {
                    if receiver.receive_operation(peer, workspace, &package, *operation).is_ok() {
                        accepted += 1;
                    }
                }
                return Ok(accepted);
            }
            event = event_rx.recv() => {
                if let Ok(LogSyncEvent::OperationReceived { operation, .. }) = event
                    && receiver.receive_operation(peer, workspace, &package, *operation).is_ok()
                {
                    accepted += 1;
                }
            }
        }
    }
}

pub(crate) struct LogSyncHandler {
    stopping: AtomicBool,
    receiver: Arc<dyn SyncReceiver>,
}

impl std::fmt::Debug for LogSyncHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LogSyncHandler")
            .finish_non_exhaustive()
    }
}

impl LogSyncHandler {
    pub(crate) fn new(receiver: Arc<dyn SyncReceiver>) -> Self {
        Self {
            stopping: AtomicBool::new(false),
            receiver,
        }
    }

    pub(crate) fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
    }
}

async fn run_log_session(
    connection: Connection,
    receiver: Arc<dyn SyncReceiver>,
    peer: PublicIdentity,
) -> Result<(), String> {
    let (send, receive) = connection
        .accept_bi()
        .await
        .map_err(|error| error.to_string())?;
    let mut io = LogSyncIo::new(send, receive);
    let (workspace, package) = io.hello().await?;
    let operations = receiver
        .operations(workspace, &package)
        .map_err(|_| "failed to load local operations".to_owned())?;
    let store = crate::synchronization::p2panda_adapter::TeamarnessLogStore::from_operations(
        operations.clone(),
    );
    let mut logs = std::collections::BTreeMap::new();
    for (log_id, operation) in operations {
        logs.entry(operation.header.verifying_key)
            .or_insert_with(Vec::new)
            .push(log_id);
    }
    for ids in logs.values_mut() {
        ids.sort();
        ids.dedup();
    }
    let (events, mut event_rx) = tokio::sync::broadcast::channel::<LogSyncEvent<()>>(64);
    let protocol = p2panda_sync::protocols::LogSync::new(store, logs, events);
    let (mut sink, mut stream) = io.split();
    let mut running = Box::pin(crate::synchronization::p2panda_adapter::run_finite(
        protocol,
        &mut sink,
        &mut stream,
    ));
    loop {
        tokio::select! {
            result = &mut running => {
                result.map_err(|error| error.to_string())?;
                while let Ok(LogSyncEvent::OperationReceived { operation, .. }) = event_rx.try_recv() {
                    let _ = receiver.receive_operation(peer, workspace, &package, *operation);
                }
                return Ok(());
            }
            event = event_rx.recv() => {
                if let Ok(LogSyncEvent::OperationReceived { operation, .. }) = event {
                    let _ = receiver.receive_operation(peer, workspace, &package, *operation);
                }
            }
        }
    }
}

impl ProtocolHandler for LogSyncHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        if self.stopping.load(Ordering::Acquire) {
            connection.close(1_u8.into(), b"package sync unavailable");
            return Ok(());
        }
        let peer = PublicIdentity::from_bytes(*connection.remote_id().as_bytes())
            .map_err(|_| AcceptError::from(std::io::Error::other("invalid peer")))?;
        let result = timeout(
            SYNC_SESSION_TIMEOUT,
            run_log_session(connection.clone(), Arc::clone(&self.receiver), peer),
        )
        .await;
        if !matches!(result, Ok(Ok(()))) {
            connection.close(4_u8.into(), b"log sync invalid or timed out");
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

    fn batch(count: usize) -> SyncBatch {
        SyncBatch {
            workspace: [7; 16],
            membership: vec![],
            package: "teamarness.conversations".to_owned(),
            records: (0..count).map(|n| vec![n as u8]).collect(),
        }
    }

    #[test]
    fn package_sync_wire_is_round_trip_and_bounded() {
        let encoded = encode_batch(&batch(2)).unwrap();
        assert_eq!(decode_batch(&encoded).unwrap(), batch(2));
        assert!(encode_batch(&batch(MAX_SYNC_OPERATIONS + 1)).is_err());
        assert!(decode_batch(&[0, 0, 0, 1, VERSION, BATCH_TAG]).is_err());
    }
}
