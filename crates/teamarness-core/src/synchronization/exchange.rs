//! One LogSync exchange loop, shared by the initiator and the responder.
//!
//! The loop does two things at once: it drives the upstream LogSync protocol,
//! and it applies received operations in batches on blocking threads. The frame
//! reader task keeps draining the transport throughout, so two peers that both
//! have a large backlog cannot block each other.

use std::collections::BTreeSet;
use std::sync::Arc;

use futures_util::SinkExt;
use p2panda_core::Operation;
use p2panda_sync::protocols::{LogSync, LogSyncEvent};

use super::frames::{FrameError, FrameReader, LogSyncWire};
use super::limits::SessionLimits;
use super::network_handler::{FatalSyncError, LogSessionError, OperationOutcome, SyncReceiver};
use crate::{PublicIdentity, WorkspaceId};

/// How the operations one session received were classified.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutcomeCounts {
    pub(crate) applied: usize,
    pub(crate) duplicate: usize,
    pub(crate) parked: usize,
    pub(crate) rejected: usize,
}

impl OutcomeCounts {
    fn count(&mut self, outcome: &OperationOutcome) {
        match outcome {
            OperationOutcome::Applied => self.applied += 1,
            OperationOutcome::Duplicate => self.duplicate += 1,
            OperationOutcome::Parked(_) => self.parked += 1,
            OperationOutcome::Rejected(_) | OperationOutcome::ParkRefused => self.rejected += 1,
        }
    }
}

/// What both sides learned from one finished session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExchangeReport {
    pub(crate) outcomes: OutcomeCounts,
    /// This side stopped at its send budget and has more to send.
    pub(crate) sent_truncated: bool,
    /// The peer stopped at its send budget and has more to send.
    pub(crate) peer_truncated: bool,
}

/// Which half of the session this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Initiator,
    Responder,
}

/// Who this session is with, and about what.
#[derive(Debug, Clone)]
pub(crate) struct ExchangeContext {
    pub(crate) peer: PublicIdentity,
    pub(crate) workspace: WorkspaceId,
    pub(crate) package: String,
    pub(crate) role: Role,
}

type Logs = std::collections::BTreeMap<p2panda_core::VerifyingKey, Vec<[u8; 32]>>;

/// Runs the LogSync exchange once both sides have been authorized.
///
/// Returns what was received. The caller exchanges the application
/// acknowledgement afterwards, because only it owns the connection.
pub(crate) async fn run_exchange<S, W>(
    reader: &mut FrameReader,
    sink: &mut W,
    store: S,
    logs: Logs,
    receiver: Arc<dyn SyncReceiver>,
    context: ExchangeContext,
    limits: SessionLimits,
) -> Result<OutcomeCounts, LogSessionError>
where
    S: p2panda_store::logs::LogStore<
            Operation<()>,
            p2panda_core::VerifyingKey,
            [u8; 32],
            u32,
            p2panda_core::Hash,
        > + Clone
        + Send
        + 'static,
    W: futures_util::Sink<LogSyncWire, Error = FrameError> + Unpin + Send,
{
    let failure = reader.failure_handle();
    let (events, mut event_rx) =
        tokio::sync::broadcast::channel::<LogSyncEvent<()>>(limits.event_capacity());
    let protocol = LogSync::new(store, logs, events);
    let mut protocol_sink = sink.with(|message| {
        futures_util::future::ready(Ok::<_, FrameError>(LogSyncWire::Message(message)))
    });
    let mut messages = reader.messages(limits);
    let mut running = Box::pin(super::p2panda_adapter::run_finite(
        protocol,
        &mut protocol_sink,
        &mut messages,
    ));

    let mut outcomes = OutcomeCounts::default();
    // One skipped-author set per session, shared by both drain paths.
    let mut skipped = BTreeSet::new();
    loop {
        tokio::select! {
            result = &mut running => {
                let failed = result.err().map(|error| session_error(&error, failure.take()));
                // Whatever LogSync emitted before it stopped is applied either
                // way: a peer that exceeded a bound is disconnected, and the
                // operations it already delivered are kept.
                loop {
                    match event_rx.try_recv() {
                        Ok(event) => {
                            let applied = apply_batch(
                                &receiver, &context, limits, event, &mut event_rx,
                                &mut skipped, &mut outcomes,
                            ).await;
                            match applied {
                                Ok(()) => {}
                                Err(error) => return Err(failed.unwrap_or(error)),
                            }
                        }
                        Err(tokio::sync::broadcast::error::TryRecvError::Empty
                            | tokio::sync::broadcast::error::TryRecvError::Closed) => break,
                        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {
                            debug_assert!(false, "the receive budget makes broadcast lag impossible");
                            return Err(failed.unwrap_or(
                                LogSessionError::Protocol("event lag".to_owned()),
                            ));
                        }
                    }
                }
                return match failed {
                    Some(error) => Err(error),
                    None => Ok(outcomes),
                };
            }
            event = event_rx.recv() => match event {
                Ok(event) => {
                    apply_batch(
                        &receiver, &context, limits, event, &mut event_rx,
                        &mut skipped, &mut outcomes,
                    ).await?;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    debug_assert!(false, "the receive budget makes broadcast lag impossible");
                    return Err(LogSessionError::Protocol("event lag".to_owned()));
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    return Err(LogSessionError::Protocol("event delivery closed".to_owned()));
                }
            },
        }
    }
}

/// Maps a LogSync failure onto the session error.
///
/// The reader names the bound it refused, so a budget or frame refusal keeps
/// its own close code instead of being flattened into a protocol error.
fn session_error(
    error: &p2panda_sync::protocols::LogSyncError,
    reader_failure: Option<FrameError>,
) -> LogSessionError {
    match reader_failure {
        Some(FrameError::BudgetExceeded) => LogSessionError::BudgetExceeded,
        Some(FrameError::TooLarge { .. } | FrameError::Empty) => LogSessionError::FrameTooLarge,
        Some(FrameError::Io(text)) => LogSessionError::Transport(text),
        Some(FrameError::Decode(text)) => LogSessionError::Protocol(text),
        Some(FrameError::Closed) | None => LogSessionError::Protocol(error.to_string()),
    }
}

/// Collects one operation plus up to `apply_batch - 1` more that are already
/// queued, and applies them in one lock hold.
///
/// Events that are not operations are handled inline, so a batch is only ever
/// operations.
async fn apply_batch(
    receiver: &Arc<dyn SyncReceiver>,
    context: &ExchangeContext,
    limits: SessionLimits,
    first: LogSyncEvent<()>,
    events: &mut tokio::sync::broadcast::Receiver<LogSyncEvent<()>>,
    skipped: &mut BTreeSet<p2panda_core::VerifyingKey>,
    outcomes: &mut OutcomeCounts,
) -> Result<(), LogSessionError> {
    let mut batch = Vec::with_capacity(limits.apply_batch);
    push_operation(first, skipped, &mut batch);
    while batch.len() < limits.apply_batch {
        match events.try_recv() {
            Ok(event) => push_operation(event, skipped, &mut batch),
            Err(
                tokio::sync::broadcast::error::TryRecvError::Empty
                | tokio::sync::broadcast::error::TryRecvError::Closed,
            ) => break,
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {
                debug_assert!(false, "the receive budget makes broadcast lag impossible");
                return Err(LogSessionError::Protocol("event lag".to_owned()));
            }
        }
    }
    if batch.is_empty() {
        return Ok(());
    }
    let authors = batch
        .iter()
        .map(|operation| operation.header.verifying_key)
        .collect::<Vec<_>>();
    let peer = context.peer;
    let workspace = context.workspace;
    let package = context.package.clone();
    // Applying operations is SQLite and signature work: it crosses to a
    // blocking thread, so the session never holds the asynchronous executor.
    let applied = crate::blocking::receiver_call(receiver, move |receiver| {
        receiver.receive_operations(peer, workspace, &package, batch)
    })
    .await
    .map_err(|_| LogSessionError::Transport("runtime state is unavailable".to_owned()))?;
    let results = applied.map_err(|error| match error {
        FatalSyncError::PeerRevoked => {
            LogSessionError::Protocol("peer was revoked mid-session".to_owned())
        }
        FatalSyncError::Storage => {
            LogSessionError::Transport("failed to store synchronized operation".to_owned())
        }
    })?;
    for (author, outcome) in authors.into_iter().zip(results) {
        outcomes.count(&outcome);
        if matches!(
            outcome,
            OperationOutcome::Rejected(_) | OperationOutcome::ParkRefused
        ) {
            // Later operations from this author would fail the sequence check
            // anyway, and LogSync redelivers them next time.
            skipped.insert(author);
        }
    }
    Ok(())
}

fn push_operation(
    event: LogSyncEvent<()>,
    skipped: &BTreeSet<p2panda_core::VerifyingKey>,
    batch: &mut Vec<Operation<()>>,
) {
    let LogSyncEvent::OperationReceived { operation, .. } = event else {
        return;
    };
    if skipped.contains(&operation.header.verifying_key) {
        return;
    }
    batch.push(*operation);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::super::network_handler::tests::{ScriptedReceiver, signed_log, test_peer};
    use super::super::p2panda_adapter::TeamarnessLogStore;
    use super::*;

    const LOG: [u8; 32] = [4; 32];

    fn limits(operations: usize, apply_batch: usize) -> SessionLimits {
        SessionLimits {
            operations,
            operation_bytes: 8 * 1024 * 1024,
            control_frames: 64,
            apply_batch,
            frame_bytes: 1024 * 1024,
            ..SessionLimits::CURRENT
        }
    }

    fn context(role: Role) -> ExchangeContext {
        ExchangeContext {
            peer: test_peer(9),
            workspace: WorkspaceId::from_bytes([4; 16]),
            package: "teamarness.test".to_owned(),
            role,
        }
    }

    fn logs_for(seeds: &[u8]) -> Logs {
        seeds
            .iter()
            .map(|seed| {
                (
                    p2panda_core::SigningKey::from_bytes(&[*seed; 32]).verifying_key(),
                    vec![LOG],
                )
            })
            .collect::<BTreeMap<_, _>>()
    }

    fn store(seed: u8, count: u32) -> TeamarnessLogStore {
        TeamarnessLogStore::from_operations(
            signed_log(seed, count)
                .into_iter()
                .map(|operation| (LOG, operation)),
        )
    }

    /// Runs both halves of one exchange over an in-memory transport.
    ///
    /// The transport is real frame I/O over `duplex`, and the LogSync protocol
    /// is real; only the Core seam is scripted.
    async fn run_pair(
        initiator: TeamarnessLogStore,
        responder: TeamarnessLogStore,
        authors: &[u8],
        limits: SessionLimits,
        buffer: usize,
    ) -> (Arc<ScriptedReceiver>, Arc<ScriptedReceiver>) {
        let (left, right) = tokio::io::duplex(buffer);
        let (left_read, left_write) = tokio::io::split(left);
        let (right_read, right_write) = tokio::io::split(right);
        let initiator_receiver = Arc::new(ScriptedReceiver::default());
        let responder_receiver = Arc::new(ScriptedReceiver::default());
        let logs = logs_for(authors);

        let one = {
            let receiver: Arc<dyn SyncReceiver> = Arc::clone(&initiator_receiver) as _;
            let logs = logs.clone();
            async move {
                let mut reader = FrameReader::spawn(left_read, limits);
                reader.set_phase(super::super::frames::FramePhase::Session);
                let mut sink = super::super::frames::frame_sink(left_write, limits);
                run_exchange(
                    &mut reader,
                    &mut sink,
                    initiator,
                    logs,
                    receiver,
                    context(Role::Initiator),
                    limits,
                )
                .await
            }
        };
        let two = {
            let receiver: Arc<dyn SyncReceiver> = Arc::clone(&responder_receiver) as _;
            async move {
                let mut reader = FrameReader::spawn(right_read, limits);
                reader.set_phase(super::super::frames::FramePhase::Session);
                let mut sink = super::super::frames::frame_sink(right_write, limits);
                run_exchange(
                    &mut reader,
                    &mut sink,
                    responder,
                    logs,
                    receiver,
                    context(Role::Responder),
                    limits,
                )
                .await
            }
        };
        let (first, second) = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            futures_util::future::join(one, two),
        )
        .await
        .expect("both halves of the exchange finish");
        first.expect("the initiator half completes");
        second.expect("the responder half completes");
        (initiator_receiver, responder_receiver)
    }

    /// Verifies: TEAM.SYS.TRANS-R07
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bidirectional_full_budget_exchange_does_not_deadlock() {
        let limits = limits(2_048, 16);
        let (initiator, responder) =
            run_pair(store(1, 2_048), store(2, 2_048), &[1, 2], limits, 16 * 1024).await;
        assert_eq!(initiator.seen().len(), 2_048);
        assert_eq!(responder.seen().len(), 2_048);
    }

    /// Verifies: TEAM.SYS.TRANS-R07
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn applies_are_batched_by_at_most_sixteen() {
        let limits = limits(64, 16);
        let (initiator, _) = run_pair(
            TeamarnessLogStore::from_operations(Vec::new()),
            store(2, 40),
            &[2],
            limits,
            16 * 1024,
        )
        .await;
        let batches = initiator.batches();
        assert!(
            batches.iter().all(|size| (1..=16).contains(size)),
            "every batch is between one and sixteen operations: {batches:?}"
        );
        assert_eq!(batches.iter().sum::<usize>(), 40);
        let sequences = initiator
            .seen()
            .into_iter()
            .map(|(_, sequence)| sequence)
            .collect::<Vec<_>>();
        assert_eq!(
            sequences,
            (0..40).collect::<Vec<_>>(),
            "operations reach the receiver in the order they arrived"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn skipped_author_operations_are_not_sent_to_the_receiver() {
        // One operation per batch, so the skip takes effect between operations
        // exactly as it does without batching.
        let limits = limits(64, 1);
        let (left, right) = tokio::io::duplex(16 * 1024);
        let (left_read, left_write) = tokio::io::split(left);
        let (right_read, right_write) = tokio::io::split(right);
        let receiving = Arc::new(ScriptedReceiver::default());
        let rejected = p2panda_core::SigningKey::from_bytes(&[1; 32]).verifying_key();
        receiving.reject(rejected);
        let serving = Arc::new(ScriptedReceiver::default());
        let logs = logs_for(&[1, 2]);

        let mut served = signed_log(1, 3)
            .into_iter()
            .map(|operation| (LOG, operation))
            .collect::<Vec<_>>();
        served.extend(
            signed_log(2, 2)
                .into_iter()
                .map(|operation| (LOG, operation)),
        );

        let one = {
            let receiver: Arc<dyn SyncReceiver> = Arc::clone(&receiving) as _;
            let logs = logs.clone();
            async move {
                let mut reader = FrameReader::spawn(left_read, limits);
                reader.set_phase(super::super::frames::FramePhase::Session);
                let mut sink = super::super::frames::frame_sink(left_write, limits);
                run_exchange(
                    &mut reader,
                    &mut sink,
                    TeamarnessLogStore::from_operations(Vec::new()),
                    logs,
                    receiver,
                    context(Role::Initiator),
                    limits,
                )
                .await
            }
        };
        let two = {
            let receiver: Arc<dyn SyncReceiver> = Arc::clone(&serving) as _;
            async move {
                let mut reader = FrameReader::spawn(right_read, limits);
                reader.set_phase(super::super::frames::FramePhase::Session);
                let mut sink = super::super::frames::frame_sink(right_write, limits);
                run_exchange(
                    &mut reader,
                    &mut sink,
                    TeamarnessLogStore::from_operations(served),
                    logs,
                    receiver,
                    context(Role::Responder),
                    limits,
                )
                .await
            }
        };
        let (first, second) = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            futures_util::future::join(one, two),
        )
        .await
        .expect("both halves finish");
        let outcomes = first.expect("the receiving half completes");
        second.expect("the serving half completes");

        let from_rejected = receiving
            .seen()
            .into_iter()
            .filter(|(author, _)| *author == rejected)
            .count();
        assert_eq!(
            from_rejected, 1,
            "the rejected author's later operations never reach the receiver"
        );
        assert_eq!(outcomes.applied, 2, "the other author's operations applied");
        assert_eq!(outcomes.rejected, 1);
    }
}
