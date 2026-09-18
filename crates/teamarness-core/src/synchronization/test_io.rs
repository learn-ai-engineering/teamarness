//! In-memory doubles for synchronization tests.
//!
//! These exist to inject a fault or to stand in for a component whose real
//! version needs a database: the protocol, the frame I/O, and the session loops
//! under test are always the real ones.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, PoisonError};

use p2panda_core::{Body, Hash, Operation, VerifyingKey};
use p2panda_store::logs::LogStore;

use super::network_handler::{
    FatalSyncError, OperationOutcome, ProtectionBundle, RangeLoadError, Refusal, ReplicationEntry,
    SessionGrant, SessionSeed, SyncReceiver,
};
use crate::{PublicIdentity, WorkspaceId};

/// A `LogStore` over operations held in memory.
#[derive(Debug, Clone, Default)]
pub(crate) struct MemoryLogStore {
    entries: HashMap<(VerifyingKey, [u8; 32]), BTreeMap<u32, Operation<()>>>,
}

#[derive(Debug, Clone)]
pub(crate) struct MemoryLogStoreError(pub(crate) String);

impl std::fmt::Display for MemoryLogStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for MemoryLogStoreError {}

impl MemoryLogStore {
    pub(crate) fn from_operations(
        operations: impl IntoIterator<Item = ([u8; 32], Operation<()>)>,
    ) -> Self {
        let mut entries: HashMap<_, BTreeMap<u32, Operation<()>>> = HashMap::new();
        for (log_id, operation) in operations {
            entries
                .entry((operation.header.verifying_key, log_id))
                .or_default()
                .insert(operation.header.seq_num, operation);
        }
        Self { entries }
    }

    /// Builds correctly backlinked logs of `count` operations per author seed,
    /// all under the log id tests use.
    pub(crate) fn with_logs(logs: &[(u8, u32)]) -> Self {
        let operations = logs
            .iter()
            .flat_map(|(seed, count)| {
                signed_log(*seed, *count)
                    .into_iter()
                    .map(|operation| ([4; 32], operation))
            })
            .collect::<Vec<_>>();
        Self::from_operations(operations)
    }

    fn log(
        &self,
        author: &VerifyingKey,
        log_id: &[u8; 32],
    ) -> Option<&BTreeMap<u32, Operation<()>>> {
        self.entries.get(&(*author, *log_id))
    }

    pub(crate) fn heights(&self) -> BTreeMap<PublicIdentity, u32> {
        let mut heights = BTreeMap::new();
        for ((author, _), log) in &self.entries {
            let Ok(identity) = PublicIdentity::from_bytes(*author.as_bytes()) else {
                continue;
            };
            if let Some(height) = log.keys().next_back().copied() {
                let entry = heights.entry(identity).or_insert(height);
                *entry = (*entry).max(height);
            }
        }
        heights
    }

    /// The operations of one author in `after+1..=until`, in sequence order.
    pub(crate) fn range(
        &self,
        author: PublicIdentity,
        after: Option<u32>,
        until: Option<u32>,
    ) -> Vec<Operation<()>> {
        let Ok(key) = VerifyingKey::from_bytes(author.as_bytes()) else {
            return Vec::new();
        };
        self.log(&key, &[4; 32]).map_or_else(Vec::new, |log| {
            log.iter()
                .filter(|(sequence, _)| {
                    after.is_none_or(|value| **sequence > value)
                        && until.is_none_or(|value| **sequence <= value)
                })
                .map(|(_, operation)| operation.clone())
                .collect()
        })
    }
}

impl LogStore<Operation<()>, VerifyingKey, [u8; 32], u32, Hash> for MemoryLogStore {
    type Error = MemoryLogStoreError;

    async fn get_latest_entry(
        &self,
        author: &VerifyingKey,
        log_id: &[u8; 32],
    ) -> Result<Option<Operation<()>>, Self::Error> {
        Ok(self
            .log(author, log_id)
            .and_then(|log| log.values().next_back().cloned()))
    }

    async fn get_latest_entry_tx(
        &self,
        author: &VerifyingKey,
        log_id: &[u8; 32],
    ) -> Result<Option<Operation<()>>, Self::Error> {
        self.get_latest_entry(author, log_id).await
    }

    async fn get_log_heights(
        &self,
        author: &VerifyingKey,
        logs: &[[u8; 32]],
    ) -> Result<Option<BTreeMap<[u8; 32], u32>>, Self::Error> {
        let heights = logs
            .iter()
            .filter_map(|log_id| {
                self.log(author, log_id)
                    .and_then(|log| log.keys().next_back().copied())
                    .map(|height| (*log_id, height))
            })
            .collect::<BTreeMap<_, _>>();
        if heights.is_empty() {
            Ok(None)
        } else {
            Ok(Some(heights))
        }
    }

    async fn get_log_size(
        &self,
        author: &VerifyingKey,
        log_id: &[u8; 32],
        after: Option<u32>,
        until: Option<u32>,
    ) -> Result<Option<(u32, u32)>, Self::Error> {
        let Some(log) = self.log(author, log_id) else {
            return Ok(None);
        };
        let mut count = 0_u32;
        let mut bytes = 0_u32;
        for (_, operation) in log.iter().filter(|(sequence, _)| {
            after.is_none_or(|value| **sequence > value)
                && until.is_none_or(|value| **sequence <= value)
        }) {
            count = count.saturating_add(1);
            bytes =
                bytes.saturating_add(u32::try_from(operation_size(operation)).unwrap_or(u32::MAX));
        }
        Ok(Some((count, bytes)))
    }

    async fn get_log_entries(
        &self,
        author: &VerifyingKey,
        log_id: &[u8; 32],
        after: Option<u32>,
        until: Option<u32>,
    ) -> Result<Option<Vec<(Operation<()>, Vec<u8>)>>, Self::Error> {
        let Some(log) = self.log(author, log_id) else {
            return Ok(None);
        };
        Ok(Some(
            log.iter()
                .filter(|(sequence, _)| {
                    after.is_none_or(|value| **sequence > value)
                        && until.is_none_or(|value| **sequence <= value)
                })
                .map(|(_, operation)| (operation.clone(), operation.header.encode()))
                .collect(),
        ))
    }

    async fn prune_entries(
        &self,
        _author: &VerifyingKey,
        _log_id: &[u8; 32],
        _until: &u32,
    ) -> Result<u64, Self::Error> {
        Ok(0)
    }
}

pub(crate) fn operation_size(operation: &Operation<()>) -> usize {
    operation.header.encode().len()
        + operation
            .body
            .as_ref()
            .map_or(0, |body| body.to_bytes().len())
}

/// A correctly backlinked log of `count` operations for one author seed.
pub(crate) fn signed_log(seed: u8, count: u32) -> Vec<Operation<()>> {
    let key = p2panda_core::SigningKey::from_bytes(&[seed; 32]);
    let mut backlink = None;
    let mut log = Vec::with_capacity(count as usize);
    for sequence in 0..count {
        let payload = sequence.to_be_bytes().to_vec();
        let header = p2panda_core::Header::<()>::builder()
            .body(payload.as_slice())
            .seq_num(sequence)
            .backlink(backlink)
            .build(&key, ());
        backlink = Some(header.hash());
        log.push(Operation::from_parts(
            header,
            Some(Body::from_bytes(&payload)),
        ));
    }
    log
}

/// A Core seam backed by [`MemoryLogStore`] that records how it was read.
///
/// It stands in for storage so the lazy view's paging and budget behaviour can
/// be asserted without a database; the view under test is the real one.
pub(crate) struct RecordingReceiver {
    store: MemoryLogStore,
    pages: Mutex<Vec<usize>>,
    rows_loaded: Mutex<usize>,
}

impl RecordingReceiver {
    pub(crate) fn new(store: MemoryLogStore) -> Self {
        Self {
            store,
            pages: Mutex::new(Vec::new()),
            rows_loaded: Mutex::new(0),
        }
    }

    pub(crate) fn seed(&self) -> SessionSeed {
        let heights = self.store.heights();
        SessionSeed {
            authors: heights.keys().copied().collect(),
            heights,
        }
    }

    /// The size of each page the view asked for, in order.
    pub(crate) fn pages(&self) -> Vec<usize> {
        self.pages
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn rows_loaded(&self) -> usize {
        *self
            .rows_loaded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl SyncReceiver for RecordingReceiver {
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
        Ok(self.seed())
    }

    fn range_sizes(
        &self,
        _peer: PublicIdentity,
        _workspace: WorkspaceId,
        _package: &str,
        author: PublicIdentity,
        after: Option<u32>,
        until: Option<u32>,
        limit: usize,
    ) -> Result<Vec<(u32, usize)>, ()> {
        Ok(self
            .store
            .range(author, after, until)
            .into_iter()
            .take(limit)
            .map(|operation| (operation.header.seq_num, operation_size(&operation)))
            .collect())
    }

    fn load_range_page(
        &self,
        _peer: PublicIdentity,
        _workspace: WorkspaceId,
        _package: &str,
        author: PublicIdentity,
        from: u32,
        until: u32,
        rows: usize,
    ) -> Result<Vec<ReplicationEntry>, RangeLoadError> {
        let page = self
            .store
            .range(author, from.checked_sub(1), Some(until))
            .into_iter()
            .take(rows)
            .map(|operation| {
                let header = operation.header.encode();
                (operation, header)
            })
            .collect::<Vec<_>>();
        self.pages
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(page.len());
        *self
            .rows_loaded
            .lock()
            .unwrap_or_else(PoisonError::into_inner) += page.len();
        Ok(page)
    }

    fn receive_operation(
        &self,
        _peer: PublicIdentity,
        _workspace: WorkspaceId,
        _package: &str,
        _operation: Operation<()>,
    ) -> Result<OperationOutcome, FatalSyncError> {
        Ok(OperationOutcome::Applied)
    }
}

/// A writer that stops accepting bytes after `remaining`, without waking.
///
/// It stands in for a peer that has stopped reading, so a test can observe
/// transport backpressure rather than a growing buffer.
pub(crate) struct StallAfter<W> {
    inner: W,
    remaining: usize,
}

impl<W> StallAfter<W> {
    pub(crate) fn new(inner: W, remaining: usize) -> Self {
        Self { inner, remaining }
    }
}

impl<W: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for StallAfter<W> {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        buffer: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        if this.remaining == 0 {
            // No waker is stored: the writer never makes progress again.
            return std::task::Poll::Pending;
        }
        let take = buffer.len().min(this.remaining);
        match std::pin::Pin::new(&mut this.inner).poll_write(context, &buffer[..take]) {
            std::task::Poll::Ready(Ok(written)) => {
                this.remaining -= written;
                std::task::Poll::Ready(Ok(written))
            }
            other => other,
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_flush(context)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_shutdown(context)
    }
}
