//! Notify-and-reread delivery of committed immutable records.
//!
//! Core holds no per-subscriber queue. Every commit that inserts a delivery row
//! advances the scope's [`ScopeHead`], and a subscription reads pages of at most
//! 32 committed records after its own position. Live delivery therefore cannot
//! diverge from the durable journal, and an idle subscriber costs one position
//! and two notifier handles.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use rusqlite::OptionalExtension;
use tokio::sync::watch;

use crate::WorkspaceId;
use crate::runtime::RuntimeState;

use super::health::{self, AvailabilityInputs, SyncHealthRegistry};
use super::{
    ContentKey, Cursor, P2pandaRecordProtection, RecordEvent, RecordProtection, SealedRecordV1,
    SynchronizationError, fixed, parking, quarantine, record_from_row, scope_digest, storage_error,
};

/// A scope's delivery journal identity and content key.
type ScopeKeys = ([u8; 16], ContentKey);
/// One delivery row: sequence, record identity, order key, author, purpose,
/// signature, sealed envelope, and whether the record is quarantined.
type DeliveryRow = (
    i64,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    String,
    Vec<u8>,
    Vec<u8>,
    bool,
);

/// The largest page a subscriber may ask for, so one lock hold is bounded.
pub(crate) const MAX_PAGE: usize = 32;

/// How many stored records subscriptions have decoded, per scope, since the
/// process began.
///
/// An idle subscription decodes none: nothing is read until a page is asked
/// for. The count is kept per scope so tests that run in parallel, each in its
/// own Workspace, never observe each other's reads.
#[cfg(any(test, feature = "test-hooks"))]
static RECORDS_DECODED: Mutex<BTreeMap<[u8; 32], usize>> = Mutex::new(BTreeMap::new());

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn records_decoded_for_test(scope: [u8; 32]) -> usize {
    RECORDS_DECODED
        .lock()
        .ok()
        .and_then(|counts| counts.get(&scope).copied())
        .unwrap_or(0)
}

#[allow(unused_variables)]
fn note_decoded(scope: [u8; 32]) {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Ok(mut counts) = RECORDS_DECODED.lock() {
        *counts.entry(scope).or_insert(0) += 1;
    }
}

/// What every waiter on one scope observes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HeadState {
    /// The highest committed delivery sequence in the scope.
    pub(crate) sequence: i64,
    /// Whether the scope has a `sync_scopes` row, so records can arrive at all.
    pub(crate) scope_exists: bool,
    /// Bumped when something other than the head changed what a subscriber
    /// would report: quarantine, membership, or session health.
    pub(crate) revision: u64,
}

/// The per-scope notifier every commit path advances while holding the runtime
/// lock. Blocking readers wait on the condvar; asynchronous readers await the
/// watch.
pub(crate) struct ScopeHead {
    state: Mutex<HeadState>,
    condvar: Condvar,
    watch: watch::Sender<HeadState>,
}

impl std::fmt::Debug for ScopeHead {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ScopeHead").finish_non_exhaustive()
    }
}

impl ScopeHead {
    pub(crate) fn new(sequence: i64, scope_exists: bool) -> Self {
        let initial = HeadState {
            sequence,
            scope_exists,
            revision: 0,
        };
        Self {
            state: Mutex::new(initial),
            condvar: Condvar::new(),
            watch: watch::Sender::new(initial),
        }
    }

    fn update(&self, change: impl FnOnce(&mut HeadState) -> bool) {
        let Ok(mut guard) = self.state.lock() else {
            return;
        };
        if !change(&mut guard) {
            return;
        }
        let updated = *guard;
        drop(guard);
        self.condvar.notify_all();
        self.watch.send_modify(|value| *value = updated);
    }

    /// Records a committed delivery sequence. A delivery row implies the scope
    /// exists, so this also opens a subscription that was waiting for it.
    pub(crate) fn publish(&self, sequence: i64) {
        self.update(|head| {
            let changed = sequence > head.sequence || !head.scope_exists;
            head.sequence = head.sequence.max(sequence);
            head.scope_exists = true;
            changed
        });
    }

    /// Reports that the scope now has a content key and can receive records.
    pub(crate) fn set_scope_exists(&self) {
        self.update(|head| {
            let changed = !head.scope_exists;
            head.scope_exists = true;
            changed
        });
    }

    /// Asks every waiter to recompute what it reports, without a new record.
    pub(crate) fn bump_revision(&self) {
        self.update(|head| {
            head.revision = head.revision.saturating_add(1);
            true
        });
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<HeadState> {
        self.watch.subscribe()
    }

    pub(crate) fn state(&self) -> HeadState {
        self.state.lock().map_or(
            HeadState {
                sequence: 0,
                scope_exists: false,
                revision: 0,
            },
            |guard| *guard,
        )
    }

    /// Blocks until the head differs from `seen` or `timeout` elapses.
    fn wait_until_changed(&self, seen: HeadState, timeout: Duration) {
        let Ok(guard) = self.state.lock() else {
            return;
        };
        let _unused = self
            .condvar
            .wait_timeout_while(guard, timeout, |head| *head == seen);
    }
}

/// Returns the scope's notifier, creating it from storage on first use.
pub(crate) fn scope_head(state: &RuntimeState, scope: [u8; 32]) -> Arc<ScopeHead> {
    if let Some(head) = lookup_head(&state.record_heads, scope) {
        return head;
    }
    let sequence: i64 = state
        .connection
        .query_row(
            "SELECT COALESCE(MAX(delivery_sequence), 0) FROM record_delivery WHERE scope_id=?1",
            [scope.as_slice()],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let exists: bool = state
        .connection
        .query_row(
            "SELECT 1 FROM sync_scopes WHERE scope_id=?1",
            [scope.as_slice()],
            |_| Ok(true),
        )
        .optional()
        .ok()
        .flatten()
        .unwrap_or(false);
    let head = Arc::new(ScopeHead::new(sequence, exists));
    match state.record_heads.lock() {
        Ok(mut heads) => Arc::clone(heads.entry(scope).or_insert(head)),
        Err(_) => head,
    }
}

/// Reads one scope's notifier without taking the runtime lock.
///
/// Sync health and quarantine bump a revision from outside the runtime, so this
/// map is shared rather than owned by the runtime state.
pub(crate) fn lookup_head(heads: &SharedRecordHeads, scope: [u8; 32]) -> Option<Arc<ScopeHead>> {
    heads.lock().ok()?.get(&scope).map(Arc::clone)
}

/// Convenience for the commit paths: publish a delivery sequence for one scope.
pub(crate) fn publish_commit(
    state: &RuntimeState,
    workspace: WorkspaceId,
    package: &str,
    delivery_sequence: i64,
) {
    let scope = scope_digest(package, workspace);
    scope_head(state, scope).publish(delivery_sequence);
}

/// Reports that a scope's content key has arrived.
pub(crate) fn publish_scope_exists(state: &RuntimeState, workspace: WorkspaceId, package: &str) {
    let scope = scope_digest(package, workspace);
    scope_head(state, scope).set_scope_exists();
}

/// Asks every subscriber of one Workspace's scopes to recompute what it
/// reports, after a membership transition changed who the peers are.
pub(crate) fn bump_workspace_scopes(state: &RuntimeState, workspace: WorkspaceId) {
    let scopes: Vec<[u8; 32]> = {
        let Ok(mut statement) = state
            .connection
            .prepare("SELECT scope_id FROM sync_scopes WHERE workspace_id=?1")
        else {
            return;
        };
        let Ok(rows) = statement.query_map([workspace.as_bytes().as_slice()], |row| {
            row.get::<_, Vec<u8>>(0)
        }) else {
            return;
        };
        rows.filter_map(|row| row.ok().and_then(|id| id.try_into().ok()))
            .collect()
    };
    for scope in scopes {
        if let Some(head) = lookup_head(&state.record_heads, scope) {
            head.bump_revision();
        }
    }
}

/// The map every runtime shares with the components that must wake subscribers
/// without taking the runtime lock.
pub(crate) type SharedRecordHeads = Arc<Mutex<BTreeMap<[u8; 32], Arc<ScopeHead>>>>;

/// Reads the scope's delivery journal identity, if the scope exists.
pub(crate) fn journal_of(
    connection: &rusqlite::Connection,
    scope: [u8; 32],
) -> Result<Option<[u8; 16]>, SynchronizationError> {
    let journal: Option<Vec<u8>> = connection
        .query_row(
            "SELECT journal_id FROM sync_scopes WHERE scope_id=?1",
            [scope.as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| storage_error())?;
    journal.map(fixed::<16>).transpose()
}

/// Turns a subscription cursor into a starting delivery position.
///
/// A cursor can only come from an existing journal, so any cursor for a scope
/// this Installation does not hold is ahead of it.
pub(crate) fn validate_cursor(
    connection: &rusqlite::Connection,
    scope: [u8; 32],
    journal: Option<[u8; 16]>,
    after: Option<Cursor>,
) -> Result<i64, SynchronizationError> {
    let Some(cursor) = after else {
        return Ok(0);
    };
    let Some(journal) = journal else {
        return Err(SynchronizationError::CursorAhead);
    };
    let bytes = cursor.as_bytes();
    if bytes[1..17] != journal {
        return Err(SynchronizationError::CursorScopeMismatch);
    }
    let sequence = i64::try_from(u64::from_be_bytes(
        bytes[17..25].try_into().expect("cursor width"),
    ))
    .map_err(|_| SynchronizationError::CursorAhead)?;
    let head: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(delivery_sequence), 0) FROM record_delivery WHERE scope_id=?1",
            [scope.as_slice()],
            |row| row.get(0),
        )
        .map_err(|_| storage_error())?;
    if sequence > head {
        return Err(SynchronizationError::CursorAhead);
    }
    Ok(sequence)
}

/// One bounded read of committed records.
#[derive(Debug, Default)]
pub struct RecordPage {
    /// The records read, in delivery order.
    pub events: Vec<RecordEvent>,
    /// How many records in the same range were skipped because they are
    /// quarantined. Their delivery sequences are still consumed.
    pub quarantined_skipped: usize,
}

/// What a Package is told about its ability to synchronize this scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SyncAvailability {
    /// The scope has no content key yet, so no history can be read.
    WaitingForHistoryAccess,
    /// Nothing is reachable, or this Installation may no longer participate.
    Offline,
    /// Records are known to be outstanding, and peers are reachable.
    CatchingUp,
    /// Everything known locally has been delivered.
    Current,
}

impl SyncAvailability {
    /// The stable wire name of this state.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WaitingForHistoryAccess => "waiting_for_history_access",
            Self::Offline => "offline",
            Self::CatchingUp => "catching_up",
            Self::Current => "current",
        }
    }
}

/// A stable, non-sensitive reason a subscription is not fully healthy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum SyncWarning {
    /// At least one record was skipped because it failed authentication.
    RecordQuarantined,
    /// The whole scope is isolated after a local integrity failure.
    ScopeQuarantined,
    /// This Installation is no longer a current Member of the Workspace.
    MembershipRevoked,
}

impl SyncWarning {
    /// The stable wire code of this warning.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RecordQuarantined => "record_quarantined",
            Self::ScopeQuarantined => "scope_quarantined",
            Self::MembershipRevoked => "membership_revoked",
        }
    }
}

/// What a subscription reports about its scope at one moment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordAvailability {
    /// The availability state.
    pub availability: SyncAvailability,
    /// Records known to be outstanding for this subscription.
    pub pending_records: u64,
    /// Every warning that applies, in a stable order.
    pub warnings: Vec<SyncWarning>,
}

/// A durable-journal subscription that holds no queue in Core.
pub struct RecordSubscription {
    pub(crate) state: Weak<Mutex<RuntimeState>>,
    pub(crate) workspace: WorkspaceId,
    pub(crate) package: String,
    pub(crate) scope: [u8; 32],
    pub(crate) journal: Option<[u8; 16]>,
    pub(crate) position: i64,
    pub(crate) head: watch::Receiver<HeadState>,
    pub(crate) scope_head: Arc<ScopeHead>,
    pub(crate) buffer: VecDeque<RecordEvent>,
    /// Records skipped as quarantined since availability was last read.
    pub(crate) quarantined_seen: usize,
    pub(crate) last_seen: HeadState,
    /// This Installation, so a revocation of it is visible to the Package.
    pub(crate) local: crate::PublicIdentity,
    pub(crate) health: Arc<SyncHealthRegistry>,
}

impl std::fmt::Debug for RecordSubscription {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RecordSubscription")
            .finish_non_exhaustive()
    }
}

impl RecordSubscription {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        state: Weak<Mutex<RuntimeState>>,
        workspace: WorkspaceId,
        package: String,
        scope: [u8; 32],
        journal: Option<[u8; 16]>,
        position: i64,
        scope_head: Arc<ScopeHead>,
        local: crate::PublicIdentity,
        health: Arc<SyncHealthRegistry>,
    ) -> Self {
        let head = scope_head.subscribe();
        let last_seen = *head.borrow();
        Self {
            state,
            workspace,
            package,
            scope,
            journal,
            position,
            head,
            scope_head,
            buffer: VecDeque::new(),
            quarantined_seen: 0,
            last_seen,
            local,
            health,
        }
    }

    /// Reads at most `limit` (clamped to 1..=32) committed records after the
    /// current position, authenticating each one.
    ///
    /// Blocking: it takes the runtime state lock for exactly one page.
    pub fn next_page(&mut self, limit: usize) -> Result<RecordPage, SynchronizationError> {
        let limit = limit.clamp(1, MAX_PAGE);
        self.last_seen = *self.head.borrow_and_update();
        let runtime = self
            .state
            .upgrade()
            .ok_or(SynchronizationError::RuntimeUnavailable)?;
        let state = runtime
            .lock()
            .map_err(|_| SynchronizationError::RuntimeUnavailable)?;
        if state.poisoned {
            return Err(SynchronizationError::RuntimeUnavailable);
        }
        let now_unix_ms = state.clock.now_unix_ms();
        let Some((journal, content_key)) = self.scope_state(&state)? else {
            return Ok(RecordPage::default());
        };
        self.journal = Some(journal);
        let rows = self.read_rows(&state.connection, limit)?;
        let protection = P2pandaRecordProtection {
            package_identity: self.package.clone(),
            workspace_id: self.workspace,
            content_key,
        };
        let mut page = RecordPage::default();
        for row in rows {
            let (sequence, id, order, author, purpose, signature, sealed, quarantined) = row;
            self.position = self.position.max(sequence);
            if quarantined {
                page.quarantined_skipped += 1;
                continue;
            }
            let record_id: [u8; 32] = fixed(id.clone())?;
            let Ok(sealed_record) = SealedRecordV1::decode(&sealed) else {
                quarantine::quarantine_record(
                    &state.connection,
                    self.scope,
                    record_id,
                    quarantine::RecordFault::Decode,
                    now_unix_ms,
                )?;
                page.quarantined_skipped += 1;
                continue;
            };
            let Ok(opened) = protection.open(&sealed_record) else {
                quarantine::quarantine_record(
                    &state.connection,
                    self.scope,
                    record_id,
                    quarantine::RecordFault::Open,
                    now_unix_ms,
                )?;
                page.quarantined_skipped += 1;
                continue;
            };
            note_decoded(self.scope);
            let record = record_from_row((
                &id,
                &order,
                &author,
                &purpose,
                &signature,
                &opened.canonical,
                &sealed,
            ))?;
            page.events.push(RecordEvent {
                record,
                cursor: Cursor::new(
                    journal,
                    u64::try_from(sequence).map_err(|_| SynchronizationError::CorruptRecord)?,
                ),
            });
        }
        drop(state);
        self.quarantined_seen = self
            .quarantined_seen
            .saturating_add(page.quarantined_skipped);
        Ok(page)
    }

    /// A bounded convenience for blocking callers: serves a buffered page and
    /// waits at most `timeout` for the next commit.
    pub fn next_event_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<RecordEvent>, SynchronizationError> {
        if let Some(event) = self.buffer.pop_front() {
            return Ok(Some(event));
        }
        let page = self.next_page(MAX_PAGE)?;
        self.buffer.extend(page.events);
        if let Some(event) = self.buffer.pop_front() {
            return Ok(Some(event));
        }
        if timeout.is_zero() {
            return Ok(None);
        }
        self.scope_head.wait_until_changed(self.last_seen, timeout);
        let page = self.next_page(MAX_PAGE)?;
        self.buffer.extend(page.events);
        Ok(self.buffer.pop_front())
    }

    /// Resolves when the head, scope existence, quarantine, or session health
    /// changed since the last page or availability read.
    ///
    /// # Errors
    ///
    /// Returns [`SynchronizationError::RuntimeUnavailable`] once the runtime is
    /// gone.
    pub async fn changed(&mut self) -> Result<(), SynchronizationError> {
        self.head
            .changed()
            .await
            .map_err(|_| SynchronizationError::RuntimeUnavailable)
    }

    /// What this subscription reports about its scope right now.
    ///
    /// Blocking: it takes the runtime state lock for one bounded read.
    ///
    /// # Errors
    ///
    /// Returns [`SynchronizationError::RuntimeUnavailable`] once the runtime is
    /// gone, and a storage error when the counts cannot be read.
    #[doc(hidden)]
    pub fn availability(&mut self) -> Result<RecordAvailability, SynchronizationError> {
        self.last_seen = *self.head.borrow_and_update();
        let record_quarantined_seen = std::mem::take(&mut self.quarantined_seen) > 0;
        let runtime = self
            .state
            .upgrade()
            .ok_or(SynchronizationError::RuntimeUnavailable)?;
        let state = runtime
            .lock()
            .map_err(|_| SynchronizationError::RuntimeUnavailable)?;
        if state.poisoned {
            return Err(SynchronizationError::RuntimeUnavailable);
        }
        let connection = &state.connection;
        let scope_quarantined = quarantine::is_scope_quarantined(connection, self.scope)?;
        let scope_exists = connection
            .query_row(
                "SELECT 1 FROM sync_scopes WHERE scope_id=?1",
                [self.scope.as_slice()],
                |_| Ok(true),
            )
            .optional()
            .map_err(|_| storage_error())?
            .unwrap_or(false);
        let undelivered: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM record_delivery
                 WHERE scope_id=?1 AND delivery_sequence>?2",
                rusqlite::params![self.scope.as_slice(), self.position],
                |row| row.get(0),
            )
            .map_err(|_| storage_error())?;
        let parked = parking::parked_count(connection, self.workspace, &self.package)?;
        let projection = state.memberships.get(&self.workspace);
        let locally_revoked = projection.is_some_and(|projection| {
            !projection.contains_member(self.local) && projection.has_membership_history(self.local)
        });
        let peers = projection
            .map(|projection| {
                projection
                    .snapshot()
                    .members()
                    .iter()
                    .map(crate::MemberSnapshot::identity)
                    .filter(|member| *member != self.local)
                    .collect()
            })
            .unwrap_or_default();
        drop(state);
        Ok(health::availability(&AvailabilityInputs {
            scope_quarantined,
            locally_revoked,
            scope_exists,
            undelivered: u64::try_from(undelivered).unwrap_or(0),
            parked: parked as u64,
            peers,
            health: self.health.snapshot(self.scope),
            record_quarantined_seen,
        }))
    }

    /// The cursor this subscription would resume from.
    #[must_use]
    pub fn cursor(&self) -> Option<Cursor> {
        let journal = self.journal?;
        u64::try_from(self.position)
            .ok()
            .map(|sequence| Cursor::new(journal, sequence))
    }

    fn scope_state(&self, state: &RuntimeState) -> Result<Option<ScopeKeys>, SynchronizationError> {
        let found: Option<(Vec<u8>, Vec<u8>)> = state
            .connection
            .query_row(
                "SELECT journal_id, content_key FROM sync_scopes WHERE scope_id=?1",
                [self.scope.as_slice()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|_| storage_error())?;
        match found {
            Some((journal, key)) => Ok(Some((fixed(journal)?, ContentKey::from_column(key)?))),
            None => Ok(None),
        }
    }

    /// Reads one page of delivery rows, including the quarantined ones.
    ///
    /// A quarantined row is reported rather than filtered out, so the position
    /// advances past it and the subscription never re-reads the same gap.
    fn read_rows(
        &self,
        connection: &rusqlite::Connection,
        limit: usize,
    ) -> Result<Vec<DeliveryRow>, SynchronizationError> {
        let mut statement = connection
            .prepare(
                "SELECT d.delivery_sequence,r.record_id,o.order_key,o.author_public_identity,
                        r.purpose,r.signature,r.sealed_record,
                        CASE WHEN q.record_id IS NULL THEN 0 ELSE 1 END
                 FROM record_delivery d
                 JOIN immutable_records r ON r.record_id=d.record_id
                 JOIN immutable_operations o ON o.record_id=r.record_id
                 LEFT JOIN quarantined_records q ON q.record_id=r.record_id
                 WHERE d.scope_id=?1 AND d.delivery_sequence>?2
                 ORDER BY d.delivery_sequence LIMIT ?3",
            )
            .map_err(|_| storage_error())?;
        let rows = statement
            .query_map(
                rusqlite::params![
                    self.scope.as_slice(),
                    self.position,
                    i64::try_from(limit).unwrap_or(i64::from(u32::MAX))
                ],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Vec<u8>>(5)?,
                        row.get::<_, Vec<u8>>(6)?,
                        row.get::<_, i64>(7)? != 0,
                    ))
                },
            )
            .map_err(|_| storage_error())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| storage_error())
    }
}
