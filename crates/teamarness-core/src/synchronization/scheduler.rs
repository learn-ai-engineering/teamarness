use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::PublicIdentity;

pub(crate) const MAX_OUTBOUND_PEERS: usize = 4;
pub(crate) const MAX_SESSION_OPERATIONS: usize = 4_096;
pub(crate) const MAX_SESSION_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_SESSION_DURATION: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SessionBudget {
    pub(crate) operations: usize,
    pub(crate) bytes: usize,
    pub(crate) deadline: Instant,
}

impl SessionBudget {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            operations: MAX_SESSION_OPERATIONS,
            bytes: MAX_SESSION_BYTES,
            deadline: now + MAX_SESSION_DURATION,
        }
    }

    pub(crate) fn admit(&mut self, bytes: usize, now: Instant) -> bool {
        if now >= self.deadline || self.operations == 0 || bytes > self.bytes {
            return false;
        }
        self.operations -= 1;
        self.bytes -= bytes;
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RetryState {
    failures: u32,
}

impl RetryState {
    pub(crate) const fn new() -> Self {
        Self { failures: 0 }
    }

    pub(crate) fn next_delay(&mut self) -> Duration {
        let shift = self.failures.min(6);
        self.failures = self.failures.saturating_add(1);
        Duration::from_millis(250_u64.saturating_mul(1_u64 << shift)).min(Duration::from_secs(15))
    }

    pub(crate) fn next_delay_jittered(&mut self) -> Duration {
        let delay = self.next_delay();
        let mut sample = [0_u8; 2];
        let _ = getrandom::fill(&mut sample);
        let jitter = u64::from(u16::from_be_bytes(sample)) % 251;
        delay
            .saturating_add(Duration::from_millis(jitter))
            .min(Duration::from_secs(15))
    }

    pub(crate) fn succeeded(&mut self) {
        self.failures = 0;
    }
}

/// Fair bounded peer queue. A peer is queued once until its current session is
/// completed; each completion rotates the queue so one large author log cannot
/// starve the remaining peers.
#[derive(Debug)]
pub(crate) struct ReplicaScheduler {
    peers: VecDeque<PublicIdentity>,
    active: usize,
}

impl ReplicaScheduler {
    pub(crate) fn new(peers: impl IntoIterator<Item = PublicIdentity>) -> Self {
        let mut queue = VecDeque::new();
        for peer in peers {
            if !queue.contains(&peer) {
                queue.push_back(peer);
            }
        }
        Self {
            peers: queue,
            active: 0,
        }
    }

    pub(crate) fn next(&mut self) -> Option<PublicIdentity> {
        if self.active >= MAX_OUTBOUND_PEERS {
            return None;
        }
        let peer = self.peers.pop_front()?;
        self.active += 1;
        Some(peer)
    }

    pub(crate) fn complete(&mut self, peer: PublicIdentity, retry: bool) {
        self.active = self.active.saturating_sub(1);
        if retry || !self.peers.contains(&peer) {
            self.peers.push_back(peer);
        }
    }

    pub(crate) fn active(&self) -> usize {
        self.active
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh_base::SecretKey;

    fn identity(seed: u8) -> PublicIdentity {
        PublicIdentity::from_bytes(*SecretKey::from_bytes(&[seed; 32]).public().as_bytes()).unwrap()
    }

    #[test]
    fn sessions_stop_at_operation_byte_and_time_limits() {
        let now = Instant::now();
        let mut budget = SessionBudget::new(now);
        assert!(budget.admit(1, now));
        assert!(!budget.admit(MAX_SESSION_BYTES, now));
        let mut expired = SessionBudget::new(now);
        assert!(!expired.admit(1, now + MAX_SESSION_DURATION));
    }

    #[test]
    fn scheduler_caps_four_dials_and_rotates_fairly() {
        let identities = (1..=6).map(identity).collect::<Vec<_>>();
        let mut scheduler = ReplicaScheduler::new(identities.clone());
        let first = (0..MAX_OUTBOUND_PEERS)
            .map(|_| scheduler.next().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(scheduler.active(), MAX_OUTBOUND_PEERS);
        assert!(scheduler.next().is_none());
        scheduler.complete(first[0], false);
        assert_eq!(scheduler.next(), Some(identities[4]));
    }

    #[test]
    fn twenty_five_members_rotate_without_exceeding_four_active_sessions() {
        let identities = (1..=25).map(identity).collect::<Vec<_>>();
        let mut scheduler = ReplicaScheduler::new(identities.clone());
        let mut seen = Vec::new();
        for _ in 0..identities.len() {
            let peer = scheduler.next().expect("a queued peer");
            seen.push(peer);
            scheduler.complete(peer, false);
        }
        assert_eq!(seen.len(), identities.len());
        assert_eq!(scheduler.active(), 0);
        assert!(
            seen.iter()
                .all(|peer| seen.iter().filter(|other| *other == peer).count() == 1)
        );
    }

    #[test]
    fn retry_backoff_is_capped_and_resets_after_success() {
        let mut retry = RetryState::new();
        for _ in 0..20 {
            assert!(retry.next_delay() <= Duration::from_secs(15));
        }
        retry.succeeded();
        assert_eq!(retry.next_delay(), Duration::from_millis(250));
    }
}
