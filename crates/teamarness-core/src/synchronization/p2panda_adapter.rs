//! The private P2Panda Sync adapter.
//!
//! LogSync decodes and authenticates headers inside its protocol future. That
//! future owns its transport and runs on a blocking thread, never a Tokio
//! executor worker. Cancellation retains admission until synchronous work ends.

use std::fmt::Debug;

use futures_util::{Sink, Stream};
use p2panda_core::{Extensions, Hash, LogId, Operation, SeqNum, VerifyingKey};
use p2panda_store::logs::LogStore;
use p2panda_sync::protocols::{LogSync, LogSyncError, LogSyncEvent};
use p2panda_sync::traits::Protocol;

/// Runs one LogSync session to completion within a fixed deadline.
pub(crate) async fn run_finite<L, E, S, Evt, W, R>(
    protocol: LogSync<L, E, S, Evt>,
    mut sink: W,
    mut stream: R,
    deadline: std::time::Duration,
    work: std::sync::Arc<super::peer_sessions::SessionWork>,
) -> Result<(W, R), LogSyncError>
where
    L: LogId + Debug + Send + 'static,
    E: Extensions + Send + 'static,
    S: LogStore<Operation<E>, VerifyingKey, L, SeqNum, Hash> + Clone + Send + 'static,
    Evt: Debug + From<LogSyncEvent<E>> + Send + 'static,
    W: Sink<p2panda_sync::protocols::LogSyncMessage<L>> + Unpin + Send + 'static,
    W::Error: Debug,
    R: Stream<Item = Result<p2panda_sync::protocols::LogSyncMessage<L>, super::frames::FrameError>>
        + Unpin
        + Send
        + 'static,
{
    let expires = tokio::time::Instant::now() + deadline;
    let handle = tokio::runtime::Handle::current();
    let (cancel, mut cancelled) = tokio::sync::oneshot::channel();
    // Dropping the caller cancels the worker, not its admission accounting.
    // Synchronous authentication can finish its current poll before stopping.
    let _cancel = CancelOnDrop(Some(cancel));
    tokio::task::spawn_blocking(move || {
        let _work = work;
        let result = handle.block_on(async {
            tokio::select! {
                biased;
                _ = &mut cancelled => Err(LogSyncError::MessageStream("session cancelled".to_owned())),
                result = tokio::time::timeout_at(expires, protocol.run(&mut sink, &mut stream)) => {
                    result.map_err(|_| LogSyncError::MessageStream(
                        "finite session deadline exceeded".to_owned(),
                    ))??;
                    Ok(())
                }
            }
        });
        result?;
        // QUIC stream lifetime includes the application acknowledgement after
        // LogSync. Return transport ownership instead of resetting it early.
        Ok((sink, stream))
    })
    .await
    .map_err(|error| LogSyncError::MessageStream(format!("protocol worker failed: {error}")))?
}

struct CancelOnDrop(Option<tokio::sync::oneshot::Sender<()>>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(cancel) = self.0.take() {
            let _ = cancel.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use futures_util::{sink, stream};
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
    struct EmptyStore {
        executor: Option<std::thread::ThreadId>,
        gate: Option<std::sync::Arc<StoreGate>>,
        fail: bool,
    }

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
            if let Some(executor) = self.executor {
                assert_ne!(
                    std::thread::current().id(),
                    executor,
                    "LogSync must not poll synchronous protocol work on the async executor"
                );
            }
            if let Some(gate) = &self.gate {
                if let Some(started) = gate.started.lock().unwrap().take() {
                    let _ = started.send(());
                }
                // The test owns the release; timeout is only a failure bound.
                gate.release
                    .lock()
                    .unwrap()
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
            }
            if self.fail {
                return Err(EmptyError);
            }
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

    struct StoreGate {
        started: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    }

    struct Capacity {
        permit: Option<tokio::sync::OwnedSemaphorePermit>,
        ended: Option<tokio::sync::oneshot::Sender<()>>,
    }
    impl Drop for Capacity {
        fn drop(&mut self) {
            drop(self.permit.take());
            if let Some(ended) = self.ended.take() {
                let _ = ended.send(());
            }
        }
    }

    type TestProtocol = LogSync<String, (), EmptyStore, LogSyncEvent<()>>;

    fn protocol(
        store: EmptyStore,
    ) -> (
        TestProtocol,
        tokio::sync::broadcast::Receiver<LogSyncEvent<()>>,
    ) {
        let (events, rx) = tokio::sync::broadcast::channel(8);
        let author = p2panda_core::SigningKey::from_bytes(&[1; 32]).verifying_key();
        (
            LogSync::new(
                store,
                BTreeMap::from([(author, vec!["test".to_owned()])]),
                events,
            ),
            rx,
        )
    }

    /// Verifies: TEAM.SYS.TRANS-R07
    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_protocol_keeps_admission_until_synchronous_work_ends() {
        for caller_timeout in [false, true] {
            let sessions = super::super::PeerSessions::new();
            let capacity = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
            let (ended, end) = tokio::sync::oneshot::channel();
            let work = sessions
                .begin_work(Capacity {
                    permit: Some(capacity.clone().try_acquire_owned().unwrap()),
                    ended: Some(ended),
                })
                .unwrap();
            let (started, start) = tokio::sync::oneshot::channel();
            let (release, released) = std::sync::mpsc::channel();
            let (protocol, _events) = protocol(EmptyStore {
                executor: Some(std::thread::current().id()),
                gate: Some(std::sync::Arc::new(StoreGate {
                    started: std::sync::Mutex::new(Some(started)),
                    release: std::sync::Mutex::new(released),
                })),
                fail: false,
            });
            let running = tokio::spawn(async move {
                let run = run_finite(
                    protocol,
                    sink::drain(),
                    stream::pending(),
                    std::time::Duration::from_secs(5),
                    work,
                );
                if caller_timeout {
                    assert!(
                        tokio::time::timeout(std::time::Duration::from_millis(20), run)
                            .await
                            .is_err()
                    );
                } else {
                    let _ = run.await;
                }
            });
            start.await.unwrap();
            // Reaching this line while get_log_heights is blocked also proves
            // that the current-thread executor can run application work.
            if caller_timeout {
                running.await.unwrap();
            } else {
                running.abort();
                assert!(running.await.unwrap_err().is_cancelled());
            }
            assert_eq!(capacity.available_permits(), 0);
            assert!(!sessions.work_is_idle());
            let cleanup_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
            release.send(()).unwrap();
            tokio::time::timeout_at(cleanup_deadline, end)
                .await
                .unwrap()
                .unwrap();
            // The capacity destructor signals before SessionWork finishes its
            // own bookkeeping. Observe the final lease, within the same budget.
            assert!(sessions.drain_until(cleanup_deadline).await);
            assert!(capacity.try_acquire().is_ok());
        }
    }

    /// Verifies: TEAM.SYS.TRANS-R07
    #[tokio::test(flavor = "current_thread")]
    async fn protocol_error_and_deadline_release_admission() {
        for fail in [true, false] {
            let sessions = super::super::PeerSessions::new();
            let capacity = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
            let work = sessions
                .begin_work(capacity.clone().try_acquire_owned().unwrap())
                .unwrap();
            let (protocol, _events) = protocol(EmptyStore {
                executor: None,
                gate: None,
                fail,
            });
            let error = run_finite(
                protocol,
                sink::drain(),
                stream::pending(),
                std::time::Duration::from_millis(20),
                work,
            )
            .await
            .err()
            .unwrap();
            assert!(if fail {
                matches!(error, LogSyncError::LogStore(_))
            } else {
                error
                    .to_string()
                    .contains("finite session deadline exceeded")
            });
            assert!(sessions.work_is_idle());
            assert!(capacity.try_acquire().is_ok());
        }
    }

    /// Verifies: TEAM.SYS.TRANS-R07
    #[tokio::test]
    async fn protocol_returns_live_transport_for_the_application_acknowledgement() {
        struct Transport(std::sync::Arc<std::sync::atomic::AtomicBool>);
        impl Drop for Transport {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::Release);
            }
        }
        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let transport = Transport(dropped.clone());
        let sink = futures_util::sink::unfold(
            transport,
            |transport, _: LogSyncMessage<String>| async move { Ok::<_, ()>(transport) },
        );
        let (protocol, _events) = protocol(EmptyStore {
            executor: None,
            gate: None,
            fail: false,
        });
        let stream = stream::iter(vec![
            Ok(LogSyncMessage::Have(LogHeights::new())),
            Ok(LogSyncMessage::Done),
            Ok(LogSyncMessage::Done),
        ]);
        let owned = run_finite(
            protocol,
            Box::pin(sink),
            stream,
            std::time::Duration::from_secs(5),
            super::super::PeerSessions::new().begin_work(()).unwrap(),
        )
        .await
        .unwrap();
        assert!(
            !dropped.load(std::sync::atomic::Ordering::Acquire),
            "protocol completion is not transport completion"
        );
        drop(owned);
        assert!(dropped.load(std::sync::atomic::Ordering::Acquire));
    }

    /// Verifies: TEAM.SYS.TRANS-R07
    #[tokio::test(flavor = "current_thread")]
    async fn protocol_work_runs_off_the_async_executor() {
        let (events, _events_rx) = tokio::sync::broadcast::channel::<LogSyncEvent<()>>(8);
        let author = p2panda_core::SigningKey::from_bytes(&[1; 32]).verifying_key();
        let protocol = LogSync::new(
            EmptyStore {
                executor: Some(std::thread::current().id()),
                gate: None,
                fail: false,
            },
            BTreeMap::from([(author, vec!["test".to_owned()])]),
            events,
        );
        let sink = sink::drain::<LogSyncMessage<String>>();
        let messages = stream::iter(vec![
            Ok::<_, super::super::frames::FrameError>(LogSyncMessage::Have(LogHeights::new())),
            Ok(LogSyncMessage::Done),
            Ok(LogSyncMessage::Done),
        ]);
        let _transport = run_finite(
            protocol,
            sink,
            messages,
            std::time::Duration::from_secs(5),
            super::super::PeerSessions::new().begin_work(()).unwrap(),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn adapter_executes_log_sync_over_caller_owned_streams() {
        let (events, _events_rx) = tokio::sync::broadcast::channel::<LogSyncEvent<()>>(8);
        let protocol = LogSync::new(
            EmptyStore {
                executor: None,
                gate: None,
                fail: false,
            },
            BTreeMap::<p2panda_core::VerifyingKey, Vec<String>>::new(),
            events,
        );
        let sink = sink::drain::<LogSyncMessage<String>>();
        let messages: Vec<Result<LogSyncMessage<String>, super::super::frames::FrameError>> = vec![
            Ok(LogSyncMessage::Have(LogHeights::new())),
            Ok(LogSyncMessage::Done),
            Ok(LogSyncMessage::Done),
        ];
        let stream = stream::iter(messages);
        let _transport = run_finite(
            protocol,
            sink,
            stream,
            std::time::Duration::from_secs(5),
            super::super::PeerSessions::new().begin_work(()).unwrap(),
        )
        .await
        .unwrap();
    }
}
