//! What recent sessions say about reaching each peer, and the availability a
//! subscription derives from it.
//!
//! The registry keeps its own lock, separate from the runtime state, and never
//! holds it across an await. A change bumps the affected scope's head revision,
//! so a Package that is waiting is told without polling.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::record_subscription::{
    RecordAvailability, SharedRecordHeads, SyncAvailability, SyncWarning, lookup_head,
};
pub(crate) use super::scheduler::PeerOutcome;
use crate::PublicIdentity;

/// What one scope's recent sessions reported.
#[derive(Debug, Default, Clone)]
pub(crate) struct ScopeHealth {
    /// The last outcome of a session with each peer. A peer with no entry has
    /// not been attempted since this process started.
    pub(crate) last_outcome: BTreeMap<PublicIdentity, PeerOutcome>,
    /// Operations announced by running sessions that have not been applied yet.
    pub(crate) incoming_remaining: u64,
    /// The last completed session reported that one side still has more to
    /// send, so the scope is not yet reconciled.
    pub(crate) peer_truncated: bool,
}

/// Session health for every scope of one Core runtime.
pub(crate) struct SyncHealthRegistry {
    scopes: Mutex<BTreeMap<[u8; 32], ScopeHealth>>,
    heads: SharedRecordHeads,
}

impl std::fmt::Debug for SyncHealthRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SyncHealthRegistry")
            .finish_non_exhaustive()
    }
}

impl SyncHealthRegistry {
    pub(crate) fn new(heads: SharedRecordHeads) -> Arc<Self> {
        Arc::new(Self {
            scopes: Mutex::new(BTreeMap::new()),
            heads,
        })
    }

    /// Asks every subscriber of one scope to recompute what it reports.
    pub(crate) fn scope_changed(&self, scope: [u8; 32]) {
        if let Some(head) = lookup_head(&self.heads, scope) {
            head.bump_revision();
        }
    }

    /// Records what one session said about reaching a peer.
    ///
    /// `None` removes the peer's entry: a cancelled session and one the local
    /// gate refused say nothing about the peer.
    pub(crate) fn record_outcome(
        &self,
        scope: [u8; 32],
        peer: PublicIdentity,
        outcome: Option<PeerOutcome>,
        peer_truncated: Option<bool>,
    ) {
        {
            let Ok(mut scopes) = self.scopes.lock() else {
                return;
            };
            let health = scopes.entry(scope).or_default();
            match outcome {
                Some(outcome) => {
                    health.last_outcome.insert(peer, outcome);
                }
                None => {
                    health.last_outcome.remove(&peer);
                }
            }
            if let Some(truncated) = peer_truncated {
                health.peer_truncated = truncated;
            }
        }
        self.scope_changed(scope);
    }

    /// Opens a reporter for one running session's incoming operations.
    pub(crate) fn session_incoming(self: &Arc<Self>, scope: [u8; 32]) -> HealthReporter {
        HealthReporter {
            registry: Arc::clone(self),
            scope,
            outstanding: AtomicU64::new(0),
        }
    }

    pub(crate) fn snapshot(&self, scope: [u8; 32]) -> ScopeHealth {
        self.scopes
            .lock()
            .ok()
            .and_then(|scopes| scopes.get(&scope).cloned())
            .unwrap_or_default()
    }

    fn add_incoming(&self, scope: [u8; 32], amount: u64) {
        if amount == 0 {
            return;
        }
        if let Ok(mut scopes) = self.scopes.lock() {
            let health = scopes.entry(scope).or_default();
            health.incoming_remaining = health.incoming_remaining.saturating_add(amount);
        }
        self.scope_changed(scope);
    }

    fn subtract_incoming(&self, scope: [u8; 32], amount: u64) {
        if amount == 0 {
            return;
        }
        if let Ok(mut scopes) = self.scopes.lock() {
            let health = scopes.entry(scope).or_default();
            health.incoming_remaining = health.incoming_remaining.saturating_sub(amount);
        }
        self.scope_changed(scope);
    }
}

/// One session's share of a scope's incoming count.
///
/// Whatever a session announced but never applied is subtracted when the
/// session ends, so a failed session leaves no phantom backlog behind.
pub(crate) struct HealthReporter {
    registry: Arc<SyncHealthRegistry>,
    scope: [u8; 32],
    outstanding: AtomicU64,
}

impl std::fmt::Debug for HealthReporter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HealthReporter")
            .finish_non_exhaustive()
    }
}

impl HealthReporter {
    /// The peer announced `count` operations it intends to send.
    pub(crate) fn announced(&self, count: u64) {
        self.outstanding.fetch_add(count, Ordering::Relaxed);
        self.registry.add_incoming(self.scope, count);
    }

    /// `count` of the announced operations have been applied.
    pub(crate) fn applied(&self, count: u64) {
        let charged = self
            .outstanding
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_sub(count))
            })
            .map_or(0, |before| before.min(count));
        self.registry.subtract_incoming(self.scope, charged);
    }
}

impl Drop for HealthReporter {
    fn drop(&mut self) {
        let remainder = self.outstanding.swap(0, Ordering::Relaxed);
        self.registry.subtract_incoming(self.scope, remainder);
    }
}

/// Everything the availability table reads.
#[derive(Debug, Clone)]
pub(crate) struct AvailabilityInputs {
    pub(crate) scope_quarantined: bool,
    pub(crate) locally_revoked: bool,
    pub(crate) scope_exists: bool,
    pub(crate) undelivered: u64,
    pub(crate) parked: u64,
    pub(crate) peers: BTreeSet<PublicIdentity>,
    pub(crate) health: ScopeHealth,
    pub(crate) record_quarantined_seen: bool,
}

/// The availability table, first matching row wins.
pub(crate) fn availability(inputs: &AvailabilityInputs) -> RecordAvailability {
    let parked = inputs.parked;
    let undelivered = inputs.undelivered;
    let incoming = inputs.health.incoming_remaining;
    let (availability, pending, mut warnings) = if inputs.scope_quarantined {
        // 1. The scope is isolated by a local integrity failure.
        (
            SyncAvailability::Offline,
            undelivered.saturating_add(parked),
            vec![SyncWarning::ScopeQuarantined],
        )
    } else if inputs.locally_revoked {
        // 2. This Installation may no longer participate.
        (
            SyncAvailability::Offline,
            undelivered.saturating_add(parked),
            vec![SyncWarning::MembershipRevoked],
        )
    } else if !inputs.scope_exists {
        // 3. No content key has arrived, so no history can be read.
        (
            SyncAvailability::WaitingForHistoryAccess,
            parked,
            Vec::new(),
        )
    } else if undelivered > 0 {
        // 4. Records are committed and not yet delivered.
        (
            SyncAvailability::CatchingUp,
            undelivered.saturating_add(parked).saturating_add(incoming),
            Vec::new(),
        )
    } else if incoming > 0 || inputs.health.peer_truncated {
        // 5. A session is running, or the last one had more to send.
        (
            SyncAvailability::CatchingUp,
            parked.saturating_add(incoming),
            Vec::new(),
        )
    } else if inputs.peers.is_empty() {
        // 6. There is nobody to reach.
        (SyncAvailability::Current, parked, Vec::new())
    } else if inputs
        .peers
        .iter()
        .any(|peer| inputs.health.last_outcome.get(peer) == Some(&PeerOutcome::Reachable))
    {
        // 7. At least one peer answered.
        (SyncAvailability::Current, parked, Vec::new())
    } else if inputs
        .peers
        .iter()
        .any(|peer| !inputs.health.last_outcome.contains_key(peer))
    {
        // 8. A peer has not been attempted yet.
        (SyncAvailability::CatchingUp, parked, Vec::new())
    } else {
        // 9 and 10. Everyone was attempted and nobody answered. The
        // vocabulary does not distinguish unreachable from failed; the
        // session health does.
        (SyncAvailability::Offline, parked, Vec::new())
    };
    if inputs.record_quarantined_seen {
        warnings.push(SyncWarning::RecordQuarantined);
    }
    RecordAvailability {
        availability,
        pending_records: pending,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(byte: u8) -> PublicIdentity {
        PublicIdentity::from_bytes(
            *iroh_base::SecretKey::from_bytes(&[byte; 32])
                .public()
                .as_bytes(),
        )
        .expect("a generated identity is valid")
    }

    fn inputs() -> AvailabilityInputs {
        AvailabilityInputs {
            scope_quarantined: false,
            locally_revoked: false,
            scope_exists: true,
            undelivered: 0,
            parked: 0,
            peers: BTreeSet::new(),
            health: ScopeHealth::default(),
            record_quarantined_seen: false,
        }
    }

    #[test]
    fn availability_table() {
        // 1. A quarantined scope.
        let mut row = inputs();
        row.scope_quarantined = true;
        row.undelivered = 2;
        row.parked = 3;
        let reported = availability(&row);
        assert_eq!(reported.availability, SyncAvailability::Offline);
        assert_eq!(reported.pending_records, 5);
        assert_eq!(reported.warnings, vec![SyncWarning::ScopeQuarantined]);

        // 2. A locally revoked Installation.
        let mut row = inputs();
        row.locally_revoked = true;
        row.parked = 1;
        let reported = availability(&row);
        assert_eq!(reported.availability, SyncAvailability::Offline);
        assert_eq!(reported.pending_records, 1);
        assert_eq!(reported.warnings, vec![SyncWarning::MembershipRevoked]);

        // 3. No content key yet.
        let mut row = inputs();
        row.scope_exists = false;
        row.parked = 4;
        let reported = availability(&row);
        assert_eq!(
            reported.availability,
            SyncAvailability::WaitingForHistoryAccess
        );
        assert_eq!(reported.pending_records, 4);
        assert!(reported.warnings.is_empty());

        // 4. Undelivered records.
        let mut row = inputs();
        row.undelivered = 7;
        row.parked = 1;
        row.health.incoming_remaining = 2;
        let reported = availability(&row);
        assert_eq!(reported.availability, SyncAvailability::CatchingUp);
        assert_eq!(reported.pending_records, 10);

        // 5. A running session, and a truncated one.
        let mut row = inputs();
        row.health.incoming_remaining = 5;
        assert_eq!(
            availability(&row).availability,
            SyncAvailability::CatchingUp
        );
        assert_eq!(availability(&row).pending_records, 5);
        let mut row = inputs();
        row.health.peer_truncated = true;
        assert_eq!(
            availability(&row).availability,
            SyncAvailability::CatchingUp
        );

        // 6. A solo Workspace.
        let reported = availability(&inputs());
        assert_eq!(reported.availability, SyncAvailability::Current);
        assert_eq!(reported.pending_records, 0);

        // 7. One peer answered.
        let mut row = inputs();
        row.peers = [peer(1), peer(2)].into_iter().collect();
        row.health
            .last_outcome
            .insert(peer(1), PeerOutcome::Unreachable);
        row.health
            .last_outcome
            .insert(peer(2), PeerOutcome::Reachable);
        assert_eq!(availability(&row).availability, SyncAvailability::Current);

        // 8. A peer that has not been attempted yet.
        let mut row = inputs();
        row.peers = [peer(1), peer(2)].into_iter().collect();
        row.health
            .last_outcome
            .insert(peer(1), PeerOutcome::Unreachable);
        assert_eq!(
            availability(&row).availability,
            SyncAvailability::CatchingUp
        );

        // 9. Every peer unreachable.
        let mut row = inputs();
        row.peers = [peer(1)].into_iter().collect();
        row.health
            .last_outcome
            .insert(peer(1), PeerOutcome::Unreachable);
        assert_eq!(availability(&row).availability, SyncAvailability::Offline);

        // 10. Attempted, none reachable, at least one failed.
        let mut row = inputs();
        row.peers = [peer(1), peer(2)].into_iter().collect();
        row.health
            .last_outcome
            .insert(peer(1), PeerOutcome::Unreachable);
        row.health.last_outcome.insert(peer(2), PeerOutcome::Failed);
        assert_eq!(availability(&row).availability, SyncAvailability::Offline);
    }

    #[test]
    fn first_matching_row_wins() {
        let mut row = inputs();
        row.scope_quarantined = true;
        row.undelivered = 4;
        row.parked = 1;
        let reported = availability(&row);
        assert_eq!(
            reported.availability,
            SyncAvailability::Offline,
            "quarantine wins over undelivered records"
        );
        assert_eq!(reported.pending_records, 5);
    }

    #[test]
    fn the_vocabulary_has_exactly_four_plain_values() {
        let names = [
            SyncAvailability::WaitingForHistoryAccess.as_str(),
            SyncAvailability::Offline.as_str(),
            SyncAvailability::CatchingUp.as_str(),
            SyncAvailability::Current.as_str(),
        ];
        assert_eq!(
            names,
            [
                "waiting_for_history_access",
                "offline",
                "catching_up",
                "current"
            ]
        );
        assert!(!names.contains(&"degraded"));
    }

    #[test]
    fn a_record_quarantine_adds_its_warning_to_any_row() {
        let mut row = inputs();
        row.record_quarantined_seen = true;
        assert_eq!(
            availability(&row).warnings,
            vec![SyncWarning::RecordQuarantined]
        );
        row.scope_quarantined = true;
        assert_eq!(
            availability(&row).warnings,
            vec![
                SyncWarning::ScopeQuarantined,
                SyncWarning::RecordQuarantined
            ]
        );
    }

    #[test]
    fn incoming_reporter_subtracts_its_remainder_on_drop() {
        let heads: SharedRecordHeads = Arc::default();
        let registry = SyncHealthRegistry::new(heads);
        let scope = [7_u8; 32];
        {
            let reporter = registry.session_incoming(scope);
            reporter.announced(10);
            assert_eq!(registry.snapshot(scope).incoming_remaining, 10);
            reporter.applied(4);
            assert_eq!(registry.snapshot(scope).incoming_remaining, 6);
        }
        assert_eq!(
            registry.snapshot(scope).incoming_remaining,
            0,
            "a session that ends leaves no phantom backlog"
        );
    }
}
