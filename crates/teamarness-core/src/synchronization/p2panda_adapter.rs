//! The private P2Panda Sync adapter.
//!
//! This module is feature-gated so the semantic Core seam remains independent
//! from the dependency's concrete operation types. Callers must provide a
//! finite LogSync store view; the wrapper adds the hard session deadline.

use std::collections::{BTreeMap, HashMap};
use std::fmt::{Debug, Display};

use futures_util::{Sink, Stream};
use p2panda_core::{Body, Extensions, Hash, LogId, Operation, SeqNum, VerifyingKey};
use p2panda_store::logs::LogStore;
use p2panda_sync::protocols::{LogSync, LogSyncError, LogSyncEvent};
use p2panda_sync::traits::Protocol;

use super::log_sync::{MAX_LOG_BYTES, MAX_LOG_OPERATIONS, MAX_LOG_SESSION};

#[derive(Debug, Clone)]
pub(crate) struct TeamarnessLogStore {
    entries: HashMap<(VerifyingKey, [u8; 32]), BTreeMap<u32, Operation<()>>>,
}

#[derive(Debug, Clone)]
pub(crate) struct TeamarnessLogStoreError(String);

impl Display for TeamarnessLogStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for TeamarnessLogStoreError {}

impl TeamarnessLogStore {
    pub(crate) fn from_connection(
        connection: &rusqlite::Connection,
        scope_id: [u8; 32],
    ) -> Result<Self, TeamarnessLogStoreError> {
        let mut statement = connection
            .prepare(
                "SELECT operation_bytes FROM p2panda_operations
                 WHERE scope_id=?1 AND status='accepted'
                 ORDER BY author_public_identity, author_sequence",
            )
            .map_err(|error| TeamarnessLogStoreError(error.to_string()))?;
        let operations = statement
            .query_map([scope_id.as_slice()], |row| row.get::<_, Vec<u8>>(0))
            .map_err(|error| TeamarnessLogStoreError(error.to_string()))?
            .map(|row| {
                let bytes = row.map_err(|error| TeamarnessLogStoreError(error.to_string()))?;
                let (header, payload) = super::operation_store::decode_operation(&bytes)
                    .map_err(|error| TeamarnessLogStoreError(error.to_string()))?;
                let operation = Operation::try_from((header, Some(Body::from_bytes(&payload))))
                    .map_err(|error| TeamarnessLogStoreError(error.to_string()))?;
                Ok((operation, bytes))
            })
            .collect::<Result<Vec<_>, TeamarnessLogStoreError>>()?;
        let log_id = super::operation_store::log_id(scope_id);
        Ok(Self::from_operations(
            operations
                .into_iter()
                .map(|(operation, _)| (log_id, operation)),
        ))
    }

    pub(crate) fn from_operations(
        operations: impl IntoIterator<Item = ([u8; 32], Operation<()>)>,
    ) -> Self {
        let mut entries = HashMap::new();
        for (log_id, operation) in operations {
            entries
                .entry((operation.header.verifying_key, log_id))
                .or_insert_with(BTreeMap::new)
                .insert(operation.header.seq_num, operation);
        }
        Self { entries }
    }

    pub(crate) fn operations(&self) -> Vec<Operation<()>> {
        self.entries
            .values()
            .flat_map(|log| log.values().cloned())
            .collect()
    }

    fn log(
        &self,
        author: &VerifyingKey,
        log_id: &[u8; 32],
    ) -> Option<&BTreeMap<u32, Operation<()>>> {
        self.entries.get(&(*author, *log_id))
    }
}

impl p2panda_store::logs::LogStore<Operation<()>, VerifyingKey, [u8; 32], u32, Hash>
    for TeamarnessLogStore
{
    type Error = TeamarnessLogStoreError;

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
        let entries = log.iter().filter(|(sequence, _)| {
            after.is_none_or(|value| **sequence > value)
                && until.is_none_or(|value| **sequence <= value)
        });
        let mut count = 0_u32;
        let mut bytes = 0_u32;
        for (_, operation) in entries {
            let size = operation.header.encode().len()
                + operation
                    .body
                    .as_ref()
                    .map_or(0, |body| body.to_bytes().len());
            if count as usize >= MAX_LOG_OPERATIONS
                || bytes as usize > MAX_LOG_BYTES.saturating_sub(size)
            {
                break;
            }
            count = count.saturating_add(1);
            bytes = bytes.saturating_add(u32::try_from(size).unwrap_or(u32::MAX));
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
        let mut bytes = 0_usize;
        let mut entries = Vec::new();
        for (_, operation) in log.iter().filter(|(sequence, _)| {
            after.is_none_or(|value| **sequence > value)
                && until.is_none_or(|value| **sequence <= value)
        }) {
            if entries.len() >= MAX_LOG_OPERATIONS {
                break;
            }
            let header = operation.header.encode();
            let size = header.len()
                + operation
                    .body
                    .as_ref()
                    .map_or(0, |body| body.to_bytes().len());
            if bytes > MAX_LOG_BYTES.saturating_sub(size) {
                break;
            }
            bytes += size;
            entries.push((operation.clone(), header));
        }
        Ok(Some(entries))
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

pub(crate) async fn run_finite<L, E, S, Evt>(
    protocol: LogSync<L, E, S, Evt>,
    sink: &mut (impl Sink<p2panda_sync::protocols::LogSyncMessage<L>, Error = impl Debug> + Unpin),
    stream: &mut (
             impl Stream<Item = Result<p2panda_sync::protocols::LogSyncMessage<L>, impl Debug>> + Unpin
         ),
) -> Result<(), LogSyncError>
where
    L: LogId + Debug + Send + 'static,
    E: Extensions + Send + 'static,
    S: LogStore<Operation<E>, VerifyingKey, L, SeqNum, Hash> + Clone + Send + 'static,
    Evt: Debug + From<LogSyncEvent<E>> + Send + 'static,
{
    tokio::time::timeout(MAX_LOG_SESSION, protocol.run(sink, stream))
        .await
        .map_err(|_| {
            LogSyncError::MessageStream("finite session deadline exceeded".to_owned())
        })??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn teamarness_store_caps_each_log_session_range() {
        let key = p2panda_core::SigningKey::from_bytes(&[8; 32]);
        let operations = (0..=MAX_LOG_OPERATIONS as u32)
            .map(|sequence| {
                let operation = Operation::from_parts(
                    p2panda_core::Header::<()>::builder()
                        .body(b"x")
                        .seq_num(sequence)
                        .build(&key, ()),
                    Some(p2panda_core::Body::from_bytes(b"x")),
                );
                ([4; 32], operation)
            })
            .collect::<Vec<_>>();
        let store = TeamarnessLogStore::from_operations(operations);
        let author = key.verifying_key();
        let size = store
            .get_log_size(&author, &[4; 32], None, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(size.0 as usize, MAX_LOG_OPERATIONS);
        let entries = store
            .get_log_entries(&author, &[4; 32], None, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(entries.len(), MAX_LOG_OPERATIONS);
    }

    #[tokio::test]
    async fn teamarness_store_exposes_durable_author_ranges_to_log_sync() {
        let operation = Operation::from_parts(
            p2panda_core::Header::<()>::builder()
                .body(b"sealed")
                .seq_num(0)
                .build(&p2panda_core::SigningKey::from_bytes(&[9; 32]), ()),
            Some(p2panda_core::Body::from_bytes(b"sealed")),
        );
        let store = TeamarnessLogStore::from_operations(vec![([4; 32], operation)]);
        let author = p2panda_core::SigningKey::from_bytes(&[9; 32]).verifying_key();
        let heights = store.get_log_heights(&author, &[[4; 32]]).await.unwrap();
        assert_eq!(heights.unwrap().get(&[4; 32]), Some(&0));
    }

    use futures_util::{SinkExt, sink, stream};
    use p2panda_core::Operation;
    use p2panda_core::logs::LogHeights;
    use p2panda_store::logs::LogStore;
    use p2panda_sync::protocols::{LogSyncEvent, LogSyncMessage};
    use std::collections::BTreeMap;
    use std::fmt;

    #[derive(Debug, Clone, Copy)]
    struct EmptyError;

    impl fmt::Display for EmptyError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("empty test store")
        }
    }
    impl std::error::Error for EmptyError {}

    #[derive(Clone)]
    struct EmptyStore;

    impl LogStore<Operation<()>, p2panda_core::VerifyingKey, String, u32, Hash> for EmptyStore {
        type Error = EmptyError;

        async fn get_latest_entry(
            &self,
            _author: &p2panda_core::VerifyingKey,
            _log_id: &String,
        ) -> Result<Option<Operation<()>>, Self::Error> {
            Ok(None)
        }

        async fn get_latest_entry_tx(
            &self,
            _author: &p2panda_core::VerifyingKey,
            _log_id: &String,
        ) -> Result<Option<Operation<()>>, Self::Error> {
            Ok(None)
        }

        async fn get_log_heights(
            &self,
            _author: &p2panda_core::VerifyingKey,
            _logs: &[String],
        ) -> Result<Option<BTreeMap<String, u32>>, Self::Error> {
            Ok(Some(BTreeMap::new()))
        }

        async fn get_log_size(
            &self,
            _author: &p2panda_core::VerifyingKey,
            _log_id: &String,
            _after: Option<u32>,
            _until: Option<u32>,
        ) -> Result<Option<(u32, u32)>, Self::Error> {
            Ok(None)
        }

        async fn get_log_entries(
            &self,
            _author: &p2panda_core::VerifyingKey,
            _log_id: &String,
            _after: Option<u32>,
            _until: Option<u32>,
        ) -> Result<Option<Vec<(Operation<()>, Vec<u8>)>>, Self::Error> {
            Ok(None)
        }

        async fn prune_entries(
            &self,
            _author: &p2panda_core::VerifyingKey,
            _log_id: &String,
            _until: &u32,
        ) -> Result<u64, Self::Error> {
            Ok(0)
        }
    }

    #[tokio::test]
    async fn adapter_executes_log_sync_over_caller_owned_streams() {
        let (events, _events_rx) = tokio::sync::broadcast::channel::<LogSyncEvent<()>>(8);
        let protocol = LogSync::new(
            EmptyStore,
            BTreeMap::<p2panda_core::VerifyingKey, Vec<String>>::new(),
            events,
        );
        let mut sink = sink::drain::<LogSyncMessage<String>>();
        let messages: Vec<Result<LogSyncMessage<String>, ()>> = vec![
            Ok(LogSyncMessage::Have(LogHeights::new())),
            Ok(LogSyncMessage::Done),
            Ok(LogSyncMessage::Done),
        ];
        let mut stream = stream::iter(messages);
        run_finite(protocol, &mut sink, &mut stream).await.unwrap();
        sink.close().await.unwrap();
    }
}
