//! Durable mapping between Teamarness semantic operations and private P2Panda operations.
//!
//! The projection deliberately stores the P2Panda header and sealed payload separately from the
//! Teamarness identifiers. This keeps `RecordId`, `OrderKey`, and `Cursor` independent while giving
//! the later LogSync adapter a durable, authenticated author log.

use iroh_base::SecretKey;
use p2panda_core::operation::validate_operation;
use p2panda_core::{Body, Hash, Header, Operation, SigningKey};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::{OrderKey, RecordId, SealedRecordV1};
use crate::PublicIdentity;
use crate::diag::fault;
use crate::error::StorageError;

const LOG_DOMAIN: &[u8] = b"teamarness.immutable-record-log/v1";
const OPERATION_VERSION: u8 = 1;
type RepairRow = (
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    i64,
    String,
    Vec<u8>,
    i64,
);

pub(crate) fn log_id(scope_id: [u8; 32]) -> [u8; 32] {
    let mut input = Vec::with_capacity(LOG_DOMAIN.len() + scope_id.len());
    input.extend_from_slice(LOG_DOMAIN);
    input.extend_from_slice(&scope_id);
    *Hash::digest(input).as_bytes()
}

pub(crate) fn encode_operation(header: &Header<()>, payload: &[u8]) -> Vec<u8> {
    let header = header.encode();
    let mut bytes = Vec::with_capacity(1 + 4 + header.len() + payload.len());
    bytes.push(OPERATION_VERSION);
    bytes.extend_from_slice(&(header.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(payload);
    bytes
}

pub(crate) fn decode_operation(
    bytes: &[u8],
) -> Result<(p2panda_core::AnyHeader, Vec<u8>), StorageError> {
    if bytes.len() < 5 || bytes[0] != OPERATION_VERSION {
        return Err(StorageError::CorruptWorkspace);
    }
    let header_len = u32::from_be_bytes(
        bytes[1..5]
            .try_into()
            .map_err(|_| StorageError::CorruptWorkspace)?,
    ) as usize;
    if header_len == 0 || bytes.len() < 5 + header_len {
        return Err(StorageError::CorruptWorkspace);
    }
    let header = p2panda_core::AnyHeader::decode(&bytes[5..5 + header_len])
        .map_err(|_| StorageError::CorruptWorkspace)?;
    Ok((header, bytes[5 + header_len..].to_vec()))
}

fn p2panda_key(secret: &SecretKey) -> SigningKey {
    SigningKey::from_bytes(&secret.to_bytes())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn insert(
    tx: &Transaction<'_>,
    secret: &SecretKey,
    scope_id: [u8; 32],
    teamarness_operation_id: RecordId,
    record_id: RecordId,
    order_key: OrderKey,
    author: PublicIdentity,
    author_sequence: u32,
    payload: &[u8],
    causal_parents: &[u8],
    causal_height: u64,
    operation_kind: &str,
    content_key_id: [u8; 32],
) -> Result<[u8; 32], StorageError> {
    let log = log_id(scope_id);
    let backlink: Option<[u8; 32]> = if author_sequence == 0 {
        None
    } else {
        tx.query_row(
            "SELECT operation_id FROM p2panda_operations
             WHERE scope_id=?1 AND log_id=?2 AND author_public_identity=?3 AND author_sequence=?4",
            params![
                scope_id.as_slice(),
                log.as_slice(),
                author.as_bytes().as_slice(),
                i64::from(author_sequence - 1)
            ],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(fault!(StorageError::Database))?
        .map(|bytes| bytes.try_into().map_err(|_| StorageError::CorruptWorkspace))
        .transpose()?
    };

    let key = p2panda_key(secret);
    let header = Header::<()>::builder()
        .body(payload)
        .seq_num(author_sequence)
        .backlink(backlink.map(Hash::from_bytes))
        .build(&key, ());
    let operation = Operation::from_parts(header.clone(), Some(Body::from_bytes(payload)));
    validate_operation(&operation).map_err(|_| StorageError::CorruptWorkspace)?;
    if operation.header.verifying_key.as_bytes() != author.as_bytes() {
        return Err(StorageError::CorruptWorkspace);
    }
    let header_bytes = header.encode();
    let operation_bytes = encode_operation(&header, payload);
    let operation_id = operation.hash.as_bytes().to_owned();
    let payload_hash = Hash::digest(payload);
    tx.execute(
        "INSERT INTO p2panda_operations(
            operation_id, teamarness_operation_id, scope_id, log_id,
            author_public_identity, author_sequence, backlink, payload_hash, payload,
            header, operation_bytes, record_id, order_key, content_key_id,
            causal_parents, causal_height, true_height, operation_kind, status
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,'accepted')",
        params![
            operation_id.as_slice(),
            teamarness_operation_id.as_bytes().as_slice(),
            scope_id.as_slice(),
            log.as_slice(),
            author.as_bytes().as_slice(),
            i64::from(author_sequence),
            backlink.as_ref().map(|value| value.as_slice()),
            payload_hash.as_bytes().as_slice(),
            payload,
            header_bytes,
            operation_bytes,
            record_id.as_bytes().as_slice(),
            order_key.as_bytes().as_slice(),
            content_key_id.as_slice(),
            causal_parents,
            i64::try_from(causal_height).map_err(|_| StorageError::CorruptWorkspace)?,
            i64::try_from(causal_height).map_err(|_| StorageError::CorruptWorkspace)?,
            operation_kind,
        ],
    )
    .map_err(fault!(StorageError::Database))?;
    Ok(operation_id)
}

/// Re-materialize local Phase 2 rows and retain an explicit queue for rows whose author key is not
/// held by this installation. No unverified synthetic operation is inserted during recovery.
#[allow(clippy::too_many_arguments)]
pub(crate) fn insert_remote(
    tx: &Transaction<'_>,
    scope_id: [u8; 32],
    teamarness_operation_id: RecordId,
    record_id: RecordId,
    order_key: OrderKey,
    operation: &Operation<()>,
    operation_kind: &str,
    content_key_id: [u8; 32],
) -> Result<[u8; 32], StorageError> {
    validate_operation(operation).map_err(|_| StorageError::CorruptWorkspace)?;
    let payload = operation
        .body
        .as_ref()
        .ok_or(StorageError::CorruptWorkspace)?
        .to_bytes();
    let author = super::key_bridge::identity(&operation.header.verifying_key)
        .ok_or(StorageError::CorruptWorkspace)?;
    let operation_id = operation.hash.as_bytes();
    let backlink = operation
        .header
        .backlink
        .map(|value| value.as_bytes().to_owned());
    let sequence = operation.header.seq_num;
    let log = log_id(scope_id);
    tx.execute(
        "INSERT INTO p2panda_operations(
            operation_id, teamarness_operation_id, scope_id, log_id,
            author_public_identity, author_sequence, backlink, payload_hash, payload,
            header, operation_bytes, record_id, order_key, content_key_id,
            causal_parents, causal_height, true_height, operation_kind, status
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,'accepted')",
        params![
            operation_id.as_slice(),
            teamarness_operation_id.as_bytes().as_slice(),
            scope_id.as_slice(),
            log.as_slice(),
            author.as_bytes().as_slice(),
            i64::from(sequence),
            backlink.as_ref().map(|value| value.as_slice()),
            Hash::digest(&payload).as_bytes().as_slice(),
            payload.as_slice(),
            operation.header.encode(),
            encode_operation(&operation.header, &payload),
            record_id.as_bytes().as_slice(),
            order_key.as_bytes().as_slice(),
            content_key_id.as_slice(),
            Vec::<u8>::new(),
            i64::try_from(order_key.height()).map_err(|_| StorageError::CorruptWorkspace)?,
            i64::try_from(order_key.height()).map_err(|_| StorageError::CorruptWorkspace)?,
            operation_kind,
        ],
    )
    .map_err(|_| StorageError::CorruptWorkspace)?;
    Ok(*operation_id)
}

pub(crate) fn repair(connection: &mut Connection, secret: &SecretKey) -> Result<(), StorageError> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS p2panda_projection_recovery (
                teamarness_operation_id BLOB PRIMARY KEY
                    REFERENCES immutable_operations(operation_id),
                reason TEXT NOT NULL CHECK (reason IN ('missing_author_key', 'invalid_projection'))
            ) STRICT;
            CREATE INDEX IF NOT EXISTS p2panda_projection_recovery_reason
                ON p2panda_projection_recovery(reason);",
        )
        .map_err(fault!(StorageError::Database))?;

    let rows: Vec<RepairRow> = {
        let mut statement = connection
            .prepare(
                "SELECT o.operation_id, o.scope_id, o.record_id, o.order_key,
                        o.author_public_identity, o.author_sequence, r.purpose,
                        r.sealed_record, o.causal_height
                 FROM immutable_operations o
                 JOIN immutable_records r ON r.record_id=o.record_id
                 LEFT JOIN p2panda_operations p ON p.teamarness_operation_id=o.operation_id
                 WHERE p.teamarness_operation_id IS NULL AND o.status='accepted'",
            )
            .map_err(fault!(StorageError::Database))?;
        let result = statement
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            })
            .map_err(fault!(StorageError::Database))?;
        result
            .collect::<Result<Vec<_>, _>>()
            .map_err(fault!(StorageError::Database))?
    };

    let local_author = *secret.public().as_bytes();
    for (teamarness_id, scope, record, order, author, sequence, purpose, sealed, height) in rows {
        let transaction = connection
            .transaction()
            .map_err(fault!(StorageError::Database))?;
        let author_is_local = author.as_slice() == local_author.as_slice();
        let result = if author_is_local {
            let scope_id: [u8; 32] = scope
                .clone()
                .try_into()
                .map_err(|_| StorageError::CorruptWorkspace)?;
            let record_id = RecordId(
                record
                    .clone()
                    .try_into()
                    .map_err(|_| StorageError::CorruptWorkspace)?,
            );
            let order_key = OrderKey(
                order
                    .clone()
                    .try_into()
                    .map_err(|_| StorageError::CorruptWorkspace)?,
            );
            let sealed_record =
                SealedRecordV1::decode(&sealed).map_err(|_| StorageError::CorruptWorkspace)?;
            let author = PublicIdentity::from_bytes(
                author
                    .clone()
                    .try_into()
                    .map_err(|_| StorageError::CorruptWorkspace)?,
            )
            .map_err(|_| StorageError::CorruptWorkspace)?;
            let content_key_id: [u8; 32] = sealed_record.content_key_id;
            let operation_id = RecordId(
                teamarness_id
                    .clone()
                    .try_into()
                    .map_err(|_| StorageError::CorruptWorkspace)?,
            );
            let sequence = u32::try_from(sequence).map_err(|_| StorageError::CorruptWorkspace)?;
            insert(
                &transaction,
                secret,
                scope_id,
                operation_id,
                record_id,
                order_key,
                author,
                sequence,
                &sealed,
                &[],
                u64::try_from(height).map_err(|_| StorageError::CorruptWorkspace)?,
                &purpose,
                content_key_id,
            )
            .map(|_| ())
        } else {
            transaction
                .execute(
                    "INSERT OR IGNORE INTO p2panda_projection_recovery
                     (teamarness_operation_id, reason) VALUES (?1, 'missing_author_key')",
                    [teamarness_id.as_slice()],
                )
                .map(|_| ())
                .map_err(fault!(StorageError::Database))
        };
        match result {
            Ok(()) => transaction
                .commit()
                .map_err(fault!(StorageError::UncertainOutcome))?,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Check one durable projection row, named by the Teamarness operation it
/// projects.
///
/// Restart validation checks only the records committed since the last
/// validated start, so this is per row rather than per table.
pub(crate) fn validate_projection_row(
    connection: &Connection,
    teamarness_operation_id: &[u8],
) -> Result<(), StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT p.operation_id, p.teamarness_operation_id, p.scope_id, p.log_id,
                    p.author_public_identity, p.author_sequence, p.backlink, p.payload_hash,
                    p.payload, p.header, p.operation_bytes, p.record_id, p.order_key,
                    p.causal_height, p.true_height, o.operation_bytes, o.scope_id,
                    o.record_id, o.order_key, o.author_public_identity, o.author_sequence,
                    o.causal_height
             FROM p2panda_operations p
             JOIN immutable_operations o ON o.operation_id=p.teamarness_operation_id
             WHERE p.status='accepted' AND p.teamarness_operation_id=?1",
        )
        .map_err(fault!(StorageError::Database))?;
    let mut seen = false;
    let rows = statement
        .query_map([teamarness_operation_id], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, Vec<u8>>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, Option<Vec<u8>>>(6)?,
                row.get::<_, Vec<u8>>(7)?,
                row.get::<_, Vec<u8>>(8)?,
                row.get::<_, Vec<u8>>(9)?,
                row.get::<_, Vec<u8>>(10)?,
                row.get::<_, Vec<u8>>(11)?,
                row.get::<_, Vec<u8>>(12)?,
                row.get::<_, i64>(13)?,
                row.get::<_, i64>(14)?,
                row.get::<_, Vec<u8>>(15)?,
                row.get::<_, Vec<u8>>(16)?,
                row.get::<_, Vec<u8>>(17)?,
                row.get::<_, Vec<u8>>(18)?,
                row.get::<_, Vec<u8>>(19)?,
                row.get::<_, i64>(20)?,
                row.get::<_, i64>(21)?,
            ))
        })
        .map_err(fault!(StorageError::Database))?;
    for row in rows {
        let (
            operation_id,
            teamarness_id,
            scope,
            log,
            author,
            sequence,
            backlink,
            payload_hash,
            payload,
            header,
            operation_bytes,
            record,
            order,
            causal_height,
            true_height,
            sealed,
            team_scope,
            team_record,
            team_order,
            team_author,
            team_sequence,
            team_height,
        ) = row.map_err(|_| StorageError::CorruptWorkspace)?;
        let scope_id: [u8; 32] = scope
            .clone()
            .try_into()
            .map_err(|_| StorageError::CorruptWorkspace)?;
        if log != log_id(scope_id).as_slice()
            || operation_bytes
                != encode_operation(
                    &p2panda_core::Header::<()>::decode(&header)
                        .map_err(|_| StorageError::CorruptWorkspace)?,
                    &payload,
                )
            || payload_hash != Hash::digest(&payload).as_bytes().as_slice()
            || payload != sealed
            || causal_height != true_height
            || scope != team_scope
            || record != team_record
            || order != team_order
            || author != team_author
            || sequence != team_sequence
            || causal_height != team_height
            || teamarness_id.is_empty()
            || record.len() != 32
            || order.len() != 41
        {
            return Err(StorageError::CorruptWorkspace);
        }
        let (decoded_header, decoded_payload) = decode_operation(&operation_bytes)?;
        if decoded_payload != payload
            || decoded_header.encode() != header
            || decoded_header.hash().as_bytes().as_slice() != operation_id.as_slice()
            || decoded_header.verifying_key.as_bytes() != author.as_slice()
            || i64::from(decoded_header.seq_num) != sequence
            || decoded_header
                .backlink
                .map(|value| value.as_bytes().to_vec())
                != backlink
            || (sequence == 0 && backlink.is_some())
            || (sequence > 0 && backlink.is_none())
        {
            return Err(StorageError::CorruptWorkspace);
        }
        let expected_backlink: Option<Vec<u8>> = if sequence > 0 {
            connection
                .query_row(
                    "SELECT operation_id FROM p2panda_operations
                     WHERE scope_id=?1 AND author_public_identity=?2 AND author_sequence=?3",
                    params![scope.as_slice(), author.as_slice(), sequence - 1],
                    |row| row.get(0),
                )
                .optional()
                .map_err(fault!(StorageError::Database))?
        } else {
            None
        };
        if expected_backlink != backlink {
            return Err(StorageError::CorruptWorkspace);
        }
        seen = true;
    }
    // A committed record with no accepted projection row is as broken as a row
    // that fails a check.
    if !seen {
        return Err(StorageError::CorruptWorkspace);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_maps_to_a_stable_private_log_id() {
        let first = log_id([7; 32]);
        assert_eq!(first, log_id([7; 32]));
        assert_ne!(first, log_id([8; 32]));
    }
}
