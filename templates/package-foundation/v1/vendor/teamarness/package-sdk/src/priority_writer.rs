use std::sync::Arc;

use teamarness_package_protocol::{Frame, JsonValue, MAX_WRITER_BYTES, encode_frame};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};

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

pub(crate) async fn run<W>(
    mut output: W,
    mut control: mpsc::Receiver<QueuedFrame>,
    mut data: mpsc::Receiver<QueuedFrame>,
) -> Result<(), SdkError>
where
    W: AsyncWrite + Unpin,
{
    loop {
        let queued = tokio::select! {
            biased;
            value = control.recv() => value,
            value = data.recv() => value,
        };
        let Some(queued) = queued else {
            return Ok(());
        };
        let bytes = encode_frame(&queued.frame).map_err(SdkError::protocol)?;
        output
            .write_all(&bytes)
            .await
            .map_err(|_| SdkError::ParentClosed)?;
        output.flush().await.map_err(|_| SdkError::ParentClosed)?;
    }
}
