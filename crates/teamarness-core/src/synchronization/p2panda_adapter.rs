//! The private P2Panda Sync adapter.
//!
//! This module is feature-gated so the semantic Core seam remains independent
//! from the dependency's concrete operation types. Callers must provide a
//! finite LogSync store view; the wrapper adds the hard session deadline.

use std::fmt::Debug;

use futures_util::{Sink, Stream};
use p2panda_core::{Extensions, Hash, LogId, Operation, SeqNum, VerifyingKey};
use p2panda_store::logs::LogStore;
use p2panda_sync::protocols::{LogSync, LogSyncError, LogSyncEvent};
use p2panda_sync::traits::Protocol;

/// Runs one LogSync session to completion within a fixed deadline.
pub(crate) async fn run_finite<L, E, S, Evt>(
    protocol: LogSync<L, E, S, Evt>,
    sink: &mut (impl Sink<p2panda_sync::protocols::LogSyncMessage<L>, Error = impl Debug> + Unpin),
    stream: &mut (
             impl Stream<Item = Result<p2panda_sync::protocols::LogSyncMessage<L>, impl Debug>> + Unpin
         ),
    deadline: std::time::Duration,
) -> Result<(), LogSyncError>
where
    L: LogId + Debug + Send + 'static,
    E: Extensions + Send + 'static,
    S: LogStore<Operation<E>, VerifyingKey, L, SeqNum, Hash> + Clone + Send + 'static,
    Evt: Debug + From<LogSyncEvent<E>> + Send + 'static,
{
    tokio::time::timeout(deadline, protocol.run(sink, stream))
        .await
        .map_err(|_| {
            LogSyncError::MessageStream("finite session deadline exceeded".to_owned())
        })??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        run_finite(
            protocol,
            &mut sink,
            &mut stream,
            std::time::Duration::from_secs(5),
        )
        .await
        .unwrap();
        sink.close().await.unwrap();
    }
}
