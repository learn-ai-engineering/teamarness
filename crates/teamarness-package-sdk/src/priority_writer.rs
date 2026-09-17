use std::sync::Arc;

use teamarness_package_protocol::{Frame, JsonValue, MAX_WRITER_BYTES, encode_frame};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};

use crate::SdkError;

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
        let (control, control_rx) = mpsc::channel(16);
        let (data, data_rx) = mpsc::channel(48);
        (
            Self {
                control,
                data,
                control_bytes: Arc::new(Semaphore::new(MAX_WRITER_BYTES / 8)),
                data_bytes: Arc::new(Semaphore::new(MAX_WRITER_BYTES * 7 / 8)),
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
) -> Result<(), SdkError>
where
    W: AsyncWrite + Unpin,
{
    loop {
        let queued = tokio::select! {
            biased;
            _ = &mut stop => {
                while let Ok(queued) = control.try_recv().or_else(|_| data.try_recv()) {
                    write(&mut output, &queued).await?;
                }
                return Ok(());
            }
            value = control.recv() => value,
            value = data.recv() => value,
        };
        let Some(queued) = queued else {
            return Ok(());
        };
        write(&mut output, &queued).await?;
    }
}

async fn write<W>(output: &mut W, queued: &QueuedFrame) -> Result<(), SdkError>
where
    W: AsyncWrite + Unpin,
{
    let bytes = encode_frame(&queued.frame).map_err(SdkError::protocol)?;
    output
        .write_all(&bytes)
        .await
        .map_err(|_| SdkError::ParentClosed)?;
    output.flush().await.map_err(|_| SdkError::ParentClosed)
}
