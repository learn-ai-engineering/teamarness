//! Isolation of a corrupt record or scope.
//!
//! A local integrity failure never stops the Core Runtime from opening, and
//! never stops other scopes from working. The failing record or scope is
//! recorded here instead, and every reader — restart validation, subscriptions,
//! and the log a peer is served — skips what this module marks.
//!
//! Quarantine rows are insert-only in version 1: there is no un-quarantine
//! path, and a developer resets the profile.

use rusqlite::OptionalExtension;

use super::SynchronizationError;
use crate::PublicIdentity;

/// Why one record is isolated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordFault {
    Decode,
    OrderKey,
    Open,
}

impl RecordFault {
    fn reason(self) -> &'static str {
        match self {
            Self::Decode => "decode",
            Self::OrderKey => "order_key",
            Self::Open => "open",
        }
    }
}

/// Why a whole scope is isolated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopeFault {
    ScopeBinding,
    ContentKeyId,
    Projection,
}

impl ScopeFault {
    fn reason(self) -> &'static str {
        match self {
            Self::ScopeBinding => "scope_binding",
            Self::ContentKeyId => "content_key_id",
            Self::Projection => "projection",
        }
    }
}

/// What one restart validated and isolated.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RecoveryReport {
    pub(crate) scopes_checked: usize,
    pub(crate) records_validated: usize,
    pub(crate) scopes_quarantined: usize,
    pub(crate) records_quarantined: usize,
}

pub(crate) fn quarantine_record(
    connection: &rusqlite::Connection,
    scope: [u8; 32],
    record_id: [u8; 32],
    fault: RecordFault,
    now_unix_ms: i64,
) -> Result<(), SynchronizationError> {
    connection
        .execute(
            "INSERT OR IGNORE INTO quarantined_records
             (record_id, scope_id, reason, detected_at_unix_ms) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                record_id.as_slice(),
                scope.as_slice(),
                fault.reason(),
                now_unix_ms
            ],
        )
        .map(|_| ())
        .map_err(|_| super::storage_error())
}

pub(crate) fn quarantine_scope(
    connection: &rusqlite::Connection,
    scope: [u8; 32],
    fault: ScopeFault,
    now_unix_ms: i64,
) -> Result<(), SynchronizationError> {
    connection
        .execute(
            "INSERT OR IGNORE INTO quarantined_scopes
             (scope_id, reason, detected_at_unix_ms) VALUES (?1, ?2, ?3)",
            rusqlite::params![scope.as_slice(), fault.reason(), now_unix_ms],
        )
        .map(|_| ())
        .map_err(|_| super::storage_error())
}

pub(crate) fn is_scope_quarantined(
    connection: &rusqlite::Connection,
    scope: [u8; 32],
) -> Result<bool, SynchronizationError> {
    connection
        .query_row(
            "SELECT 1 FROM quarantined_scopes WHERE scope_id=?1",
            [scope.as_slice()],
            |_| Ok(()),
        )
        .optional()
        .map(|found| found.is_some())
        .map_err(|_| super::storage_error())
}

/// The first author sequence in this scope that is no longer sound.
///
/// A peer is served an author's log only up to the row before this, because a
/// peer would refuse anything past a missing sequence anyway.
pub(crate) fn lowest_quarantined_sequence(
    connection: &rusqlite::Connection,
    scope: [u8; 32],
    author: PublicIdentity,
) -> Result<Option<i64>, SynchronizationError> {
    connection
        .query_row(
            "SELECT MIN(o.author_sequence) FROM quarantined_records q
             JOIN immutable_operations o ON o.record_id = q.record_id
             WHERE q.scope_id = ?1 AND o.scope_id = ?1 AND o.author_public_identity = ?2",
            rusqlite::params![scope.as_slice(), author.as_bytes().as_slice()],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(|_| super::storage_error())
}
