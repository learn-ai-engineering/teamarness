//! Level-triggered cancellation of the synchronization sessions of one peer.
//!
//! Accepting a revocation must end every live session with the revoked
//! Installation, inbound and outbound. A registry of `watch` senders keyed by
//! peer gives each session a receiver that observes every later cancellation of
//! that peer, and only that peer.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::{Notify, watch};

use crate::PublicIdentity;

/// One cancellation generation per peer.
///
/// Cancellation is level-triggered: a receiver taken before `cancel` always
/// observes that cancellation, so a session can subscribe before it does any
/// authorization work without racing a revocation committed in between.
pub(crate) struct PeerSessions {
    peers: Mutex<HashMap<PublicIdentity, watch::Sender<u64>>>,
    work: Mutex<WorkState>,
    drained: Notify,
}

impl std::fmt::Debug for PeerSessions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PeerSessions")
            .finish_non_exhaustive()
    }
}

impl PeerSessions {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            peers: Mutex::new(HashMap::new()),
            work: Mutex::new(WorkState::default()),
            drained: Notify::new(),
        })
    }

    /// Accounts for a session until its last worker or storage callback exits.
    pub(crate) fn begin_work(
        self: &Arc<Self>,
        capacity: impl Send + Sync + 'static,
    ) -> Option<Arc<SessionWork>> {
        let mut work = self.work.lock().unwrap_or_else(PoisonError::into_inner);
        if work.closed {
            return None;
        }
        work.active += 1;
        Some(Arc::new(SessionWork {
            sessions: Arc::clone(self),
            capacity: Some(Box::new(capacity)),
            outbound_peer: None,
        }))
    }

    pub(crate) fn begin_outbound_work(
        self: &Arc<Self>,
        peer: PublicIdentity,
        capacity: impl Send + Sync + 'static,
    ) -> Option<Arc<SessionWork>> {
        let mut work = self.work.lock().unwrap_or_else(PoisonError::into_inner);
        if work.closed || !work.outbound.insert(peer) {
            return None;
        }
        work.active += 1;
        Some(Arc::new(SessionWork {
            sessions: Arc::clone(self),
            capacity: Some(Box::new(capacity)),
            outbound_peer: Some(peer),
        }))
    }

    pub(crate) fn close_work(&self) {
        self.work
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .closed = true;
    }

    pub(crate) fn work_is_idle(&self) -> bool {
        self.work
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .active
            == 0
    }

    /// Drains cancellation callbacks within the caller's existing shutdown window.
    pub(crate) async fn drain_until(&self, deadline: tokio::time::Instant) -> bool {
        loop {
            let drained = self.drained.notified();
            tokio::pin!(drained);
            drained.as_mut().enable();
            if self.work_is_idle() {
                return true;
            }
            if tokio::time::timeout_at(deadline, drained).await.is_err() {
                return self.work_is_idle();
            }
        }
    }

    /// Returns a receiver that observes every later [`Self::cancel`] of `peer`.
    pub(crate) fn subscribe(&self, peer: PublicIdentity) -> watch::Receiver<u64> {
        let mut peers = self.peers.lock().unwrap_or_else(PoisonError::into_inner);
        peers
            .entry(peer)
            .or_insert_with(|| watch::channel(0).0)
            .subscribe()
    }

    /// Cancels every session of `peer` that subscribed before this call.
    pub(crate) fn cancel(&self, peer: PublicIdentity) {
        let mut peers = self.peers.lock().unwrap_or_else(PoisonError::into_inner);
        peers
            .entry(peer)
            .or_insert_with(|| watch::channel(0).0)
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }
}

#[derive(Default)]
struct WorkState {
    closed: bool,
    active: usize,
    outbound: std::collections::HashSet<PublicIdentity>,
}

/// The original admission capacity, shared with all surviving session work.
/// A cancelled observer cannot admit a replacement while that work still runs.
pub(crate) struct SessionWork {
    sessions: Arc<PeerSessions>,
    capacity: Option<Box<dyn Send + Sync>>,
    outbound_peer: Option<PublicIdentity>,
}

impl std::fmt::Debug for SessionWork {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionWork")
            .finish_non_exhaustive()
    }
}

impl SessionWork {
    pub(crate) async fn receiver_call<T, F>(
        self: &Arc<Self>,
        receiver: &Arc<dyn crate::synchronization::SyncReceiver>,
        f: F,
    ) -> Result<T, crate::blocking::StateUnavailable>
    where
        T: Send + 'static,
        F: FnOnce(&dyn crate::synchronization::SyncReceiver) -> T + Send + 'static,
    {
        let work = Arc::clone(self);
        crate::blocking::receiver_call(receiver, move |receiver| {
            let _work = work;
            f(receiver)
        })
        .await
    }
}

impl Drop for SessionWork {
    fn drop(&mut self) {
        // Idle must imply that the original admission capacity is released too.
        drop(self.capacity.take());
        let mut work = self
            .sessions
            .work
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        work.active -= 1;
        if let Some(peer) = self.outbound_peer {
            work.outbound.remove(&peer);
        }
        let idle = work.active == 0;
        drop(work);
        if idle {
            self.sessions.drained.notify_waiters();
        }
    }
}

/// Completes once `receiver`'s peer is cancelled, and otherwise never.
pub(crate) async fn cancelled(receiver: &mut watch::Receiver<u64>) {
    // The registry owns the sender for as long as the runtime lives, so a
    // closed channel means only that the registry is gone: never cancel then.
    if receiver.changed().await.is_err() {
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_drain_waits_for_the_final_work_lease() {
        let sessions = PeerSessions::new();
        let work = sessions.begin_work(()).unwrap();
        sessions.close_work();
        let drain = sessions.drain_until(
            tokio::time::Instant::now()
                + super::super::limits::SchedulerLimits::CURRENT.shutdown_grace,
        );
        tokio::pin!(drain);
        assert!(
            futures_util::poll!(&mut drain).is_pending(),
            "shutdown must drain normal completing work, not sample its current count"
        );
        drop(work);
        assert!(drain.await);
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_drain_does_not_extend_the_existing_grace() {
        let sessions = PeerSessions::new();
        let work = sessions.begin_work(()).unwrap();
        sessions.close_work();
        let start = tokio::time::Instant::now();
        let grace = super::super::limits::SchedulerLimits::CURRENT.shutdown_grace;
        assert!(!sessions.drain_until(start + grace).await);
        assert_eq!(start.elapsed(), grace);
        assert!(!sessions.work_is_idle());
        drop(work);
        assert!(sessions.drain_until(start + grace).await);
    }

    fn identity(seed: u8) -> PublicIdentity {
        PublicIdentity::from_bytes(
            *iroh_base::SecretKey::from_bytes(&[seed; 32])
                .public()
                .as_bytes(),
        )
        .unwrap()
    }

    #[test]
    fn surviving_outbound_work_keeps_its_peer_exclusive_and_shutdown_fenced() {
        let sessions = PeerSessions::new();
        let peer = identity(5);
        let work = sessions.begin_outbound_work(peer, ()).unwrap();
        let surviving = Arc::clone(&work);
        drop(work);
        assert!(sessions.begin_outbound_work(peer, ()).is_none());
        assert!(!sessions.work_is_idle());
        drop(surviving);
        let work = sessions.begin_outbound_work(peer, ()).unwrap();
        sessions.close_work();
        assert!(sessions.begin_work(()).is_none());
        drop(work);
        assert!(sessions.work_is_idle());
    }

    #[tokio::test]
    async fn cancel_is_observed_by_receivers_created_before_it() {
        let sessions = PeerSessions::new();
        let peer = identity(1);
        let mut receiver = sessions.subscribe(peer);
        sessions.cancel(peer);
        tokio::time::timeout(std::time::Duration::from_secs(1), cancelled(&mut receiver))
            .await
            .expect("a receiver taken before the cancellation must observe it");
    }

    #[tokio::test]
    async fn cancel_of_one_peer_does_not_affect_another() {
        let sessions = PeerSessions::new();
        let mut other = sessions.subscribe(identity(2));
        sessions.cancel(identity(3));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), cancelled(&mut other))
                .await
                .is_err()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn receiver_waiting_in_select_is_woken() {
        let sessions = PeerSessions::new();
        let peer = identity(4);
        let mut receiver = sessions.subscribe(peer);
        let waiting = tokio::spawn(async move {
            tokio::select! {
                () = cancelled(&mut receiver) => true,
                () = tokio::time::sleep(std::time::Duration::from_secs(5)) => false,
            }
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        sessions.cancel(peer);
        assert!(waiting.await.unwrap());
    }
}
