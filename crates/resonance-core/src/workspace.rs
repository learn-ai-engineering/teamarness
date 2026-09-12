use iroh_base::SecretKey;
use p2panda_auth::group::GroupCrdt;
use p2panda_auth::group::resolver::StrongRemove;
use p2panda_core::Hash;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::error::{MembershipError, StorageError, WorkspaceError};
use crate::membership::{
    AuthId, MembershipAction, MembershipEnvelope, OperationAdapter, validate_context,
};
use crate::{
    MembershipOperationId, MembershipVersion, PublicIdentity, WorkspaceId, WorkspaceSummary,
};

const MAX_WORKSPACE_NAME_CHARS: usize = 64;
const VERSION_DOMAIN: &[u8] = b"resonance.membership-version/v1";

type Resolver = StrongRemove<AuthId, Hash, OperationAdapter, ()>;
type MembershipCrdt = GroupCrdt<AuthId, Hash, OperationAdapter, (), Resolver>;
type StoredOperationRow = (Vec<u8>, Vec<u8>, i64, i64, Vec<u8>);
type CreationRequestRow = (Vec<u8>, Vec<u8>, String, i64, Vec<u8>, Vec<u8>, String);

#[derive(Clone)]
struct CreationRequest {
    workspace_id: WorkspaceId,
    creator: PublicIdentity,
    name: String,
    authored_at_unix_ms: i64,
    genesis_id: MembershipOperationId,
    genesis_envelope: Vec<u8>,
    status: String,
}

pub(crate) fn validate_name(name: &str) -> Result<(), WorkspaceError> {
    let character_count = name.chars().count();
    let has_valid_characters = name.chars().all(|character| {
        character.is_alphanumeric() || matches!(character, ' ' | '-' | '_' | '\'' | '.')
    });
    if character_count == 0
        || character_count > MAX_WORKSPACE_NAME_CHARS
        || name.trim() != name
        || !has_valid_characters
    {
        return Err(WorkspaceError::InvalidName);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CreationBoundary {
    BeforeTransactionOne,
    AfterTransactionOne,
    DuringTransactionTwo,
    AfterTransactionTwoCommit,
}

pub(crate) fn create(
    connection: &mut Connection,
    secret_key: &SecretKey,
    name: &str,
    authored_at_unix_ms: i64,
) -> Result<WorkspaceSummary, WorkspaceError> {
    create_with_fault(
        connection,
        secret_key,
        name,
        authored_at_unix_ms,
        |_| Ok(()),
    )
}

#[cfg(test)]
pub(crate) fn create_with_test_fault(
    connection: &mut Connection,
    secret_key: &SecretKey,
    name: &str,
    authored_at_unix_ms: i64,
    fault: impl FnMut(CreationBoundary) -> Result<(), StorageError>,
) -> Result<WorkspaceSummary, WorkspaceError> {
    create_with_fault(connection, secret_key, name, authored_at_unix_ms, fault)
}

fn create_with_fault(
    connection: &mut Connection,
    secret_key: &SecretKey,
    name: &str,
    authored_at_unix_ms: i64,
    mut fault: impl FnMut(CreationBoundary) -> Result<(), StorageError>,
) -> Result<WorkspaceSummary, WorkspaceError> {
    validate_name(name)?;
    let generated = SecretKey::generate();
    let mut workspace_bytes = [0_u8; 16];
    workspace_bytes.copy_from_slice(&generated.public().as_bytes()[..16]);
    let workspace_id = WorkspaceId::from_bytes(workspace_bytes);
    let creator = PublicIdentity::from_bytes(*secret_key.public().as_bytes())
        .map_err(|_| WorkspaceError::CorruptTrustAnchor)?;
    let genesis = MembershipEnvelope::sign(
        secret_key,
        workspace_id,
        authored_at_unix_ms,
        MembershipAction::Create,
        vec![],
    )
    .map_err(|_| WorkspaceError::CorruptTrustAnchor)?;

    fault(CreationBoundary::BeforeTransactionOne).map_err(workspace_from_storage)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| WorkspaceError::Storage(StorageError::Database))?;
    transaction
        .execute(
            "INSERT INTO workspace_creation_requests (
                workspace_id, creator_public_identity, name, authored_at_unix_ms,
                genesis_operation_id, genesis_envelope, status
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'Pending')",
            params![
                workspace_id.as_bytes().as_slice(),
                creator.as_bytes().as_slice(),
                name,
                authored_at_unix_ms,
                genesis.id.as_bytes().as_slice(),
                genesis.canonical_bytes(),
            ],
        )
        .map_err(|_| WorkspaceError::Conflict)?;
    transaction
        .commit()
        .map_err(|_| WorkspaceError::Storage(StorageError::UncertainOutcome))?;
    fault(CreationBoundary::AfterTransactionOne).map_err(workspace_from_storage)?;

    let request = CreationRequest {
        workspace_id,
        creator,
        name: name.to_owned(),
        authored_at_unix_ms,
        genesis_id: genesis.id,
        genesis_envelope: genesis.canonical_bytes().to_vec(),
        status: "Pending".to_owned(),
    };
    finish_request_with_fault(connection, &request, &mut fault).map_err(workspace_from_storage)?;
    Ok(summary(&request))
}

pub(crate) fn list(connection: &Connection) -> Result<Vec<WorkspaceSummary>, WorkspaceError> {
    verify_ready(connection).map_err(workspace_from_storage)?;
    let mut statement = connection
        .prepare(
            "SELECT workspace_id, creator_public_identity, name, authored_at_unix_ms
             FROM workspaces ORDER BY authored_at_unix_ms, workspace_id",
        )
        .map_err(|_| WorkspaceError::Storage(StorageError::Database))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(|_| WorkspaceError::Storage(StorageError::Database))?;
    let mut workspaces = Vec::new();
    for row in rows {
        let (workspace, creator, name, authored_at) =
            row.map_err(|_| WorkspaceError::Storage(StorageError::Database))?;
        workspaces.push(WorkspaceSummary::new(
            WorkspaceId::from_bytes(fixed_bytes(&workspace).map_err(workspace_from_storage)?),
            name,
            PublicIdentity::from_bytes(fixed_bytes(&creator).map_err(workspace_from_storage)?)
                .map_err(|_| WorkspaceError::CorruptTrustAnchor)?,
            authored_at,
        ));
    }
    Ok(workspaces)
}

pub(crate) fn recover_pending(connection: &mut Connection) -> Result<(), StorageError> {
    let requests = load_requests(connection, Some("Pending"))?;
    for request in &requests {
        finish_request(connection, request)?;
    }
    verify_ready(connection)
}

fn finish_request(
    connection: &mut Connection,
    request: &CreationRequest,
) -> Result<(), StorageError> {
    finish_request_with_fault(connection, request, &mut |_| Ok(()))
}

fn finish_request_with_fault(
    connection: &mut Connection,
    request: &CreationRequest,
    fault: &mut impl FnMut(CreationBoundary) -> Result<(), StorageError>,
) -> Result<(), StorageError> {
    let genesis = validate_genesis_request(request)?;
    process_genesis(&genesis)?;
    let version = membership_version(request.workspace_id, request.genesis_id);
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| StorageError::Database)?;

    if let Some(existing) = load_workspace(&transaction, request.workspace_id)? {
        if !workspace_matches_request(&existing, request) {
            return Err(StorageError::CorruptWorkspace);
        }
    } else {
        transaction
            .execute(
                "INSERT INTO workspaces (
                    workspace_id, creator_public_identity, name, authored_at_unix_ms,
                    genesis_operation_id, genesis_envelope
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    request.workspace_id.as_bytes().as_slice(),
                    request.creator.as_bytes().as_slice(),
                    request.name,
                    request.authored_at_unix_ms,
                    request.genesis_id.as_bytes().as_slice(),
                    request.genesis_envelope,
                ],
            )
            .map_err(|_| StorageError::CorruptWorkspace)?;
    }

    let existing_operation: Option<Vec<u8>> = transaction
        .query_row(
            "SELECT canonical_envelope FROM membership_operations WHERE operation_id = ?1",
            [request.genesis_id.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| StorageError::Database)?;
    match existing_operation {
        Some(bytes) if bytes != request.genesis_envelope => {
            return Err(StorageError::CorruptWorkspace);
        }
        Some(_) => {}
        None => {
            transaction
                .execute(
                    "INSERT INTO membership_operations (
                        operation_id, workspace_id, author_public_identity,
                        authored_at_unix_ms, action, canonical_envelope
                     ) VALUES (?1, ?2, ?3, ?4, 0, ?5)",
                    params![
                        request.genesis_id.as_bytes().as_slice(),
                        request.workspace_id.as_bytes().as_slice(),
                        request.creator.as_bytes().as_slice(),
                        request.authored_at_unix_ms,
                        request.genesis_envelope,
                    ],
                )
                .map_err(|_| StorageError::CorruptWorkspace)?;
        }
    }

    insert_or_verify_state(&transaction, request, version)?;
    insert_or_verify_creator(&transaction, request)?;
    let updated = transaction
        .execute(
            "UPDATE workspace_creation_requests SET status = 'Complete'
             WHERE workspace_id = ?1 AND status = 'Pending'",
            [request.workspace_id.as_bytes().as_slice()],
        )
        .map_err(|_| StorageError::Database)?;
    if updated > 1 {
        return Err(StorageError::CorruptWorkspace);
    }
    fault(CreationBoundary::DuringTransactionTwo)?;
    transaction
        .commit()
        .map_err(|_| StorageError::UncertainOutcome)?;
    fault(CreationBoundary::AfterTransactionTwoCommit)?;
    Ok(())
}

fn validate_genesis_request(request: &CreationRequest) -> Result<MembershipEnvelope, StorageError> {
    validate_name(&request.name).map_err(|_| StorageError::CorruptWorkspace)?;
    let envelope = MembershipEnvelope::decode(&request.genesis_envelope)
        .map_err(|_| StorageError::CorruptWorkspace)?;
    if envelope.id != request.genesis_id
        || envelope.workspace_id != request.workspace_id
        || envelope.author != request.creator
        || envelope.authored_at_unix_ms != request.authored_at_unix_ms
        || !envelope.dependencies.is_empty()
        || !matches!(envelope.action, MembershipAction::Create)
    {
        return Err(StorageError::CorruptWorkspace);
    }
    Ok(envelope)
}

fn process_genesis(envelope: &MembershipEnvelope) -> Result<(), StorageError> {
    if !matches!(envelope.action, MembershipAction::Create) || !envelope.dependencies.is_empty() {
        return Err(StorageError::CorruptWorkspace);
    }
    let adapter = OperationAdapter(envelope.clone());
    let state = MembershipCrdt::process(MembershipCrdt::init(), &adapter)
        .map_err(|_| StorageError::CorruptWorkspace)?;
    let members = state.members(AuthId::workspace(envelope.workspace_id));
    if members.len() != 1
        || members[0].0 != AuthId::installation(envelope.author)
        || !members[0].1.is_manage()
    {
        return Err(StorageError::CorruptWorkspace);
    }
    Ok(())
}

fn membership_version(
    workspace_id: WorkspaceId,
    genesis_id: MembershipOperationId,
) -> MembershipVersion {
    let mut bytes = Vec::with_capacity(VERSION_DOMAIN.len() + 16 + 32 + 32);
    bytes.extend_from_slice(VERSION_DOMAIN);
    bytes.extend_from_slice(workspace_id.as_bytes());
    bytes.extend_from_slice(genesis_id.as_bytes());
    bytes.extend_from_slice(genesis_id.as_bytes());
    MembershipVersion::from_bytes(*Hash::digest(bytes).as_bytes())
}

fn insert_or_verify_state(
    transaction: &Transaction<'_>,
    request: &CreationRequest,
    version: MembershipVersion,
) -> Result<(), StorageError> {
    let stored: Option<(i64, Vec<u8>)> = transaction
        .query_row(
            "SELECT epoch, membership_version FROM workspace_membership_state
             WHERE workspace_id = ?1",
            [request.workspace_id.as_bytes().as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|_| StorageError::Database)?;
    match stored {
        Some((0, stored_version)) if stored_version == version.as_bytes() => Ok(()),
        Some(_) => Err(StorageError::CorruptWorkspace),
        None => transaction
            .execute(
                "INSERT INTO workspace_membership_state (
                    workspace_id, epoch, membership_version
                 ) VALUES (?1, 0, ?2)",
                params![
                    request.workspace_id.as_bytes().as_slice(),
                    version.as_bytes().as_slice()
                ],
            )
            .map(|_| ())
            .map_err(|_| StorageError::CorruptWorkspace),
    }
}

fn insert_or_verify_creator(
    transaction: &Transaction<'_>,
    request: &CreationRequest,
) -> Result<(), StorageError> {
    let stored: Option<i64> = transaction
        .query_row(
            "SELECT role FROM membership_snapshot_members
             WHERE workspace_id = ?1 AND member_identity = ?2",
            params![
                request.workspace_id.as_bytes().as_slice(),
                request.creator.as_bytes().as_slice()
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| StorageError::Database)?;
    match stored {
        Some(1) => Ok(()),
        Some(_) => Err(StorageError::CorruptWorkspace),
        None => transaction
            .execute(
                "INSERT INTO membership_snapshot_members (workspace_id, member_identity, role)
                 VALUES (?1, ?2, 1)",
                params![
                    request.workspace_id.as_bytes().as_slice(),
                    request.creator.as_bytes().as_slice()
                ],
            )
            .map(|_| ())
            .map_err(|_| StorageError::CorruptWorkspace),
    }
}

fn verify_ready(connection: &Connection) -> Result<(), StorageError> {
    let requests = load_requests(connection, None)?;
    for request in &requests {
        if request.status != "Complete" {
            return Err(StorageError::CorruptWorkspace);
        }
        let genesis = validate_genesis_request(request)?;
        process_genesis(&genesis)?;
        let workspace = load_workspace(connection, request.workspace_id)?
            .ok_or(StorageError::CorruptWorkspace)?;
        if !workspace_matches_request(&workspace, request) {
            return Err(StorageError::CorruptWorkspace);
        }
        let operation: Option<StoredOperationRow> = connection
            .query_row(
                "SELECT workspace_id, author_public_identity, authored_at_unix_ms,
                        action, canonical_envelope
                 FROM membership_operations WHERE operation_id = ?1",
                [request.genesis_id.as_bytes().as_slice()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| StorageError::Database)?;
        if !matches!(
            operation,
            Some((workspace, author, timestamp, 0, bytes))
                if workspace == request.workspace_id.as_bytes()
                    && author == request.creator.as_bytes()
                    && timestamp == request.authored_at_unix_ms
                    && bytes == request.genesis_envelope
        ) {
            return Err(StorageError::CorruptWorkspace);
        }
        let dependency_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM membership_dependencies WHERE operation_id = ?1",
                [request.genesis_id.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .map_err(|_| StorageError::Database)?;
        if dependency_count != 0 {
            return Err(StorageError::CorruptWorkspace);
        }
    }
    let ready_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM workspaces", [], |row| row.get(0))
        .map_err(|_| StorageError::Database)?;
    if ready_count != i64::try_from(requests.len()).map_err(|_| StorageError::CorruptWorkspace)? {
        return Err(StorageError::CorruptWorkspace);
    }
    Ok(())
}

fn load_requests(
    connection: &Connection,
    status: Option<&str>,
) -> Result<Vec<CreationRequest>, StorageError> {
    let sql = match status {
        Some(_) => {
            "SELECT workspace_id, creator_public_identity, name, authored_at_unix_ms,
                    genesis_operation_id, genesis_envelope, status
             FROM workspace_creation_requests WHERE status = ?1 ORDER BY workspace_id"
        }
        None => {
            "SELECT workspace_id, creator_public_identity, name, authored_at_unix_ms,
                    genesis_operation_id, genesis_envelope, status
             FROM workspace_creation_requests ORDER BY workspace_id"
        }
    };
    let mut statement = connection
        .prepare(sql)
        .map_err(|_| StorageError::Database)?;
    let map_row = |row: &rusqlite::Row<'_>| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, Vec<u8>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, Vec<u8>>(4)?,
            row.get::<_, Vec<u8>>(5)?,
            row.get::<_, String>(6)?,
        ))
    };
    let mut requests = Vec::new();
    if let Some(status) = status {
        let rows = statement
            .query_map([status], map_row)
            .map_err(|_| StorageError::Database)?;
        for row in rows {
            requests.push(request_from_row(row.map_err(|_| StorageError::Database)?)?);
        }
    } else {
        let rows = statement
            .query_map([], map_row)
            .map_err(|_| StorageError::Database)?;
        for row in rows {
            requests.push(request_from_row(row.map_err(|_| StorageError::Database)?)?);
        }
    }
    Ok(requests)
}

fn request_from_row(row: CreationRequestRow) -> Result<CreationRequest, StorageError> {
    let (workspace, creator, name, authored_at, genesis_id, genesis_envelope, status) = row;
    Ok(CreationRequest {
        workspace_id: WorkspaceId::from_bytes(fixed_bytes(&workspace)?),
        creator: PublicIdentity::from_bytes(fixed_bytes(&creator)?)
            .map_err(|_| StorageError::CorruptWorkspace)?,
        name,
        authored_at_unix_ms: authored_at,
        genesis_id: MembershipOperationId::from_hash(Hash::from_bytes(fixed_bytes(&genesis_id)?)),
        genesis_envelope,
        status,
    })
}

fn load_workspace(
    connection: &Connection,
    workspace_id: WorkspaceId,
) -> Result<Option<CreationRequest>, StorageError> {
    let row = connection
        .query_row(
            "SELECT workspace_id, creator_public_identity, name, authored_at_unix_ms,
                    genesis_operation_id, genesis_envelope
             FROM workspaces WHERE workspace_id = ?1",
            [workspace_id.as_bytes().as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, Vec<u8>>(5)?,
                ))
            },
        )
        .optional()
        .map_err(|_| StorageError::Database)?;
    row.map(
        |(workspace, creator, name, authored_at, genesis_id, genesis_envelope)| {
            request_from_row((
                workspace,
                creator,
                name,
                authored_at,
                genesis_id,
                genesis_envelope,
                "Complete".to_owned(),
            ))
        },
    )
    .transpose()
}

fn workspace_matches_request(workspace: &CreationRequest, request: &CreationRequest) -> bool {
    workspace.workspace_id == request.workspace_id
        && workspace.creator == request.creator
        && workspace.name == request.name
        && workspace.authored_at_unix_ms == request.authored_at_unix_ms
        && workspace.genesis_id == request.genesis_id
        && workspace.genesis_envelope == request.genesis_envelope
}

fn summary(request: &CreationRequest) -> WorkspaceSummary {
    WorkspaceSummary::new(
        request.workspace_id,
        request.name.clone(),
        request.creator,
        request.authored_at_unix_ms,
    )
}

fn fixed_bytes<const N: usize>(bytes: &[u8]) -> Result<[u8; N], StorageError> {
    bytes.try_into().map_err(|_| StorageError::CorruptWorkspace)
}

fn workspace_from_storage(error: StorageError) -> WorkspaceError {
    match error {
        StorageError::CorruptWorkspace => WorkspaceError::CorruptTrustAnchor,
        other => WorkspaceError::Storage(other),
    }
}

#[allow(dead_code)]
pub(crate) fn store_future_if_needed(
    connection: &mut Connection,
    envelope: &MembershipEnvelope,
    workspace_id: WorkspaceId,
    genesis_id: MembershipOperationId,
    dependencies: &[(MembershipOperationId, WorkspaceId, i64, bool)],
    now_unix_ms: i64,
) -> Result<bool, MembershipError> {
    match validate_context(
        envelope,
        workspace_id,
        genesis_id,
        dependencies,
        now_unix_ms,
    ) {
        Err(MembershipError::FutureTimestamp) => {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| MembershipError::AuthorizationRejected)?;
            let existing: Option<Vec<u8>> = transaction
                .query_row(
                    "SELECT canonical_envelope FROM future_membership_operations
                     WHERE operation_id = ?1",
                    [envelope.id.as_bytes().as_slice()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| MembershipError::AuthorizationRejected)?;
            match existing {
                Some(bytes) if bytes == envelope.canonical_bytes() => return Ok(true),
                Some(_) => return Err(MembershipError::ContentHashMismatch),
                None => {}
            }
            transaction
                .execute(
                    "INSERT INTO future_membership_operations (
                        operation_id, workspace_id, author_public_identity,
                        authored_at_unix_ms, canonical_envelope, rejection_reason
                     ) VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
                    params![
                        envelope.id.as_bytes().as_slice(),
                        envelope.workspace_id.as_bytes().as_slice(),
                        envelope.author.as_bytes().as_slice(),
                        envelope.authored_at_unix_ms,
                        envelope.canonical_bytes(),
                    ],
                )
                .map_err(|_| MembershipError::AuthorizationRejected)?;
            for dependency in &envelope.dependencies {
                transaction
                    .execute(
                        "INSERT INTO future_membership_dependencies (
                            operation_id, dependency_operation_id
                         ) VALUES (?1, ?2)",
                        params![
                            envelope.id.as_bytes().as_slice(),
                            dependency.as_bytes().as_slice()
                        ],
                    )
                    .map_err(|_| MembershipError::AuthorizationRejected)?;
            }
            transaction
                .commit()
                .map_err(|_| MembershipError::AuthorizationRejected)?;
            Ok(true)
        }
        Ok(()) => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn secret(byte: u8) -> SecretKey {
        SecretKey::from_bytes(&[byte; 32])
    }

    fn revoke(
        secret_key: &SecretKey,
        workspace_id: WorkspaceId,
        timestamp: i64,
        dependencies: Vec<MembershipOperationId>,
    ) -> MembershipEnvelope {
        let target = PublicIdentity::from_bytes(*secret(9).public().as_bytes()).unwrap();
        MembershipEnvelope::sign(
            secret_key,
            workspace_id,
            timestamp,
            MembershipAction::Revoke { target },
            dependencies,
        )
        .unwrap()
    }

    #[test]
    fn every_creation_transaction_boundary_recovers_without_regenerating_material() {
        for (index, boundary) in [
            CreationBoundary::BeforeTransactionOne,
            CreationBoundary::AfterTransactionOne,
            CreationBoundary::DuringTransactionTwo,
            CreationBoundary::AfterTransactionTwoCommit,
        ]
        .into_iter()
        .enumerate()
        {
            let directory = std::env::temp_dir().join(format!(
                "resonance-creation-boundary-{}-{index}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&directory);
            let runtime = crate::storage::open(&directory).unwrap();
            let mut connection = runtime.connection;
            let secret_key = runtime.secret_key;
            let result = create_with_test_fault(
                &mut connection,
                &secret_key,
                "Fault boundary",
                4_242,
                |current| {
                    if current == boundary {
                        Err(if current == CreationBoundary::AfterTransactionTwoCommit {
                            StorageError::UncertainOutcome
                        } else {
                            StorageError::Database
                        })
                    } else {
                        Ok(())
                    }
                },
            );
            assert!(result.is_err());
            let pending_material: Option<(Vec<u8>, Vec<u8>, i64)> = connection
                .query_row(
                    "SELECT workspace_id, genesis_envelope, authored_at_unix_ms
                     FROM workspace_creation_requests",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .unwrap();
            drop(connection);

            let reopened = crate::storage::open(&directory).unwrap();
            let workspace_count: i64 = reopened
                .connection
                .query_row("SELECT COUNT(*) FROM workspaces", [], |row| row.get(0))
                .unwrap();
            if boundary == CreationBoundary::BeforeTransactionOne {
                assert_eq!(pending_material, None);
                assert_eq!(workspace_count, 0);
            } else {
                let (workspace_id, pending_bytes, pending_timestamp) = pending_material.unwrap();
                let recovered: (Vec<u8>, Vec<u8>, i64, i64) = reopened
                    .connection
                    .query_row(
                        "SELECT w.workspace_id, o.canonical_envelope, w.authored_at_unix_ms,
                                (SELECT COUNT(*) FROM membership_operations
                                 WHERE workspace_id = w.workspace_id)
                         FROM workspaces w
                         JOIN membership_operations o
                           ON o.operation_id = w.genesis_operation_id",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                    .unwrap();
                assert_eq!(
                    recovered,
                    (workspace_id, pending_bytes, pending_timestamp, 1)
                );
                assert_eq!(workspace_count, 1);
            }
            drop(reopened);
            fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn context_rejects_workspace_dependency_descent_timestamp_and_second_genesis_failures() {
        let workspace = WorkspaceId::from_bytes([1; 16]);
        let other_workspace = WorkspaceId::from_bytes([2; 16]);
        let genesis = MembershipOperationId::digest(b"genesis");
        let dependency = MembershipOperationId::digest(b"dependency");
        let envelope = revoke(&secret(3), workspace, 1_000, vec![dependency]);

        assert_eq!(
            validate_context(&envelope, other_workspace, genesis, &[], 1_000),
            Err(MembershipError::WorkspaceMismatch)
        );
        assert_eq!(
            validate_context(&envelope, workspace, genesis, &[], 1_000),
            Err(MembershipError::MissingDependency)
        );
        assert_eq!(
            validate_context(
                &envelope,
                workspace,
                genesis,
                &[(dependency, other_workspace, 900, true)],
                1_000,
            ),
            Err(MembershipError::CrossWorkspaceDependency)
        );
        assert_eq!(
            validate_context(
                &envelope,
                workspace,
                genesis,
                &[(dependency, workspace, 900, false)],
                1_000,
            ),
            Err(MembershipError::NonDescendantDependency)
        );
        assert_eq!(
            validate_context(
                &envelope,
                workspace,
                genesis,
                &[(dependency, workspace, 1_001, true)],
                1_001,
            ),
            Err(MembershipError::NonMonotonicTimestamp)
        );
        let no_dependencies = revoke(&secret(3), workspace, 1_000, vec![]);
        assert_eq!(
            validate_context(&no_dependencies, workspace, genesis, &[], 1_000),
            Err(MembershipError::NonDescendantDependency)
        );
        let second_genesis = MembershipEnvelope::sign(
            &secret(3),
            workspace,
            1_000,
            MembershipAction::Create,
            vec![],
        )
        .unwrap();
        assert_eq!(
            validate_context(&second_genesis, workspace, genesis, &[], 1_000),
            Err(MembershipError::ConflictingGenesis)
        );
    }

    #[test]
    fn future_authenticated_operation_is_durable_and_inert() {
        let directory =
            std::env::temp_dir().join(format!("resonance-future-context-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        let runtime = crate::storage::open(&directory).unwrap();
        let mut connection = runtime.connection;
        let creator = runtime.secret_key;
        let request_summary = create(&mut connection, &creator, "Future", 1_000).unwrap();
        let genesis_id: Vec<u8> = connection
            .query_row(
                "SELECT genesis_operation_id FROM workspaces WHERE workspace_id = ?1",
                [request_summary.id().as_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        let genesis =
            MembershipOperationId::from_hash(Hash::from_bytes(fixed_bytes(&genesis_id).unwrap()));
        let envelope = revoke(&creator, request_summary.id(), 301_001, vec![genesis]);
        let dependencies = vec![(genesis, request_summary.id(), 1_000, true)];

        assert!(
            store_future_if_needed(
                &mut connection,
                &envelope,
                request_summary.id(),
                genesis,
                &dependencies,
                1_000,
            )
            .unwrap()
        );
        let counts: (i64, i64, i64, i64) = connection
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM future_membership_operations),
                    (SELECT COUNT(*) FROM membership_operations),
                    (SELECT epoch FROM workspace_membership_state),
                    (SELECT COUNT(*) FROM membership_snapshot_members)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(counts, (1, 1, 0, 1));
        drop(connection);

        let reopened = crate::storage::open(&directory).unwrap();
        let future_count: i64 = reopened
            .connection
            .query_row(
                "SELECT COUNT(*) FROM future_membership_operations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(future_count, 1);
        drop(reopened);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn malformed_genesis_is_rejected_before_or_by_strong_remove() {
        let workspace = WorkspaceId::from_bytes([1; 16]);
        let dependency = MembershipOperationId::digest(b"missing");
        let genesis = MembershipEnvelope::sign(
            &secret(3),
            workspace,
            1_000,
            MembershipAction::Create,
            vec![dependency],
        )
        .unwrap();
        assert_eq!(
            process_genesis(&genesis),
            Err(StorageError::CorruptWorkspace)
        );
    }
}
