//! The one place an accepted operation becomes journal rows.
//!
//! A local append and a remote ingest wrote the same six statements in two
//! copies, one of them as rustfmt-exempt 300-character lines. They now share
//! these statements and this row, so the bytes a record leaves behind cannot
//! drift between the path that produced it and the path that received it.
//!
//! Both halves return `rusqlite::Error`, so each ingest path keeps its own
//! error vocabulary: a local append reports storage, and a remote ingest
//! separates a constraint violation from a storage failure.

use p2panda_core::Hash;
use rusqlite::Transaction;

const INSERT_OPERATION: &str = "INSERT INTO immutable_operations(
     operation_id, scope_id, record_id, order_key, author_public_identity,
     author_sequence, operation_bytes, parents, causal_height, status)
 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'accepted')";

const INSERT_RECORD: &str = "INSERT INTO immutable_records(
     record_id, scope_id, purpose, canonical_digest, signature, sealed_record)
 VALUES(?1, ?2, ?3, ?4, ?5, ?6)";

const INSERT_SEALED_ENVELOPE: &str =
    "INSERT INTO sealed_envelopes(record_id, scope_id, envelope) VALUES(?1, ?2, ?3)";

const INSERT_IDEMPOTENCY: &str = "INSERT INTO immutable_idempotency(
     scope_id, author_public_identity, purpose, canonical_digest, record_id)
 VALUES(?1, ?2, ?3, ?4, ?5)";

const UPSERT_AUTHOR_HEAD: &str =
    "INSERT INTO immutable_author_heads(scope_id, author_public_identity, operation_id)
 VALUES(?1, ?2, ?3)
 ON CONFLICT(scope_id, author_public_identity) DO UPDATE SET operation_id = excluded.operation_id";

const INSERT_DELIVERY: &str =
    "INSERT INTO record_delivery(scope_id, journal_id, record_id, created_at_unix_ms)
 VALUES(?1, ?2, ?3, ?4)";

const BUMP_WAKE: &str = "UPDATE durable_wakes SET wake_generation = wake_generation + 1
 WHERE scope_id = ?1";

/// Everything the journal rows of one accepted operation are written from.
///
/// Every value is explicit, including the ones the two ingest paths derive
/// differently, so the rows they write stay byte-for-byte the same.
pub(super) struct AcceptedOperation<'a> {
    pub(super) operation_id: &'a [u8],
    pub(super) scope: &'a [u8],
    pub(super) record_id: &'a [u8],
    pub(super) order_key: &'a [u8],
    pub(super) author: &'a [u8],
    pub(super) sequence: i64,
    pub(super) operation_bytes: &'a [u8],
    pub(super) parents: &'a [u8],
    pub(super) causal_height: u64,
    pub(super) purpose: &'a str,
    pub(super) canonical: &'a [u8],
    pub(super) signature: &'a [u8],
    pub(super) sealed_record: &'a [u8],
    pub(super) journal: &'a [u8],
    pub(super) created_at_unix_ms: i64,
}

/// The operation row itself.
///
/// This is separate from [`insert_record_rows`] because the p2panda projection
/// is written between the two, and moving it would change which row
/// `last_insert_rowid` reports.
pub(super) fn insert_operation(
    tx: &Transaction<'_>,
    row: &AcceptedOperation<'_>,
) -> Result<(), rusqlite::Error> {
    tx.execute(
        INSERT_OPERATION,
        rusqlite::params![
            row.operation_id,
            row.scope,
            row.record_id,
            row.order_key,
            row.author,
            row.sequence,
            row.operation_bytes,
            row.parents,
            row.causal_height,
        ],
    )?;
    Ok(())
}

/// The record, its sealed envelope, its idempotency key, the author head, the
/// delivery row and the scope's wake, in that order.
///
/// Returns the delivery sequence the record was given.
pub(super) fn insert_record_rows(
    tx: &Transaction<'_>,
    row: &AcceptedOperation<'_>,
) -> Result<i64, rusqlite::Error> {
    let canonical_digest = Hash::digest(row.canonical);
    tx.execute(
        INSERT_RECORD,
        rusqlite::params![
            row.record_id,
            row.scope,
            row.purpose,
            canonical_digest.as_bytes().as_slice(),
            row.signature,
            row.sealed_record,
        ],
    )?;
    tx.execute(
        INSERT_SEALED_ENVELOPE,
        rusqlite::params![row.record_id, row.scope, row.sealed_record],
    )?;
    tx.execute(
        INSERT_IDEMPOTENCY,
        rusqlite::params![
            row.scope,
            row.author,
            row.purpose,
            canonical_digest.as_bytes().as_slice(),
            row.record_id,
        ],
    )?;
    tx.execute(
        UPSERT_AUTHOR_HEAD,
        rusqlite::params![row.scope, row.author, row.operation_id],
    )?;
    tx.execute(
        INSERT_DELIVERY,
        rusqlite::params![
            row.scope,
            row.journal,
            row.record_id,
            row.created_at_unix_ms,
        ],
    )?;
    let delivery_sequence = tx.last_insert_rowid();
    tx.execute(BUMP_WAKE, [row.scope])?;
    Ok(delivery_sequence)
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use rusqlite::types::Value;

    use super::*;

    const SCOPE: [u8; 32] = [1; 32];
    const JOURNAL: [u8; 16] = [2; 16];

    fn row<'a>(
        sealed: &'a [u8],
        canonical: &'a [u8],
        signature: &'a [u8],
    ) -> AcceptedOperation<'a> {
        AcceptedOperation {
            operation_id: &[3; 32],
            scope: &SCOPE,
            record_id: &[4; 32],
            order_key: &[5; 41],
            author: &[6; 32],
            sequence: 7,
            operation_bytes: sealed,
            parents: &[],
            causal_height: 9,
            purpose: "chat.message",
            canonical,
            signature,
            sealed_record: sealed,
            journal: &JOURNAL,
            created_at_unix_ms: 1_700_000_000_000,
        }
    }

    /// Every column of one table, in the order the query names them.
    fn dump(connection: &Connection, query: &str) -> Vec<Vec<Value>> {
        let mut statement = connection.prepare(query).unwrap();
        let columns = statement.column_count();
        let rows = statement
            .query_map([], |row| {
                (0..columns)
                    .map(|index| row.get::<_, Value>(index))
                    .collect()
            })
            .unwrap();
        rows.map(Result::unwrap).collect()
    }

    fn blob(bytes: &[u8]) -> Value {
        Value::Blob(bytes.to_vec())
    }

    fn text(value: &str) -> Value {
        Value::Text(value.to_owned())
    }

    /// Both ingest paths write through these two functions, so this pins the one
    /// mapping from the row to the columns. A swapped parameter would show here
    /// before it reached a journal.
    #[test]
    fn an_accepted_operation_lands_in_every_journal_table() {
        let mut connection = crate::storage::memory_schema_for_test();
        let sealed = vec![10_u8; 48];
        let canonical = vec![11_u8; 16];
        let signature = vec![12_u8; 64];
        let accepted = row(&sealed, &canonical, &signature);
        let digest = blob(Hash::digest(&canonical).as_bytes());

        let tx = connection.transaction().unwrap();
        insert_operation(&tx, &accepted).unwrap();
        let delivery_sequence = insert_record_rows(&tx, &accepted).unwrap();
        tx.commit().unwrap();

        assert_eq!(
            dump(
                &connection,
                "SELECT operation_id, scope_id, record_id, order_key, author_public_identity,
                        author_sequence, operation_bytes, parents, causal_height, status
                 FROM immutable_operations",
            ),
            vec![vec![
                blob(&[3; 32]),
                blob(&SCOPE),
                blob(&[4; 32]),
                blob(&[5; 41]),
                blob(&[6; 32]),
                Value::Integer(7),
                blob(&sealed),
                blob(&[]),
                Value::Integer(9),
                text("accepted"),
            ]]
        );
        assert_eq!(
            dump(
                &connection,
                "SELECT record_id, scope_id, purpose, canonical_digest, signature, sealed_record
                 FROM immutable_records",
            ),
            vec![vec![
                blob(&[4; 32]),
                blob(&SCOPE),
                text("chat.message"),
                digest.clone(),
                blob(&signature),
                blob(&sealed),
            ]]
        );
        assert_eq!(
            dump(
                &connection,
                "SELECT record_id, scope_id, envelope FROM sealed_envelopes",
            ),
            vec![vec![blob(&[4; 32]), blob(&SCOPE), blob(&sealed)]]
        );
        assert_eq!(
            dump(
                &connection,
                "SELECT scope_id, author_public_identity, purpose, canonical_digest, record_id
                 FROM immutable_idempotency",
            ),
            vec![vec![
                blob(&SCOPE),
                blob(&[6; 32]),
                text("chat.message"),
                digest,
                blob(&[4; 32]),
            ]]
        );
        assert_eq!(
            dump(
                &connection,
                "SELECT scope_id, author_public_identity, operation_id FROM immutable_author_heads",
            ),
            vec![vec![blob(&SCOPE), blob(&[6; 32]), blob(&[3; 32])]]
        );
        assert_eq!(
            dump(
                &connection,
                "SELECT delivery_sequence, scope_id, journal_id, record_id, created_at_unix_ms
                 FROM record_delivery",
            ),
            vec![vec![
                Value::Integer(delivery_sequence),
                blob(&SCOPE),
                blob(&JOURNAL),
                blob(&[4; 32]),
                Value::Integer(1_700_000_000_000),
            ]]
        );
    }

    /// The head is an upsert: a second operation by the same author in the same
    /// scope replaces it rather than failing the ingest.
    #[test]
    fn a_later_operation_replaces_the_author_head() {
        let mut connection = crate::storage::memory_schema_for_test();
        let sealed = vec![10_u8; 48];
        let canonical = vec![11_u8; 16];
        let signature = vec![12_u8; 64];

        let tx = connection.transaction().unwrap();
        insert_record_rows(&tx, &row(&sealed, &canonical, &signature)).unwrap();
        let second_canonical = vec![15_u8; 16];
        let mut later = row(&sealed, &second_canonical, &signature);
        later.operation_id = &[13; 32];
        later.record_id = &[14; 32];
        insert_record_rows(&tx, &later).unwrap();
        tx.commit().unwrap();

        assert_eq!(
            dump(
                &connection,
                "SELECT operation_id FROM immutable_author_heads",
            ),
            vec![vec![blob(&[13; 32])]]
        );
    }
}
