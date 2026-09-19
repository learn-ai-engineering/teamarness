//! Bounded durable parking for authenticated operations that cannot be applied yet.
//!
//! An operation whose author has no known membership history, or whose scope has
//! no content key, is neither applied nor dropped: it waits here until the
//! admission or the key arrives. Parking is bounded per scope and per sending
//! peer within a scope, and expires, so a peer cannot use it as storage.

use p2panda_core::Operation;
use rusqlite::{Connection, OptionalExtension};

use super::{SealedRecordV1, SynchronizationError, scope_digest, storage_error};
use crate::{PublicIdentity, WorkspaceId};

/// The bounds parking runs under. Plan phase 4 tunes these in one place.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ParkingLimits {
    pub(crate) scope_operations: usize,
    pub(crate) scope_bytes: usize,
    pub(crate) peer_operations: usize,
    pub(crate) peer_bytes: usize,
    pub(crate) ttl_ms: i64,
}

impl ParkingLimits {
    pub(crate) const CURRENT: Self = Self {
        scope_operations: 4_096,
        scope_bytes: 64 * 1024 * 1024,
        peer_operations: 1_024,
        peer_bytes: 16 * 1024 * 1024,
        ttl_ms: 7 * 24 * 60 * 60 * 1_000,
    };
}

/// Why an authenticated operation is waiting rather than applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParkReason {
    UnknownAuthor,
    MissingContentKey,
}

impl ParkReason {
    const fn tag(self) -> &'static str {
        match self {
            Self::UnknownAuthor => "unknown_author",
            Self::MissingContentKey => "missing_content_key",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParkResult {
    Parked,
    AlreadyParked,
    QuotaExceeded,
}

#[derive(Debug, Clone)]
pub(crate) struct ParkedOperation {
    pub(crate) operation_hash: [u8; 32],
    pub(crate) package: String,
    pub(crate) operation: Operation<()>,
}

/// Parks one authenticated operation within the scope and per-peer bounds.
///
/// The operation is validated and bound to its scope first, so parking never
/// stores something that could not be applied even once its author is known.
#[allow(clippy::too_many_arguments)]
pub(crate) fn park(
    conn: &mut Connection,
    workspace: WorkspaceId,
    package: &str,
    from: PublicIdentity,
    reason: ParkReason,
    operation: &Operation<()>,
    now_unix_ms: i64,
    limits: ParkingLimits,
) -> Result<ParkResult, SynchronizationError> {
    p2panda_core::operation::validate_operation(operation)
        .map_err(|_| SynchronizationError::InvalidInput)?;
    let payload = operation
        .body
        .as_ref()
        .ok_or(SynchronizationError::InvalidInput)?
        .to_bytes();
    let sealed =
        SealedRecordV1::decode(&payload).map_err(|_| SynchronizationError::InvalidInput)?;
    let scope = scope_digest(package, workspace);
    if sealed.scope_digest != scope {
        return Err(SynchronizationError::WrongScope);
    }
    let author = PublicIdentity::from_bytes(*operation.header.verifying_key.as_bytes())
        .map_err(|_| SynchronizationError::InvalidInput)?;
    let bytes = super::operation_store::encode_operation(&operation.header, &payload);
    let hash = *operation.hash.as_bytes();

    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| storage_error())?;
    purge_expired_in(&tx, now_unix_ms, limits)?;
    let existing: Option<i64> = tx
        .query_row(
            "SELECT 1 FROM parked_operations WHERE operation_hash=?1",
            [hash.as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| storage_error())?;
    if existing.is_some() {
        tx.commit()
            .map_err(|_| SynchronizationError::Storage(crate::StorageError::UncertainOutcome))?;
        return Ok(ParkResult::AlreadyParked);
    }
    let (scope_count, scope_bytes) = usage(
        &tx,
        "SELECT COUNT(*), COALESCE(SUM(length(operation_bytes)),0)
         FROM parked_operations WHERE scope_id=?1",
        rusqlite::params![scope.as_slice()],
    )?;
    let (peer_count, peer_bytes) = usage(
        &tx,
        "SELECT COUNT(*), COALESCE(SUM(length(operation_bytes)),0)
         FROM parked_operations WHERE scope_id=?1 AND received_from=?2",
        rusqlite::params![scope.as_slice(), from.as_bytes().as_slice()],
    )?;
    let added = bytes.len();
    if scope_count + 1 > limits.scope_operations
        || scope_bytes + added > limits.scope_bytes
        || peer_count + 1 > limits.peer_operations
        || peer_bytes + added > limits.peer_bytes
    {
        tx.commit()
            .map_err(|_| SynchronizationError::Storage(crate::StorageError::UncertainOutcome))?;
        return Ok(ParkResult::QuotaExceeded);
    }
    tx.execute(
        "INSERT INTO parked_operations(
            operation_hash, scope_id, workspace_id, package_identity,
            author_public_identity, author_sequence, reason, received_from,
            operation_bytes, parked_at_unix_ms
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        rusqlite::params![
            hash.as_slice(),
            scope.as_slice(),
            workspace.as_bytes().as_slice(),
            package,
            author.as_bytes().as_slice(),
            i64::from(operation.header.seq_num),
            reason.tag(),
            from.as_bytes().as_slice(),
            bytes.as_slice(),
            now_unix_ms,
        ],
    )
    .map_err(|_| storage_error())?;
    tx.commit()
        .map_err(|_| SynchronizationError::Storage(crate::StorageError::UncertainOutcome))?;
    Ok(ParkResult::Parked)
}

fn usage(
    conn: &Connection,
    query: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<(usize, usize), SynchronizationError> {
    let (count, bytes): (i64, i64) = conn
        .query_row(query, params, |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|_| storage_error())?;
    Ok((
        usize::try_from(count).map_err(|_| storage_error())?,
        usize::try_from(bytes).map_err(|_| storage_error())?,
    ))
}

/// Everything parked for `author` in this Workspace because it was unknown.
pub(crate) fn parked_for_author(
    conn: &Connection,
    workspace: WorkspaceId,
    author: PublicIdentity,
) -> Result<Vec<ParkedOperation>, SynchronizationError> {
    load(
        conn,
        "SELECT operation_hash, package_identity, operation_bytes FROM parked_operations
         WHERE workspace_id=?1 AND author_public_identity=?2 AND reason='unknown_author'
         ORDER BY scope_id, author_sequence",
        rusqlite::params![
            workspace.as_bytes().as_slice(),
            author.as_bytes().as_slice()
        ],
    )
}

/// Everything parked for one scope because its content key had not arrived.
pub(crate) fn parked_for_scope(
    conn: &Connection,
    workspace: WorkspaceId,
    package: &str,
) -> Result<Vec<ParkedOperation>, SynchronizationError> {
    load(
        conn,
        "SELECT operation_hash, package_identity, operation_bytes FROM parked_operations
         WHERE scope_id=?1 AND reason='missing_content_key'
         ORDER BY author_public_identity, author_sequence",
        rusqlite::params![scope_digest(package, workspace).as_slice()],
    )
}

fn load(
    conn: &Connection,
    query: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<Vec<ParkedOperation>, SynchronizationError> {
    let mut statement = conn.prepare(query).map_err(|_| storage_error())?;
    let rows = statement
        .query_map(params, |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .map_err(|_| storage_error())?;
    let mut parked = Vec::new();
    for row in rows {
        let (hash, package, bytes) = row.map_err(|_| storage_error())?;
        let (header, payload) = super::operation_store::decode_operation(&bytes)
            .map_err(|_| SynchronizationError::CorruptRecord)?;
        let operation =
            Operation::try_from((header, Some(p2panda_core::Body::from_bytes(&payload))))
                .map_err(|_| SynchronizationError::CorruptRecord)?;
        parked.push(ParkedOperation {
            operation_hash: hash
                .try_into()
                .map_err(|_| SynchronizationError::CorruptRecord)?,
            package,
            operation,
        });
    }
    Ok(parked)
}

pub(crate) fn remove(
    conn: &Connection,
    operation_hash: [u8; 32],
) -> Result<(), SynchronizationError> {
    conn.execute(
        "DELETE FROM parked_operations WHERE operation_hash=?1",
        [operation_hash.as_slice()],
    )
    .map(|_| ())
    .map_err(|_| storage_error())
}

/// Drops everything parked longer than the time-to-live.
pub(crate) fn purge_expired(
    conn: &Connection,
    now_unix_ms: i64,
    limits: ParkingLimits,
) -> Result<usize, SynchronizationError> {
    purge_expired_in(conn, now_unix_ms, limits)
}

fn purge_expired_in(
    conn: &Connection,
    now_unix_ms: i64,
    limits: ParkingLimits,
) -> Result<usize, SynchronizationError> {
    conn.execute(
        "DELETE FROM parked_operations WHERE parked_at_unix_ms < ?1",
        [now_unix_ms.saturating_sub(limits.ttl_ms)],
    )
    .map_err(|_| storage_error())
}

/// How many operations one scope currently holds parked.
///
/// Plan phase 4 reports this as the Package-visible pending record count.
pub(crate) fn parked_count(
    conn: &Connection,
    workspace: WorkspaceId,
    package: &str,
) -> Result<usize, SynchronizationError> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM parked_operations WHERE scope_id=?1",
            [scope_digest(package, workspace).as_slice()],
            |row| row.get(0),
        )
        .map_err(|_| storage_error())?;
    usize::try_from(count).map_err(|_| storage_error())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use iroh_base::SecretKey;
    use p2panda_core::{Body, Hash, Header, SigningKey};

    use super::super::{
        ContentKey, OrderKey, P2pandaRecordProtection, RecordId, RecordProtection, SignedRecordV1,
    };
    use super::*;

    const PACKAGE: &str = "teamarness.test";
    const SMALL: ParkingLimits = ParkingLimits {
        scope_operations: 3,
        scope_bytes: 4_096,
        peer_operations: 2,
        peer_bytes: 2_048,
        ttl_ms: 1_000,
    };

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    /// A real migrated SQLite database and real signed, sealed operations.
    /// Only the peer identities are synthetic.
    struct Fixture {
        directory: PathBuf,
        connection: Connection,
        secret: SecretKey,
        workspace: WorkspaceId,
        key: ContentKey,
        next_sequence: u32,
        previous: Option<Hash>,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let directory = std::env::temp_dir().join(format!(
                "teamarness-parking-{name}-{}-{sequence}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&directory);
            let stored = crate::storage::open(&directory, 0).expect("a migrated test database");
            Self {
                directory,
                connection: stored.connection,
                secret: stored.secret_key,
                workspace: WorkspaceId::from_bytes([11; 16]),
                key: ContentKey::from_column(vec![7; 32]).expect("a test key"),
                next_sequence: 0,
                previous: None,
            }
        }

        fn connection(&self) -> &Connection {
            &self.connection
        }

        fn sealed(&self, package: &str, canonical: &[u8], height: u64) -> Vec<u8> {
            let signed = SignedRecordV1::sign(
                &self.secret,
                package,
                self.workspace,
                "message.post",
                canonical,
            )
            .expect("a signed test record");
            let protection = P2pandaRecordProtection {
                package_identity: package.to_owned(),
                workspace_id: self.workspace,
                content_key: self.key.clone(),
            };
            let record_id = RecordId::digest(&signed.canonical_bytes());
            let order = OrderKey::from_height_and_id(height, record_id);
            protection
                .seal(&signed, order)
                .expect("a sealed record")
                .encode()
        }

        fn chain(&mut self, payload: &[u8]) -> Operation<()> {
            let sequence = self.next_sequence;
            let header = Header::<()>::builder()
                .body(payload)
                .seq_num(sequence)
                .backlink(self.previous)
                .build(&SigningKey::from_bytes(&self.secret.to_bytes()), ());
            let operation = Operation::from_parts(header, Some(Body::from_bytes(payload)));
            self.next_sequence += 1;
            self.previous = Some(operation.hash);
            operation
        }

        fn operation(&mut self, canonical: &[u8]) -> Operation<()> {
            let height = u64::from(self.next_sequence);
            let payload = self.sealed(PACKAGE, canonical, height);
            self.chain(&payload)
        }

        fn operation_for_other_scope(&mut self, canonical: &[u8]) -> Operation<()> {
            let height = u64::from(self.next_sequence);
            let payload = self.sealed("teamarness.other", canonical, height);
            self.chain(&payload)
        }

        fn operation_with_broken_signature(&mut self, canonical: &[u8]) -> Operation<()> {
            let operation = self.operation(canonical);
            Operation::from_parts(operation.header, Some(Body::from_bytes(b"not the payload")))
        }

        fn peer(seed: u8) -> PublicIdentity {
            PublicIdentity::from_bytes(*SecretKey::from_bytes(&[seed; 32]).public().as_bytes())
                .expect("a test peer identity")
        }

        fn park(
            &mut self,
            operation: &Operation<()>,
            reason: ParkReason,
            now: i64,
            limits: ParkingLimits,
        ) -> Result<ParkResult, SynchronizationError> {
            self.park_from(operation, 1, reason, now, limits)
        }

        fn park_from(
            &mut self,
            operation: &Operation<()>,
            peer: u8,
            reason: ParkReason,
            now: i64,
            limits: ParkingLimits,
        ) -> Result<ParkResult, SynchronizationError> {
            park(
                &mut self.connection,
                self.workspace,
                PACKAGE,
                Self::peer(peer),
                reason,
                operation,
                now,
                limits,
            )
        }

        fn count(&self) -> usize {
            parked_count(&self.connection, self.workspace, PACKAGE).expect("a parked count")
        }

        fn parked_for_author(&self) -> Vec<ParkedOperation> {
            let author =
                PublicIdentity::from_bytes(*self.secret.public().as_bytes()).expect("an author");
            parked_for_author(&self.connection, self.workspace, author).expect("parked operations")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn park_is_idempotent_by_operation_hash() {
        let mut fixture = Fixture::new("idempotent");
        let operation = fixture.operation(b"one");
        assert_eq!(
            fixture.park(&operation, ParkReason::UnknownAuthor, 0, SMALL),
            Ok(ParkResult::Parked)
        );
        assert_eq!(
            fixture.park(&operation, ParkReason::UnknownAuthor, 0, SMALL),
            Ok(ParkResult::AlreadyParked)
        );
        assert_eq!(fixture.count(), 1);
    }

    #[test]
    fn park_refuses_beyond_scope_operation_limit() {
        let mut fixture = Fixture::new("scope-count");
        let limits = ParkingLimits {
            peer_operations: 16,
            peer_bytes: 1 << 20,
            ..SMALL
        };
        for index in 0..3_u8 {
            let operation = fixture.operation(&[index]);
            assert_eq!(
                fixture.park(&operation, ParkReason::UnknownAuthor, 0, limits),
                Ok(ParkResult::Parked)
            );
        }
        let overflow = fixture.operation(b"overflow");
        assert_eq!(
            fixture.park(&overflow, ParkReason::UnknownAuthor, 0, limits),
            Ok(ParkResult::QuotaExceeded)
        );
        assert_eq!(fixture.count(), 3);
    }

    #[test]
    fn park_refuses_beyond_scope_byte_limit() {
        let mut fixture = Fixture::new("scope-bytes");
        let limits = ParkingLimits {
            scope_bytes: 8,
            ..SMALL
        };
        let operation = fixture.operation(b"one");
        assert_eq!(
            fixture.park(&operation, ParkReason::UnknownAuthor, 0, limits),
            Ok(ParkResult::QuotaExceeded)
        );
        assert_eq!(fixture.count(), 0);
    }

    #[test]
    fn park_refuses_beyond_per_peer_limits_but_accepts_other_peers() {
        let mut fixture = Fixture::new("peer");
        let first = fixture.operation(b"one");
        let second = fixture.operation(b"two");
        let third = fixture.operation(b"three");
        assert_eq!(
            fixture.park_from(&first, 1, ParkReason::UnknownAuthor, 0, SMALL),
            Ok(ParkResult::Parked)
        );
        assert_eq!(
            fixture.park_from(&second, 1, ParkReason::UnknownAuthor, 0, SMALL),
            Ok(ParkResult::Parked)
        );
        assert_eq!(
            fixture.park_from(&third, 1, ParkReason::UnknownAuthor, 0, SMALL),
            Ok(ParkResult::QuotaExceeded)
        );
        assert_eq!(
            fixture.park_from(&third, 2, ParkReason::UnknownAuthor, 0, SMALL),
            Ok(ParkResult::Parked),
            "a second peer has its own allowance"
        );
    }

    #[test]
    fn park_rejects_unauthenticated_or_wrong_scope_operations() {
        let mut fixture = Fixture::new("invalid");
        let forged = fixture.operation_for_other_scope(b"other");
        assert_eq!(
            fixture.park(&forged, ParkReason::UnknownAuthor, 0, SMALL),
            Err(SynchronizationError::WrongScope)
        );
        let tampered = fixture.operation_with_broken_signature(b"tampered");
        assert_eq!(
            fixture.park(&tampered, ParkReason::UnknownAuthor, 0, SMALL),
            Err(SynchronizationError::InvalidInput)
        );
        assert_eq!(fixture.count(), 0);
    }

    #[test]
    fn expired_operations_are_purged() {
        let mut fixture = Fixture::new("expiry");
        let operation = fixture.operation(b"one");
        assert_eq!(
            fixture.park(&operation, ParkReason::UnknownAuthor, 0, SMALL),
            Ok(ParkResult::Parked)
        );
        let later = fixture.operation(b"two");
        assert_eq!(
            fixture.park(&later, ParkReason::UnknownAuthor, 5_000, SMALL),
            Ok(ParkResult::Parked)
        );
        assert_eq!(
            fixture.count(),
            1,
            "the first park expired before the second was accepted"
        );
        assert_eq!(
            purge_expired(fixture.connection(), 10_000, SMALL).unwrap(),
            1
        );
        assert_eq!(fixture.count(), 0);
    }

    #[test]
    fn parked_for_author_orders_by_sequence() {
        let mut fixture = Fixture::new("order");
        let limits = ParkingLimits {
            peer_operations: 16,
            scope_operations: 16,
            ..SMALL
        };
        let first = fixture.operation(b"one");
        let second = fixture.operation(b"two");
        assert_eq!(
            fixture.park(&second, ParkReason::UnknownAuthor, 0, limits),
            Ok(ParkResult::Parked)
        );
        assert_eq!(
            fixture.park(&first, ParkReason::UnknownAuthor, 0, limits),
            Ok(ParkResult::Parked)
        );
        let parked = fixture.parked_for_author();
        assert_eq!(parked.len(), 2);
        assert!(parked[0].operation.header.seq_num < parked[1].operation.header.seq_num);
        remove(fixture.connection(), parked[0].operation_hash).unwrap();
        assert_eq!(fixture.count(), 1);
    }
}
