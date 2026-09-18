//! The log one session serves, read lazily from storage.
//!
//! A session never loads a whole scope. It learns the author list and their
//! heights once, then plans what fits in its send budget and reads only that,
//! in pages, one lock hold at a time. A session that stops at its budget says
//! so, and a later session continues from where it stopped.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use p2panda_core::{Hash, Operation, VerifyingKey};
use p2panda_store::logs::LogStore;

use super::limits::{SendBudget, SessionLimits};
use super::network_handler::{RangeLoadError, SessionSeed, SyncReceiver};
use crate::{PublicIdentity, WorkspaceId};

/// Who this session serves, and which log it serves.
#[derive(Debug, Clone)]
pub(crate) struct ViewContext {
    pub(crate) peer: PublicIdentity,
    pub(crate) workspace: WorkspaceId,
    pub(crate) package: String,
    pub(crate) log_id: [u8; 32],
}

/// What this session has already committed to send, per author.
#[derive(Debug)]
struct ViewPlan {
    budget: SendBudget,
    planned: BTreeMap<VerifyingKey, u32>,
}

/// The error type LogSync sees when a served range cannot be produced.
#[derive(Debug, Clone)]
pub(crate) struct LogViewError(String);

impl std::fmt::Display for LogViewError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for LogViewError {}

/// A [`LogStore`] backed by storage, bounded by one session's send budget.
#[derive(Clone)]
pub(crate) struct SqliteLogView {
    receiver: Arc<dyn SyncReceiver>,
    context: Arc<ViewContext>,
    seed: Arc<SessionSeed>,
    plan: Arc<Mutex<ViewPlan>>,
    limits: SessionLimits,
}

impl std::fmt::Debug for SqliteLogView {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqliteLogView")
            .finish_non_exhaustive()
    }
}

impl SqliteLogView {
    pub(crate) fn new(
        receiver: Arc<dyn SyncReceiver>,
        context: ViewContext,
        seed: SessionSeed,
        limits: SessionLimits,
    ) -> Self {
        Self {
            receiver,
            context: Arc::new(context),
            seed: Arc::new(seed),
            plan: Arc::new(Mutex::new(ViewPlan {
                budget: SendBudget::new(&limits),
                planned: BTreeMap::new(),
            })),
            limits,
        }
    }

    /// Whether this session stopped short of everything it holds.
    pub(crate) fn truncated(&self) -> bool {
        self.plan
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .budget
            .truncated()
    }

    fn identity(author: &VerifyingKey) -> Option<PublicIdentity> {
        PublicIdentity::from_bytes(*author.as_bytes()).ok()
    }

    fn serves(&self, log_id: &[u8; 32]) -> bool {
        *log_id == self.context.log_id
    }

    /// Plans the prefix of `after+1..=until` this session will send, reserving
    /// it from the shared budget.
    ///
    /// The lock over the plan is never held across an await.
    async fn plan_range(
        &self,
        author: &VerifyingKey,
        after: Option<u32>,
        until: Option<u32>,
    ) -> Result<Option<(u32, u32, u32)>, LogViewError> {
        let Some(identity) = Self::identity(author) else {
            return Ok(None);
        };
        let remaining = {
            let plan = self.plan.lock().unwrap_or_else(PoisonError::into_inner);
            plan.budget.remaining_operations()
        };
        if remaining == 0 {
            self.plan
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .budget
                .note_truncated();
            return Ok(None);
        }
        let head = self.seed.heights.get(&identity).copied();
        let sizes = self
            .range_sizes(identity, after, until, remaining)
            .await?
            .unwrap_or_default();
        if sizes.is_empty() {
            return Ok(None);
        }
        let mut plan = self.plan.lock().unwrap_or_else(PoisonError::into_inner);
        // A range the row limit already cut short is a truncation of its own,
        // even when every row it returned fits the budget.
        if let (Some(head), Some((last, _))) = (head, sizes.last())
            && sizes.len() == remaining
            && *last < head
        {
            plan.budget.note_truncated();
        }
        let Some(planned_until) = plan.budget.reserve_range(&sizes) else {
            return Ok(None);
        };
        plan.planned.insert(*author, planned_until);
        let counted = sizes
            .iter()
            .take_while(|(sequence, _)| *sequence <= planned_until)
            .collect::<Vec<_>>();
        let count = u32::try_from(counted.len()).unwrap_or(u32::MAX);
        let bytes = counted
            .iter()
            .map(|(_, size)| *size)
            .sum::<usize>()
            .try_into()
            .unwrap_or(u32::MAX);
        Ok(Some((count, bytes, planned_until)))
    }

    async fn range_sizes(
        &self,
        author: PublicIdentity,
        after: Option<u32>,
        until: Option<u32>,
        limit: usize,
    ) -> Result<Option<Vec<(u32, usize)>>, LogViewError> {
        let context = Arc::clone(&self.context);
        let sizes = crate::blocking::receiver_call(&self.receiver, move |receiver| {
            receiver.range_sizes(
                context.peer,
                context.workspace,
                &context.package,
                author,
                after,
                until,
                limit,
            )
        })
        .await
        .map_err(|_| LogViewError("runtime state is unavailable".to_owned()))?;
        match sizes {
            Ok(sizes) => Ok(Some(sizes)),
            Err(()) => Ok(None),
        }
    }

    async fn load_pages(
        &self,
        author: PublicIdentity,
        from: u32,
        until: u32,
    ) -> Result<Vec<(Operation<()>, Vec<u8>)>, LogViewError> {
        let mut entries = Vec::new();
        let mut next = from;
        loop {
            let context = Arc::clone(&self.context);
            let rows = self.limits.view_page_rows;
            let page = crate::blocking::receiver_call(&self.receiver, move |receiver| {
                receiver.load_range_page(
                    context.peer,
                    context.workspace,
                    &context.package,
                    author,
                    next,
                    until,
                    rows,
                )
            })
            .await
            .map_err(|_| LogViewError("runtime state is unavailable".to_owned()))?;
            let page = page.map_err(|error| match error {
                RangeLoadError::ScopeCorrupt => LogViewError("scope corrupt".to_owned()),
                RangeLoadError::Unavailable => LogViewError("served log is unavailable".to_owned()),
            })?;
            let Some(last) = page.last().map(|(operation, _)| operation.header.seq_num) else {
                break;
            };
            entries.extend(page);
            if last >= until {
                break;
            }
            next = last.saturating_add(1);
        }
        Ok(entries)
    }
}

impl LogStore<Operation<()>, VerifyingKey, [u8; 32], u32, Hash> for SqliteLogView {
    type Error = LogViewError;

    async fn get_latest_entry(
        &self,
        author: &VerifyingKey,
        log_id: &[u8; 32],
    ) -> Result<Option<Operation<()>>, Self::Error> {
        if !self.serves(log_id) {
            return Ok(None);
        }
        let Some(identity) = Self::identity(author) else {
            return Ok(None);
        };
        let Some(height) = self.seed.heights.get(&identity).copied() else {
            return Ok(None);
        };
        Ok(self
            .load_pages(identity, height, height)
            .await?
            .into_iter()
            .next()
            .map(|(operation, _)| operation))
    }

    async fn get_latest_entry_tx(
        &self,
        author: &VerifyingKey,
        log_id: &[u8; 32],
    ) -> Result<Option<Operation<()>>, Self::Error> {
        self.get_latest_entry(author, log_id).await
    }

    async fn get_log_heights(
        &self,
        author: &VerifyingKey,
        logs: &[[u8; 32]],
    ) -> Result<Option<BTreeMap<[u8; 32], u32>>, Self::Error> {
        let Some(identity) = Self::identity(author) else {
            return Ok(None);
        };
        let Some(height) = self.seed.heights.get(&identity).copied() else {
            return Ok(None);
        };
        let heights = logs
            .iter()
            .filter(|log_id| self.serves(log_id))
            .map(|log_id| (*log_id, height))
            .collect::<BTreeMap<_, _>>();
        if heights.is_empty() {
            Ok(None)
        } else {
            Ok(Some(heights))
        }
    }

    async fn get_log_size(
        &self,
        author: &VerifyingKey,
        log_id: &[u8; 32],
        after: Option<u32>,
        until: Option<u32>,
    ) -> Result<Option<(u32, u32)>, Self::Error> {
        if !self.serves(log_id) {
            return Ok(None);
        }
        Ok(self
            .plan_range(author, after, until)
            .await?
            .map(|(count, bytes, _)| (count, bytes)))
    }

    async fn get_log_entries(
        &self,
        author: &VerifyingKey,
        log_id: &[u8; 32],
        after: Option<u32>,
        until: Option<u32>,
    ) -> Result<Option<Vec<(Operation<()>, Vec<u8>)>>, Self::Error> {
        if !self.serves(log_id) {
            return Ok(None);
        }
        let planned = {
            let plan = self.plan.lock().unwrap_or_else(PoisonError::into_inner);
            plan.planned.get(author).copied()
        };
        let planned = match planned {
            Some(planned) => Some(planned),
            // LogSync may ask for a range it never sized; the same plan then
            // decides what this session will send.
            None => self
                .plan_range(author, after, until)
                .await?
                .map(|(_, _, planned)| planned),
        };
        let Some(planned) = planned else {
            return Ok(None);
        };
        let Some(identity) = Self::identity(author) else {
            return Ok(None);
        };
        let from = after.map_or(0, |value| value.saturating_add(1));
        if from > planned {
            return Ok(Some(Vec::new()));
        }
        Ok(Some(self.load_pages(identity, from, planned).await?))
    }

    async fn prune_entries(
        &self,
        _author: &VerifyingKey,
        _log_id: &[u8; 32],
        _until: &u32,
    ) -> Result<u64, Self::Error> {
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synchronization::test_io::{MemoryLogStore, RecordingReceiver};

    const LOG: [u8; 32] = [4; 32];

    fn context() -> ViewContext {
        ViewContext {
            peer: crate::synchronization::network_handler::tests::test_peer(9),
            workspace: WorkspaceId::from_bytes([4; 16]),
            package: "teamarness.test".to_owned(),
            log_id: LOG,
        }
    }

    fn view(receiver: Arc<RecordingReceiver>, limits: SessionLimits) -> SqliteLogView {
        let seed = receiver.seed();
        SqliteLogView::new(receiver, context(), seed, limits)
    }

    fn author(seed: u8) -> VerifyingKey {
        p2panda_core::SigningKey::from_bytes(&[seed; 32]).verifying_key()
    }

    #[tokio::test]
    async fn size_and_entries_agree_after_truncation() {
        let limits = SessionLimits {
            operations: 5,
            ..SessionLimits::CURRENT
        };
        let store = MemoryLogStore::with_logs(&[(1, 4), (2, 4)]);
        let receiver = Arc::new(RecordingReceiver::new(store));
        let view = view(Arc::clone(&receiver), limits);

        let first = view
            .get_log_size(&author(1), &LOG, None, None)
            .await
            .unwrap()
            .unwrap();
        let second = view
            .get_log_size(&author(2), &LOG, None, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.0, 4);
        assert_eq!(second.0, 1, "only one operation of the second author fits");

        let served_first = view
            .get_log_entries(&author(1), &LOG, None, None)
            .await
            .unwrap()
            .unwrap();
        let served_second = view
            .get_log_entries(&author(2), &LOG, None, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(served_first.len(), 4);
        assert_eq!(served_second.len(), 1);
        assert!(view.truncated(), "the session has more to send");
    }

    #[tokio::test]
    async fn entries_are_loaded_in_pages() {
        let limits = SessionLimits {
            view_page_rows: 3,
            ..SessionLimits::CURRENT
        };
        let store = MemoryLogStore::with_logs(&[(1, 7)]);
        let receiver = Arc::new(RecordingReceiver::new(store));
        let view = view(Arc::clone(&receiver), limits);

        let served = view
            .get_log_entries(&author(1), &LOG, None, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(served.len(), 7);
        assert_eq!(
            receiver.pages(),
            vec![3, 3, 1],
            "the range is read in pages, not in one read"
        );
        assert!(!view.truncated());
    }

    #[tokio::test]
    async fn nothing_is_served_for_another_log() {
        let store = MemoryLogStore::with_logs(&[(1, 2)]);
        let receiver = Arc::new(RecordingReceiver::new(store));
        let view = view(Arc::clone(&receiver), SessionLimits::CURRENT);
        assert_eq!(
            view.get_log_entries(&author(1), &[9; 32], None, None)
                .await
                .unwrap(),
            None
        );
        assert_eq!(receiver.pages(), Vec::<usize>::new());
    }
}
