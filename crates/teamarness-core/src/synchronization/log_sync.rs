use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use crate::PublicIdentity;

pub(crate) const MAX_LOG_OPERATIONS: usize = 4_096;
pub(crate) const MAX_LOG_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_LOG_SESSION: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LogRange {
    pub(crate) author: PublicIdentity,
    pub(crate) from: u64,
    pub(crate) to: u64,
}

/// A finite, fair view passed to the P2Panda LogSync adapter. The adapter may
/// only advertise contiguous author ranges selected by this view; it cannot
/// accidentally expose an unbounded Core log.
#[derive(Debug)]
pub(crate) struct BoundedLogSyncView {
    true_heights: BTreeMap<PublicIdentity, u64>,
    cursor: VecDeque<PublicIdentity>,
    operations: usize,
    bytes: usize,
    deadline: Instant,
}

impl BoundedLogSyncView {
    pub(crate) fn new(true_heights: BTreeMap<PublicIdentity, u64>, now: Instant) -> Self {
        let cursor = true_heights.keys().copied().collect();
        Self {
            true_heights,
            cursor,
            operations: MAX_LOG_OPERATIONS,
            bytes: MAX_LOG_BYTES,
            deadline: now + MAX_LOG_SESSION,
        }
    }

    pub(crate) fn next_range(
        &mut self,
        remote_heights: &BTreeMap<PublicIdentity, u64>,
    ) -> Option<LogRange> {
        let count = self.cursor.len();
        for _ in 0..count {
            let author = self.cursor.pop_front()?;
            self.cursor.push_back(author);
            let local = *self.true_heights.get(&author)?;
            let remote = remote_heights.get(&author).copied().unwrap_or(0);
            if local > remote
                && self.operations > 0
                && self.bytes > 0
                && Instant::now() < self.deadline
            {
                return Some(LogRange {
                    author,
                    from: remote + 1,
                    to: local,
                });
            }
        }
        None
    }

    pub(crate) fn admit_operation(&mut self, bytes: usize, now: Instant) -> bool {
        if now >= self.deadline || self.operations == 0 || bytes > self.bytes {
            return false;
        }
        self.operations -= 1;
        self.bytes -= bytes;
        true
    }

    pub(crate) fn remaining(&self) -> (usize, usize) {
        (self.operations, self.bytes)
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
    fn view_advertises_contiguous_ranges_and_rotates_authors() {
        let now = Instant::now();
        let mut heights = BTreeMap::new();
        heights.insert(identity(1), 10);
        heights.insert(identity(2), 20);
        let mut view = BoundedLogSyncView::new(heights, now);
        let remote = BTreeMap::from([(identity(1), 5), (identity(2), 10)]);
        let first = view.next_range(&remote).unwrap();
        let second = view.next_range(&remote).unwrap();
        assert_eq!(
            [(first.from, first.to), (second.from, second.to)]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            [(6, 10), (11, 20)].into_iter().collect()
        );
        assert_ne!(first.author, second.author);
    }

    #[test]
    fn view_rejects_operations_after_any_session_bound() {
        let now = Instant::now();
        let mut view = BoundedLogSyncView::new(BTreeMap::from([(identity(1), 1)]), now);
        assert!(view.admit_operation(1, now));
        assert!(!view.admit_operation(MAX_LOG_BYTES, now));
        assert!(!view.admit_operation(1, now + MAX_LOG_SESSION));
    }
}
