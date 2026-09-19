use std::sync::Arc;

use teamarness_package_protocol::{
    ContractState, Direction, Frame, JsonValue, MAX_FRAME_BYTES, MAX_IN_FLIGHT_EACH_DIRECTION,
    MAX_WRITER_BYTES, encode_frame,
};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, mpsc, oneshot};

use crate::SdkError;

pub(crate) type SharedState = Arc<Mutex<ContractState>>;

/// Lifecycle and health frames reserved beside the answers on the control lane.
///
/// `initialized`, `pong` and `shutdown_ack` are one frame each.
const CONTROL_RESERVE: usize = 3;

/// Slots on the control lane.
///
/// Worst case: one terminal frame for each of the in-flight requests the
/// contract permits the Host, plus the reserved lifecycle frames. A lane
/// shallower than the contract's own concurrency makes a Package that answers
/// its requests wait on its own writer.
const CONTROL_QUEUE: usize = MAX_IN_FLIGHT_EACH_DIRECTION + CONTROL_RESERVE;

/// Slots on the data lane.
///
/// The Package's own in-flight capability requests travel here, each with the
/// stream frames that may follow it.
const DATA_QUEUE: usize = 2 * MAX_IN_FLIGHT_EACH_DIRECTION;

/// The control lane's byte budget: two contract-maximum frames, so one maximum
/// answer never waits on another that is already queued.
const CONTROL_BYTES: usize = 2 * MAX_FRAME_BYTES;

/// The data lane keeps the rest of the writer budget the contract publishes.
const DATA_BYTES: usize = MAX_WRITER_BYTES - CONTROL_BYTES;

pub(crate) struct QueuedFrame {
    frame: Frame,
    _bytes: OwnedSemaphorePermit,
}

#[derive(Clone)]
pub(crate) struct PriorityWriter {
    control: mpsc::Sender<QueuedFrame>,
    data: mpsc::Sender<QueuedFrame>,
    control_bytes: Arc<Semaphore>,
    data_bytes: Arc<Semaphore>,
}

impl PriorityWriter {
    pub(crate) fn channels() -> (
        Self,
        mpsc::Receiver<QueuedFrame>,
        mpsc::Receiver<QueuedFrame>,
    ) {
        let (control, control_rx) = mpsc::channel(CONTROL_QUEUE);
        let (data, data_rx) = mpsc::channel(DATA_QUEUE);
        (
            Self {
                control,
                data,
                control_bytes: Arc::new(Semaphore::new(CONTROL_BYTES)),
                data_bytes: Arc::new(Semaphore::new(DATA_BYTES)),
            },
            control_rx,
            data_rx,
        )
    }

    pub(crate) async fn control(&self, value: JsonValue) -> Result<(), SdkError> {
        self.send(value, true).await
    }

    pub(crate) async fn data(&self, value: JsonValue) -> Result<(), SdkError> {
        self.send(value, false).await
    }

    async fn send(&self, value: JsonValue, control: bool) -> Result<(), SdkError> {
        let frame = Frame::try_from(value).map_err(SdkError::protocol)?;
        let size = encode_frame(&frame).map_err(SdkError::protocol)?.len();
        let permits = u32::try_from(size).map_err(|_| SdkError::Protocol("overloaded"))?;
        let semaphore = if control {
            self.control_bytes.clone()
        } else {
            self.data_bytes.clone()
        };
        let permit = semaphore
            .try_acquire_many_owned(permits)
            .map_err(|_| SdkError::Protocol("overloaded"))?;
        let queued = QueuedFrame {
            frame,
            _bytes: permit,
        };
        let sender = if control { &self.control } else { &self.data };
        sender
            .send(queued)
            .await
            .map_err(|_| SdkError::ParentClosed)
    }
}

/// Writes queued frames until every sender is gone or `stop` fires.
///
/// Package-owned background work may keep `CapabilityClient` clones (and so
/// writer senders) alive indefinitely, so the dispatcher's end of the session
/// cannot rely on the channels closing. When `stop` fires, the writer flushes
/// only frames already queued (such as `shutdown_ack`) and returns.
pub(crate) async fn run<W>(
    mut output: W,
    mut control: mpsc::Receiver<QueuedFrame>,
    mut data: mpsc::Receiver<QueuedFrame>,
    mut stop: oneshot::Receiver<()>,
    state: SharedState,
) -> Result<(), SdkError>
where
    W: AsyncWrite + Unpin,
{
    loop {
        let queued = tokio::select! {
            biased;
            _ = &mut stop => {
                while let Ok(queued) = control.try_recv().or_else(|_| data.try_recv()) {
                    write(&mut output, &queued, &state).await?;
                }
                return Ok(());
            }
            value = control.recv() => value,
            value = data.recv() => value,
        };
        let Some(queued) = queued else {
            return Ok(());
        };
        write(&mut output, &queued, &state).await?;
    }
}

/// Writes one frame, after the shared classifier has seen it.
///
/// Frames are observed here, in the order they reach the wire, so the Package's
/// view of the contract is the one the Host will see. The verdict is about
/// delivery, which a writer has no part in: it writes every frame the
/// classifier accepts, and ends the session only on one the classifier calls
/// out of contract.
async fn write<W>(output: &mut W, queued: &QueuedFrame, state: &SharedState) -> Result<(), SdkError>
where
    W: AsyncWrite + Unpin,
{
    state
        .lock()
        .await
        .observe(Direction::PackageToHost, &queued.frame)
        .map_err(SdkError::protocol)?;
    let bytes = encode_frame(&queued.frame).map_err(SdkError::protocol)?;
    output
        .write_all(&bytes)
        .await
        .map_err(|_| SdkError::ParentClosed)?;
    output.flush().await.map_err(|_| SdkError::ParentClosed)
}

#[cfg(test)]
mod tests {
    use super::{
        CONTROL_BYTES, CONTROL_QUEUE, CONTROL_RESERVE, DATA_BYTES, DATA_QUEUE,
        MAX_IN_FLIGHT_EACH_DIRECTION, MAX_WRITER_BYTES,
    };
    use teamarness_package_protocol::{MAX_FRAME_BYTES, MAX_WRITER_FRAMES};

    /// Every writer bound admits the worst case the contract permits.
    #[test]
    fn writer_lanes_admit_the_traffic_the_contract_permits() {
        const {
            assert!(CONTROL_QUEUE >= MAX_IN_FLIGHT_EACH_DIRECTION + CONTROL_RESERVE);
            assert!(DATA_QUEUE >= MAX_IN_FLIGHT_EACH_DIRECTION);
            assert!(CONTROL_QUEUE <= MAX_WRITER_FRAMES);
            assert!(DATA_QUEUE <= MAX_WRITER_FRAMES);
            assert!(CONTROL_BYTES >= 2 * MAX_FRAME_BYTES);
            assert!(DATA_BYTES >= 2 * MAX_FRAME_BYTES);
            assert!(CONTROL_BYTES + DATA_BYTES == MAX_WRITER_BYTES);
        }
    }
}
