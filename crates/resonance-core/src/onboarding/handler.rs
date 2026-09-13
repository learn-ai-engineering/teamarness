use std::collections::HashSet;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh_base::PublicKey;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::timeout;

use super::wire::{MAX_REQUEST_BODY_BYTES, decode_request};

pub(crate) const MAX_INBOUND_HANDLERS: usize = 8;

pub(crate) struct OnboardingHandler {
    stopping: AtomicBool,
    inbound: Arc<Semaphore>,
    peers: Arc<Mutex<HashSet<PublicKey>>>,
    request_timeout: Duration,
    #[cfg(test)]
    handled: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    handled_notify: tokio::sync::Notify,
}

impl fmt::Debug for OnboardingHandler {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OnboardingHandler")
            .field("stopping", &self.stopping.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl OnboardingHandler {
    pub(crate) fn new(request_timeout: Duration) -> Self {
        Self {
            stopping: AtomicBool::new(false),
            inbound: Arc::new(Semaphore::new(MAX_INBOUND_HANDLERS)),
            peers: Arc::new(Mutex::new(HashSet::new())),
            request_timeout,
            #[cfg(test)]
            handled: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            handled_notify: tokio::sync::Notify::new(),
        }
    }

    pub(crate) fn acquire(&self, peer: PublicKey) -> Option<InboundPermit> {
        if self.stopping.load(Ordering::Acquire) {
            return None;
        }
        let global = Arc::clone(&self.inbound).try_acquire_owned().ok()?;
        let mut peers = self
            .peers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !peers.insert(peer) {
            return None;
        }
        Some(InboundPermit {
            _global: global,
            peer,
            peers: Arc::clone(&self.peers),
        })
    }

    pub(crate) fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn available_inbound_permits(&self) -> usize {
        self.inbound.available_permits()
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_handled(&self, count: usize) {
        while self.handled.load(Ordering::Acquire) < count {
            self.handled_notify.notified().await;
        }
    }
}

impl ProtocolHandler for OnboardingHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let Some(_permit) = self.acquire(connection.remote_id()) else {
            connection.close(1_u8.into(), b"onboarding unavailable");
            return Ok(());
        };
        let stream = timeout(self.request_timeout, connection.accept_bi()).await;
        let Ok(Ok((_send, mut receive))) = stream else {
            connection.close(2_u8.into(), b"onboarding request timeout");
            return Ok(());
        };
        let frame = timeout(
            self.request_timeout,
            receive.read_to_end(MAX_REQUEST_BODY_BYTES + 4),
        )
        .await;
        if let Ok(Ok(frame)) = frame {
            let _ = decode_request(&frame);
            #[cfg(test)]
            {
                self.handled.fetch_add(1, Ordering::AcqRel);
                self.handled_notify.notify_one();
            }
        }
        Ok(())
    }

    async fn shutdown(&self) {
        self.stop();
    }
}

pub(crate) struct InboundPermit {
    _global: OwnedSemaphorePermit,
    peer: PublicKey,
    peers: Arc<Mutex<HashSet<PublicKey>>>,
}

impl Drop for InboundPermit {
    fn drop(&mut self) {
        self.peers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.peer);
    }
}
