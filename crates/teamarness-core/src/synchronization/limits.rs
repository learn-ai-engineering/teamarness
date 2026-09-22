//! Every synchronization bound, in one place.
//!
//! A bound that lives here is one number with one meaning, so a change is a
//! change to the whole system rather than to one call site. Tests inject small
//! values through [`SessionLimits`] the same way they inject `ParkingLimits`.

use std::time::Duration;

/// How far beyond the highest local causal height a received record's order key
/// may reach.
///
/// A Member that has been writing while offline is legitimately ahead, so the
/// window is wide. Beyond it, the height is a claim this Installation cannot
/// check, and the operation is rejected rather than allowed to push every later
/// record to the end of the order.
pub(crate) const MAX_HEIGHT_ADVANCE: u64 = 1 << 20;

/// A version-2 protection control is exactly 354 bytes; the bound leaves room
/// without admitting an unbounded handshake.
pub(crate) const MAX_PROTECTION_CONTROL_BYTES: usize = 4096;

/// Every bound one package synchronization session obeys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SessionLimits {
    /// The first frame a responder reads, before any authorization.
    pub(crate) hello_bytes: usize,
    /// How long a responder waits for that first frame before dropping a
    /// silent peer, so it cannot hold an inbound slot for the whole session.
    pub(crate) hello_timeout: Duration,
    /// `Welcome`, `Ready` and `Refused`: two protection controls plus fields.
    pub(crate) handshake_frame_bytes: usize,
    /// Every frame after the handshake. The largest honest frame is one
    /// operation: a sealed record plus a p2panda header.
    pub(crate) frame_bytes: usize,
    /// Operations one session accepts, and sends, in each direction.
    pub(crate) operations: usize,
    /// Operation bytes one session accepts, and sends, in each direction.
    pub(crate) operation_bytes: usize,
    /// Non-operation frames per direction. An honest session sends about six.
    pub(crate) control_frames: usize,
    /// Remote operations applied under one lock hold.
    pub(crate) apply_batch: usize,
    /// Rows read from storage under one lock hold while sending.
    pub(crate) view_page_rows: usize,
    /// Concurrent inbound sessions across all peers.
    pub(crate) inbound_sessions: usize,
    /// Concurrent inbound sessions from one peer. The scheduler dials a peer
    /// once at a time; two tolerates a crossing retry.
    pub(crate) inbound_sessions_per_peer: usize,
    /// How long one whole session may run.
    pub(crate) session_timeout: Duration,
    /// How long an initiator waits for the connection itself, so a peer that
    /// has gone away is discovered well inside the session budget.
    pub(crate) connect_timeout: Duration,
}

impl SessionLimits {
    pub(crate) const CURRENT: Self = Self {
        hello_bytes: 1024,
        hello_timeout: Duration::from_secs(5),
        handshake_frame_bytes: 16 * 1024,
        frame_bytes: 1024 * 1024,
        operations: 2_048,
        operation_bytes: 8 * 1024 * 1024,
        control_frames: 64,
        apply_batch: 16,
        view_page_rows: 256,
        inbound_sessions: 8,
        inbound_sessions_per_peer: 2,
        session_timeout: Duration::from_secs(30),
        connect_timeout: Duration::from_secs(15),
    };

    /// The LogSync broadcast capacity.
    ///
    /// Lag is impossible by construction: the reader refuses operation
    /// `operations + 1`, and LogSync emits at most one non-operation event.
    pub(crate) const fn event_capacity(&self) -> usize {
        self.operations + 16
    }
}

/// Every bound the synchronization scheduler obeys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SchedulerLimits {
    /// Sessions in flight across all peers.
    pub(crate) outbound_sessions: usize,
    /// How many of those slots peers that keep failing may occupy.
    pub(crate) failing_peer_slots: usize,
    /// Consecutive failures after which a peer counts as failing.
    pub(crate) failing_threshold: u32,
    pub(crate) retry_base: Duration,
    pub(crate) retry_cap: Duration,
    pub(crate) jitter_ratio_percent: u32,
    /// How long a (scope, peer) pair may go unattempted before it is due again.
    pub(crate) record_idle: Duration,
    /// The same, for canonical membership.
    pub(crate) membership_idle: Duration,
    /// The in-memory job set; the rest are picked up by the next durable load.
    pub(crate) max_scheduled_jobs: usize,
    /// How long shutdown waits for sessions before abandoning them.
    pub(crate) shutdown_grace: Duration,
}

impl SchedulerLimits {
    pub(crate) const CURRENT: Self = Self {
        outbound_sessions: 4,
        failing_peer_slots: 2,
        failing_threshold: 2,
        retry_base: Duration::from_millis(500),
        retry_cap: Duration::from_secs(60),
        jitter_ratio_percent: 20,
        record_idle: Duration::from_secs(60),
        membership_idle: Duration::from_secs(30),
        max_scheduled_jobs: 4_096,
        shutdown_grace: Duration::from_secs(2),
    };
}

/// A peer asked for more than its session budget allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BudgetExceeded;

/// What one session may still receive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReceiveBudget {
    operations: usize,
    bytes: usize,
    control: usize,
}

impl ReceiveBudget {
    pub(crate) const fn new(limits: &SessionLimits) -> Self {
        Self {
            operations: limits.operations,
            bytes: limits.operation_bytes,
            control: limits.control_frames,
        }
    }

    pub(crate) fn charge_operation(&mut self, bytes: usize) -> Result<(), BudgetExceeded> {
        if self.operations == 0 || bytes > self.bytes {
            return Err(BudgetExceeded);
        }
        self.operations -= 1;
        self.bytes -= bytes;
        Ok(())
    }

    pub(crate) fn charge_control(&mut self) -> Result<(), BudgetExceeded> {
        if self.control == 0 {
            return Err(BudgetExceeded);
        }
        self.control -= 1;
        Ok(())
    }
}

/// What one session may still send.
///
/// The budget reserves a prefix of a range at a time, so a sender stops at a
/// record boundary and a later session continues from there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SendBudget {
    operations: usize,
    bytes: usize,
    truncated: bool,
}

impl SendBudget {
    pub(crate) const fn new(limits: &SessionLimits) -> Self {
        Self {
            operations: limits.operations,
            bytes: limits.operation_bytes,
            truncated: false,
        }
    }

    pub(crate) const fn remaining_operations(&self) -> usize {
        self.operations
    }

    pub(crate) const fn truncated(&self) -> bool {
        self.truncated
    }

    /// Reserves the longest prefix of `sizes` that fits, in sequence order.
    ///
    /// Returns the sequence the reservation reaches, or `None` when nothing
    /// fits. A prefix shorter than `sizes` marks the session as truncated.
    pub(crate) fn reserve_range(&mut self, sizes: &[(u32, usize)]) -> Option<u32> {
        let mut planned = None;
        for (sequence, bytes) in sizes {
            if self.operations == 0 || *bytes > self.bytes {
                self.truncated = true;
                return planned;
            }
            self.operations -= 1;
            self.bytes -= *bytes;
            planned = Some(*sequence);
        }
        planned
    }

    /// Records that a range was cut for a reason other than this budget, such
    /// as a row limit that stopped short of the author's head.
    pub(crate) const fn note_truncated(&mut self) {
        self.truncated = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receive_budget_charges_operations_bytes_and_control_frames() {
        let limits = SessionLimits {
            operations: 2,
            operation_bytes: 10,
            control_frames: 1,
            ..SessionLimits::CURRENT
        };
        let mut budget = ReceiveBudget::new(&limits);
        assert_eq!(budget.charge_operation(4), Ok(()));
        assert_eq!(budget.charge_operation(4), Ok(()));
        assert_eq!(budget.charge_operation(1), Err(BudgetExceeded));
        assert_eq!(budget.charge_control(), Ok(()));
        assert_eq!(budget.charge_control(), Err(BudgetExceeded));

        let mut bytes_bound = ReceiveBudget::new(&limits);
        assert_eq!(bytes_bound.charge_operation(11), Err(BudgetExceeded));
    }
}
