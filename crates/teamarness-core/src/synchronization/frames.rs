//! Length-delimited, bounded frame I/O for `teamarness/package-sync/2`.
//!
//! Every frame is read in two steps: a four-byte length, then exactly that many
//! bytes. The length is checked against the cap for the session's current
//! phase **before anything is allocated**, so an oversized claim costs nothing.
//! A decoded frame is then charged to the session's receive budget, so the
//! number of frames and their total bytes are bounded for the whole session.
//!
//! The reader runs as its own task and never waits on its consumer. That is
//! what keeps two peers with large backlogs from blocking each other: the
//! upstream LogSync protocol does not read while it sends, so the socket must
//! be drained by someone else. The queue between the reader and the session is
//! unbounded in type, but bounded in fact by the receive budget: the reader
//! stops the session at operation `operations + 1`, and the bytes it can have
//! forwarded are at most `operation_bytes` plus `control_frames` small frames.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use futures_util::{Sink, Stream};
use p2panda_sync::protocols::LogSyncMessage;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::ProtectionBundle;
use super::limits::{MAX_PROTECTION_CONTROL_BYTES, ReceiveBudget, SessionLimits};

/// Which cap applies to the next frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FramePhase {
    Hello,
    Handshake,
    Session,
}

impl FramePhase {
    const fn code(self) -> u8 {
        match self {
            Self::Hello => 0,
            Self::Handshake => 1,
            Self::Session => 2,
        }
    }

    const fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Hello,
            1 => Self::Handshake,
            _ => Self::Session,
        }
    }

    const fn cap(self, limits: &SessionLimits) -> usize {
        match self {
            Self::Hello => limits.hello_bytes,
            Self::Handshake => limits.handshake_frame_bytes,
            Self::Session => limits.frame_bytes,
        }
    }
}

/// Why frame I/O stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FrameError {
    /// The peer closed the stream.
    Closed,
    /// The claimed length is larger than this phase allows. Nothing was read
    /// into memory for it.
    TooLarge {
        length: usize,
        cap: usize,
    },
    /// A zero-length frame carries nothing and is never sent by an honest peer.
    Empty,
    /// The peer asked for more than its session budget allows.
    BudgetExceeded,
    Decode(String),
    Io(String),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => formatter.write_str("package sync stream closed"),
            Self::TooLarge { length, cap } => {
                write!(formatter, "package sync frame of {length} exceeds {cap}")
            }
            Self::Empty => formatter.write_str("package sync frame is empty"),
            Self::BudgetExceeded => formatter.write_str("package sync session budget exceeded"),
            Self::Decode(error) => write!(formatter, "package sync frame decode: {error}"),
            Self::Io(error) => write!(formatter, "package sync frame io: {error}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProtectionBundleWire {
    pub(crate) origin: Vec<u8>,
    pub(crate) grant: Vec<u8>,
}

impl ProtectionBundleWire {
    pub(crate) fn validated(self) -> Result<ProtectionBundle, String> {
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

    pub(crate) fn from_bundle(bundle: ProtectionBundle) -> Self {
        Self {
            origin: bundle.origin,
            grant: bundle.grant,
        }
    }
}

/// Every frame `teamarness/package-sync/2` carries.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum LogSyncWire {
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

impl LogSyncWire {
    const fn outbound_cap(&self, limits: &SessionLimits) -> usize {
        match self {
            Self::Hello { .. } => limits.hello_bytes,
            Self::Welcome { .. } | Self::Ready { .. } | Self::Refused { .. } => {
                limits.handshake_frame_bytes
            }
            Self::Message(_) => limits.frame_bytes,
        }
    }
}

/// A shared view of why one reader stopped.
#[derive(Clone)]
pub(crate) struct FrameFailure(Arc<std::sync::Mutex<Option<FrameError>>>);

impl FrameFailure {
    pub(crate) fn take(&self) -> Option<FrameError> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// Remembers why the reader stopped and tells the session about it.
fn record(
    failure: &std::sync::Mutex<Option<FrameError>>,
    outbound: &tokio::sync::mpsc::UnboundedSender<Result<LogSyncWire, FrameError>>,
    error: FrameError,
) {
    *failure
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error.clone());
    let _ = outbound.send(Err(error));
}

/// A bounded reader over one package synchronization stream.
pub(crate) struct FrameReader {
    inbound: tokio::sync::mpsc::UnboundedReceiver<Result<LogSyncWire, FrameError>>,
    phase: Arc<AtomicU8>,
    /// The failure that stopped the reader, if any. The session reads it after
    /// the protocol fails, so a bound refusal keeps its own close code instead
    /// of being flattened into a protocol error.
    failure: Arc<std::sync::Mutex<Option<FrameError>>>,
    #[cfg(test)]
    allocations: Arc<std::sync::atomic::AtomicUsize>,
}

impl FrameReader {
    /// Spawns the reader task in the [`FramePhase::Hello`] phase.
    pub(crate) fn spawn<R>(mut reader: R, limits: SessionLimits) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        let phase = Arc::new(AtomicU8::new(FramePhase::Hello.code()));
        let task_phase = Arc::clone(&phase);
        let failure: Arc<std::sync::Mutex<Option<FrameError>>> =
            Arc::new(std::sync::Mutex::new(None));
        let task_failure = Arc::clone(&failure);
        #[cfg(test)]
        let allocations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        #[cfg(test)]
        let task_allocations = Arc::clone(&allocations);
        // The queue is unbounded in type only: what the task may forward is
        // bounded by the receive budget it enforces before forwarding.
        let (outbound, inbound) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut budget = ReceiveBudget::new(&limits);
            loop {
                let mut header = [0_u8; 4];
                if reader.read_exact(&mut header).await.is_err() {
                    break;
                }
                let length = u32::from_be_bytes(header) as usize;
                if length == 0 {
                    record(&task_failure, &outbound, FrameError::Empty);
                    break;
                }
                let cap = FramePhase::from_code(task_phase.load(Ordering::Acquire)).cap(&limits);
                if length > cap {
                    // Nothing is allocated for a claim this large.
                    record(
                        &task_failure,
                        &outbound,
                        FrameError::TooLarge { length, cap },
                    );
                    break;
                }
                let mut bytes = vec![0_u8; length];
                #[cfg(test)]
                task_allocations.fetch_add(1, Ordering::Release);
                if let Err(error) = reader.read_exact(&mut bytes).await {
                    record(&task_failure, &outbound, FrameError::Io(error.to_string()));
                    break;
                }
                let wire: LogSyncWire = match postcard::from_bytes(&bytes) {
                    Ok(wire) => wire,
                    Err(error) => {
                        record(
                            &task_failure,
                            &outbound,
                            FrameError::Decode(error.to_string()),
                        );
                        break;
                    }
                };
                let charged = if matches!(wire, LogSyncWire::Message(LogSyncMessage::Operation(_)))
                {
                    budget.charge_operation(length)
                } else {
                    budget.charge_control()
                };
                if charged.is_err() {
                    record(&task_failure, &outbound, FrameError::BudgetExceeded);
                    break;
                }
                if outbound.send(Ok(wire)).is_err() {
                    break;
                }
            }
        });
        Self {
            inbound,
            phase,
            failure,
            #[cfg(test)]
            allocations,
        }
    }

    /// A handle to the failure that stopped the reader, if it stopped for a
    /// reason of its own rather than because the peer closed the stream.
    ///
    /// It is a handle rather than a value so a session can hold it while the
    /// reader itself is borrowed by the protocol's message stream.
    pub(crate) fn failure_handle(&self) -> FrameFailure {
        FrameFailure(Arc::clone(&self.failure))
    }

    pub(crate) fn set_phase(&self, phase: FramePhase) {
        self.phase.store(phase.code(), Ordering::Release);
    }

    pub(crate) async fn next_wire(&mut self) -> Result<LogSyncWire, FrameError> {
        match self.inbound.recv().await {
            Some(result) => result,
            None => Err(FrameError::Closed),
        }
    }

    /// Adapts the reader for the LogSync protocol, which speaks only
    /// [`LogSyncMessage`]s. A handshake frame here is a protocol error.
    pub(crate) fn messages(&mut self, limits: SessionLimits) -> FrameMessages<'_> {
        let failure = Arc::clone(&self.failure);
        FrameMessages {
            reader: self,
            limits,
            failure,
        }
    }

    /// How many frame bodies the reader allocated. An oversized claim must not
    /// increase this.
    #[cfg(test)]
    pub(crate) fn allocations(&self) -> usize {
        self.allocations.load(Ordering::Acquire)
    }
}

pub(crate) struct FrameMessages<'a> {
    reader: &'a mut FrameReader,
    limits: SessionLimits,
    failure: Arc<std::sync::Mutex<Option<FrameError>>>,
}

impl Stream for FrameMessages<'_> {
    type Item = Result<LogSyncMessage<[u8; 32]>, FrameError>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let limits = this.limits;
        let failure = Arc::clone(&this.failure);
        this.reader.inbound.poll_recv(context).map(|item| {
            item.map(|result| match result {
                Ok(LogSyncWire::Message(LogSyncMessage::PreSync {
                    total_operations,
                    total_bytes,
                })) if total_operations as usize > limits.operations
                    || total_bytes as usize > limits.operation_bytes =>
                {
                    // The peer announced more than the session allows, so it is
                    // stopped before a single operation is exchanged.
                    let mut slot = failure
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    slot.get_or_insert(FrameError::BudgetExceeded);
                    Err(FrameError::BudgetExceeded)
                }
                Ok(LogSyncWire::Message(message)) => Ok(message),
                Ok(_) => Err(FrameError::Decode(
                    "unexpected package sync handshake frame".to_owned(),
                )),
                Err(error) => Err(error),
            })
        })
    }
}

/// A sink that writes frames straight to the transport.
///
/// There is no buffer: QUIC stream flow control is the backpressure, so a peer
/// that stops reading slows this side down instead of growing its memory.
pub(crate) fn frame_sink<W>(
    writer: W,
    limits: SessionLimits,
) -> impl Sink<LogSyncWire, Error = FrameError> + Unpin + Send
where
    W: AsyncWrite + Unpin + Send + 'static,
{
    Box::pin(futures_util::sink::unfold(
        (writer, limits),
        |(mut writer, limits), wire: LogSyncWire| async move {
            let cap = wire.outbound_cap(&limits);
            let bytes = postcard::to_stdvec(&wire)
                .map_err(|error| FrameError::Decode(error.to_string()))?;
            if bytes.is_empty() {
                return Err(FrameError::Empty);
            }
            if bytes.len() > cap {
                return Err(FrameError::TooLarge {
                    length: bytes.len(),
                    cap,
                });
            }
            let length = u32::try_from(bytes.len()).map_err(|_| FrameError::TooLarge {
                length: bytes.len(),
                cap,
            })?;
            writer
                .write_all(&length.to_be_bytes())
                .await
                .map_err(|error| FrameError::Io(error.to_string()))?;
            writer
                .write_all(&bytes)
                .await
                .map_err(|error| FrameError::Io(error.to_string()))?;
            Ok((writer, limits))
        },
    ))
}

#[cfg(test)]
mod tests {
    use futures_util::{FutureExt, SinkExt, StreamExt};
    use p2panda_core::logs::LogHeights;

    use super::*;

    fn small() -> SessionLimits {
        SessionLimits {
            hello_bytes: 1024,
            handshake_frame_bytes: 2048,
            frame_bytes: 4096,
            operations: 3,
            operation_bytes: 4096,
            control_frames: 2,
            ..SessionLimits::CURRENT
        }
    }

    fn operation_frame(seed: u8, sequence: u32) -> LogSyncWire {
        let key = p2panda_core::SigningKey::from_bytes(&[seed; 32]);
        let payload = vec![seed; 16];
        let header = p2panda_core::Header::<()>::builder()
            .body(payload.as_slice())
            .seq_num(sequence)
            .build(&key, ());
        LogSyncWire::Message(LogSyncMessage::Operation((header.encode(), Some(payload))))
    }

    async fn write_frame(writer: &mut tokio::io::DuplexStream, wire: &LogSyncWire) {
        let bytes = postcard::to_stdvec(wire).unwrap();
        let length = u32::try_from(bytes.len()).unwrap();
        writer.write_all(&length.to_be_bytes()).await.unwrap();
        writer.write_all(&bytes).await.unwrap();
    }

    #[tokio::test]
    async fn oversized_frame_is_rejected_before_allocation() {
        let (mut peer, local) = tokio::io::duplex(64 * 1024);
        let limits = small();
        let mut reader = FrameReader::spawn(local, limits);
        reader.set_phase(FramePhase::Session);
        let length = u32::try_from(limits.frame_bytes + 1).unwrap();
        peer.write_all(&length.to_be_bytes()).await.unwrap();

        assert_eq!(
            reader.next_wire().await.err(),
            Some(FrameError::TooLarge {
                length: limits.frame_bytes + 1,
                cap: limits.frame_bytes,
            })
        );
        assert_eq!(reader.allocations(), 0, "no body was ever allocated");
    }

    #[tokio::test]
    async fn hello_phase_uses_the_hello_cap() {
        let (mut peer, local) = tokio::io::duplex(64 * 1024);
        let limits = small();
        let mut reader = FrameReader::spawn(local, limits);
        peer.write_all(&1025_u32.to_be_bytes()).await.unwrap();
        assert_eq!(
            reader.next_wire().await.err(),
            Some(FrameError::TooLarge {
                length: 1025,
                cap: 1024
            })
        );
    }

    #[tokio::test]
    async fn zero_length_frame_is_rejected() {
        let (mut peer, local) = tokio::io::duplex(64 * 1024);
        let mut reader = FrameReader::spawn(local, small());
        peer.write_all(&0_u32.to_be_bytes()).await.unwrap();
        assert_eq!(reader.next_wire().await.err(), Some(FrameError::Empty));
    }

    #[tokio::test]
    async fn receive_budget_stops_at_the_operation_limit() {
        let (mut peer, local) = tokio::io::duplex(256 * 1024);
        let mut reader = FrameReader::spawn(local, small());
        reader.set_phase(FramePhase::Session);
        for sequence in 0..4 {
            write_frame(&mut peer, &operation_frame(7, sequence)).await;
        }
        for _ in 0..3 {
            assert!(matches!(
                reader.next_wire().await,
                Ok(LogSyncWire::Message(_))
            ));
        }
        assert_eq!(
            reader.next_wire().await.err(),
            Some(FrameError::BudgetExceeded)
        );
    }

    #[tokio::test]
    async fn receive_budget_stops_at_the_byte_limit() {
        let (mut peer, local) = tokio::io::duplex(256 * 1024);
        let limits = SessionLimits {
            operation_bytes: 32,
            ..small()
        };
        let mut reader = FrameReader::spawn(local, limits);
        reader.set_phase(FramePhase::Session);
        write_frame(&mut peer, &operation_frame(7, 0)).await;
        assert_eq!(
            reader.next_wire().await.err(),
            Some(FrameError::BudgetExceeded)
        );
    }

    #[tokio::test]
    async fn control_frame_budget_is_enforced() {
        let (mut peer, local) = tokio::io::duplex(256 * 1024);
        let mut reader = FrameReader::spawn(local, small());
        reader.set_phase(FramePhase::Session);
        for _ in 0..3 {
            write_frame(&mut peer, &LogSyncWire::Message(LogSyncMessage::Done)).await;
        }
        for _ in 0..2 {
            assert!(matches!(
                reader.next_wire().await,
                Ok(LogSyncWire::Message(_))
            ));
        }
        assert_eq!(
            reader.next_wire().await.err(),
            Some(FrameError::BudgetExceeded)
        );
    }

    #[tokio::test]
    async fn presync_announcing_more_than_the_budget_fails_early() {
        let (mut peer, local) = tokio::io::duplex(256 * 1024);
        let limits = small();
        let mut reader = FrameReader::spawn(local, limits);
        reader.set_phase(FramePhase::Session);
        write_frame(
            &mut peer,
            &LogSyncWire::Message(LogSyncMessage::PreSync {
                total_operations: u32::try_from(limits.operations).unwrap() + 1,
                total_bytes: 16,
            }),
        )
        .await;
        let mut messages = reader.messages(limits);
        assert_eq!(
            messages.next().await.and_then(Result::err),
            Some(FrameError::BudgetExceeded)
        );
    }

    #[tokio::test]
    async fn handshake_frame_inside_a_session_is_a_protocol_error() {
        let (mut peer, local) = tokio::io::duplex(64 * 1024);
        let limits = small();
        let mut reader = FrameReader::spawn(local, limits);
        reader.set_phase(FramePhase::Session);
        write_frame(&mut peer, &LogSyncWire::Refused { reason: 1 }).await;
        let mut messages = reader.messages(limits);
        assert!(matches!(
            messages.next().await,
            Some(Err(FrameError::Decode(_)))
        ));
    }

    #[tokio::test]
    async fn sink_applies_backpressure_when_the_peer_does_not_read() {
        let (_peer, local) = tokio::io::duplex(64 * 1024);
        let limits = SessionLimits {
            frame_bytes: 4 * 1024 * 1024,
            ..SessionLimits::CURRENT
        };
        let mut sink = frame_sink(local, limits);
        // The peer end is never read, so the transport buffer fills and the
        // sink waits. It must not fail, and it must not buffer the frame.
        let payload = vec![3_u8; 1024 * 1024];
        let header = p2panda_core::Header::<()>::builder()
            .body(payload.as_slice())
            .build(&p2panda_core::SigningKey::from_bytes(&[5; 32]), ());
        let frame =
            LogSyncWire::Message(LogSyncMessage::Operation((header.encode(), Some(payload))));
        assert!(
            sink.send(frame).now_or_never().is_none(),
            "a full transport makes the sink pending, not failed"
        );
    }

    #[tokio::test]
    async fn sink_refuses_a_frame_larger_than_its_phase_cap() {
        let (mut peer, local) = tokio::io::duplex(1024 * 1024);
        let limits = SessionLimits {
            handshake_frame_bytes: 64,
            ..SessionLimits::CURRENT
        };
        let mut sink = frame_sink(local, limits);
        let oversized = LogSyncWire::Ready {
            protection: Some(ProtectionBundleWire {
                origin: vec![1; 512],
                grant: vec![2; 512],
            }),
        };
        assert!(matches!(
            sink.send(oversized).await,
            Err(FrameError::TooLarge { cap: 64, .. })
        ));
        drop(peer.write_all(b"").await);
    }

    #[tokio::test]
    async fn frames_round_trip_through_the_sink_and_the_reader() {
        let (peer, local) = tokio::io::duplex(64 * 1024);
        let limits = small();
        let mut sink = frame_sink(peer, limits);
        let mut reader = FrameReader::spawn(local, limits);
        sink.send(LogSyncWire::Hello {
            workspace: [3; 16],
            package: "teamarness.test".to_owned(),
            membership_version: [4; 32],
            content_key_id: Some([5; 32]),
        })
        .await
        .unwrap();
        assert!(matches!(
            reader.next_wire().await,
            Ok(LogSyncWire::Hello { package, .. }) if package == "teamarness.test"
        ));
        reader.set_phase(FramePhase::Session);
        sink.send(LogSyncWire::Message(
            LogSyncMessage::Have(LogHeights::new()),
        ))
        .await
        .unwrap();
        assert!(matches!(
            reader.next_wire().await,
            Ok(LogSyncWire::Message(LogSyncMessage::Have(_)))
        ));
    }
}
