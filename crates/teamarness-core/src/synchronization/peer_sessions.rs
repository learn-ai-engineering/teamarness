//! Level-triggered cancellation of the synchronization sessions of one peer.
//!
//! Accepting a revocation must end every live session with the revoked
//! Installation, inbound and outbound. A registry of `watch` senders keyed by
//! peer gives each session a receiver that observes every later cancellation of
//! that peer, and only that peer.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::watch;

use crate::PublicIdentity;

/// One cancellation generation per peer.
///
/// Cancellation is level-triggered: a receiver taken before `cancel` always
/// observes that cancellation, so a session can subscribe before it does any
/// authorization work without racing a revocation committed in between.
pub(crate) struct PeerSessions {
    peers: Mutex<HashMap<PublicIdentity, watch::Sender<u64>>>,
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
        })
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

    fn identity(seed: u8) -> PublicIdentity {
        PublicIdentity::from_bytes(
            *iroh_base::SecretKey::from_bytes(&[seed; 32])
                .public()
                .as_bytes(),
        )
        .unwrap()
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
