use iroh_base::SecretKey;
use p2panda_auth::group::GroupCrdt;
use p2panda_auth::group::resolver::StrongRemove;
use p2panda_core::Hash;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::diag::fault;
use crate::error::{MembershipError, StorageError, WorkspaceError};
use crate::membership::{
    AuthId, MembershipAction, MembershipEnvelope, OperationAdapter, validate_context,
};
use crate::{
    MembershipOperationId, MembershipVersion, PublicIdentity, WorkspaceId, WorkspaceSummary,
};

const MAX_WORKSPACE_NAME_CHARS: usize = 64;

type Resolver = StrongRemove<AuthId, Hash, OperationAdapter, ()>;
type MembershipCrdt = GroupCrdt<AuthId, Hash, OperationAdapter, (), Resolver>;
type StoredOperationRow = (Vec<u8>, Vec<u8>, i64, i64, Vec<u8>);
type CreationRequestRow = (
    Vec<u8>,
    Vec<u8>,
    String,
    i64,
    Vec<u8>,
    Vec<u8>,
    String,
    Option<String>,
);

#[derive(Clone)]
struct CreationRequest {
    workspace_id: WorkspaceId,
    creator: PublicIdentity,
    name: String,
    authored_at_unix_ms: i64,
    genesis_id: MembershipOperationId,
    genesis_envelope: Vec<u8>,
    status: String,
    initial_person_name: Option<String>,
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

pub(crate) fn create_named(
    connection: &mut Connection,
    secret_key: &SecretKey,
    workspace_name: &str,
    person_name: &str,
    authored_at_unix_ms: i64,
) -> Result<WorkspaceSummary, WorkspaceError> {
    crate::person_names::validate(person_name).map_err(|_| WorkspaceError::InvalidName)?;
    create_with_initial_name(
        connection,
        secret_key,
        workspace_name,
        Some(person_name),
        authored_at_unix_ms,
        |_| Ok(()),
    )
}

#[cfg(test)]
fn create_named_with_test_fault(
    connection: &mut Connection,
    secret_key: &SecretKey,
    workspace_name: &str,
    person_name: &str,
    authored_at_unix_ms: i64,
    fault: impl FnMut(CreationBoundary) -> Result<(), StorageError>,
) -> Result<WorkspaceSummary, WorkspaceError> {
    crate::person_names::validate(person_name).map_err(|_| WorkspaceError::InvalidName)?;
    create_with_initial_name(
        connection,
        secret_key,
        workspace_name,
        Some(person_name),
        authored_at_unix_ms,
        fault,
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
    create_with_initial_name(
        connection,
        secret_key,
        name,
        None,
        authored_at_unix_ms,
        &mut fault,
    )
}

fn create_with_initial_name(
    connection: &mut Connection,
    secret_key: &SecretKey,
    name: &str,
    initial_person_name: Option<&str>,
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
        .map_err(fault!(WorkspaceError::Storage(StorageError::Database)))?;
    transaction
        .execute(
            "INSERT INTO workspace_creation_requests (
                workspace_id, creator_public_identity, name, authored_at_unix_ms,
                genesis_operation_id, genesis_envelope, status, initial_person_name
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'Pending', ?7)",
            params![
                workspace_id.as_bytes().as_slice(),
                creator.as_bytes().as_slice(),
                name,
                authored_at_unix_ms,
                genesis.id.as_bytes().as_slice(),
                genesis.canonical_bytes(),
                initial_person_name,
            ],
        )
        .map_err(|_| WorkspaceError::Conflict)?;
    transaction
        .commit()
        .map_err(fault!(WorkspaceError::Storage(
            StorageError::UncertainOutcome
        )))?;
    fault(CreationBoundary::AfterTransactionOne).map_err(workspace_from_storage)?;

    let request = CreationRequest {
        workspace_id,
        creator,
        name: name.to_owned(),
        authored_at_unix_ms,
        genesis_id: genesis.id,
        genesis_envelope: genesis.canonical_bytes().to_vec(),
        status: "Pending".to_owned(),
        initial_person_name: initial_person_name.map(str::to_owned),
    };
    finish_request_with_fault(connection, &request, secret_key, &mut fault)
        .map_err(workspace_from_storage)?;
    Ok(summary(&request))
}

pub(crate) fn list(connection: &Connection) -> Result<Vec<WorkspaceSummary>, WorkspaceError> {
    verify_ready(connection).map_err(workspace_from_storage)?;
    let mut statement = connection
        .prepare(
            "SELECT workspace_id, creator_public_identity, name, authored_at_unix_ms
             FROM workspaces ORDER BY authored_at_unix_ms, workspace_id",
        )
        .map_err(fault!(WorkspaceError::Storage(StorageError::Database)))?;
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

pub(crate) fn recover_pending(
    connection: &mut Connection,
    secret_key: &SecretKey,
) -> Result<(), StorageError> {
    let requests = load_requests(connection, Some("Pending"))?;
    for request in &requests {
        finish_request(connection, request, secret_key)?;
    }
    verify_ready(connection)
}

fn finish_request(
    connection: &mut Connection,
    request: &CreationRequest,
    secret_key: &SecretKey,
) -> Result<(), StorageError> {
    finish_request_with_fault(connection, request, secret_key, &mut |_| Ok(()))
}

/// The creator's own Person key group and its attachment to the Workspace.
///
/// Both are a pure function of the creator's key and the genesis, so a
/// recovered creation regenerates byte-identical operations.
fn genesis_companions(
    request: &CreationRequest,
    secret_key: &SecretKey,
) -> Result<(MembershipEnvelope, MembershipEnvelope), StorageError> {
    if secret_key.public().as_bytes() != request.creator.as_bytes() {
        return Err(StorageError::CorruptWorkspace);
    }
    let create_person = MembershipEnvelope::sign(
        secret_key,
        request.workspace_id,
        request.authored_at_unix_ms,
        MembershipAction::CreatePerson {
            creator: request.creator,
        },
        vec![request.genesis_id],
    )
    .map_err(fault!(StorageError::CorruptWorkspace))?;
    let attach = MembershipEnvelope::sign(
        secret_key,
        request.workspace_id,
        request.authored_at_unix_ms,
        MembershipAction::AttachPerson {
            group: create_person.id,
        },
        vec![create_person.id],
    )
    .map_err(fault!(StorageError::CorruptWorkspace))?;
    Ok((create_person, attach))
}

fn finish_request_with_fault(
    connection: &mut Connection,
    request: &CreationRequest,
    secret_key: &SecretKey,
    fault: &mut impl FnMut(CreationBoundary) -> Result<(), StorageError>,
) -> Result<(), StorageError> {
    let genesis = validate_genesis_request(request)?;
    process_genesis(&genesis)?;
    let (create_person, attach) = genesis_companions(request, secret_key)?;
    let version = crate::membership::projection::membership_version(
        request.workspace_id,
        request.genesis_id,
        [request.genesis_id, create_person.id, attach.id],
    );
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(fault!(StorageError::Database))?;

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
        transaction
            .execute(
                "INSERT INTO membership_sync_wakes(workspace_id, wake_generation) VALUES(?1, 0)",
                [request.workspace_id.as_bytes().as_slice()],
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
        .map_err(fault!(StorageError::Database))?;
    match existing_operation {
        Some(bytes) if bytes != request.genesis_envelope => {
            return Err(StorageError::CorruptWorkspace);
        }
        Some(_) => {
            // Genesis and its companions commit together, so both companions
            // must already be durable with the regenerated bytes.
            for companion in [&create_person, &attach] {
                let stored: Option<Vec<u8>> = transaction
                    .query_row(
                        "SELECT canonical_envelope FROM membership_operations WHERE operation_id = ?1",
                        [companion.id.as_bytes().as_slice()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(fault!(StorageError::Database))?;
                if stored.as_deref() != Some(companion.canonical_bytes()) {
                    return Err(StorageError::CorruptWorkspace);
                }
            }
        }
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
            // The creator's key group create and attachment are the first two
            // transitions, replayed exactly as any later operation would be.
            let mut operations = std::collections::BTreeMap::from([(genesis.id, genesis.clone())]);
            for companion in [&create_person, &attach] {
                operations.insert(companion.id, companion.clone());
                let prefix = crate::membership::projection::replay(
                    request.workspace_id,
                    request.creator,
                    request.genesis_id,
                    operations.clone(),
                )
                .map_err(fault!(StorageError::CorruptWorkspace))?;
                crate::membership::projection::persist_candidate_rows(
                    &transaction,
                    companion,
                    &prefix,
                )
                .map_err(|error| match error {
                    MembershipError::Storage(storage) => storage,
                    _ => StorageError::CorruptWorkspace,
                })?;
            }
        }
    }

    if let Some(name) = &request.initial_person_name {
        crate::person_names::validate(name).map_err(|_| StorageError::CorruptWorkspace)?;
        let person = crate::person_names::derive(request.workspace_id, request.genesis_id);
        transaction.execute(
            "INSERT INTO workspace_person_names (workspace_id, person_id, anchor_operation_id, owner_public_identity, name)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(workspace_id, person_id) DO NOTHING",
            params![request.workspace_id.as_bytes().as_slice(), person.as_bytes(),
                request.genesis_id.as_bytes().as_slice(), request.creator.as_bytes().as_slice(), name],
        ).map_err(fault!(StorageError::Database))?;
        transaction
            .execute(
                "INSERT INTO pending_person_name_edits (workspace_id, person_id, name)
             VALUES (?1, ?2, ?3) ON CONFLICT(workspace_id, person_id) DO NOTHING",
                params![
                    request.workspace_id.as_bytes().as_slice(),
                    person.as_bytes(),
                    name
                ],
            )
            .map_err(fault!(StorageError::Database))?;
    }
    insert_or_verify_state(&transaction, request, version)?;
    insert_or_verify_creator(&transaction, request)?;
    let updated = transaction
        .execute(
            "UPDATE workspace_creation_requests SET status = 'Complete'
             WHERE workspace_id = ?1 AND status = 'Pending'",
            [request.workspace_id.as_bytes().as_slice()],
        )
        .map_err(fault!(StorageError::Database))?;
    if updated > 1 {
        return Err(StorageError::CorruptWorkspace);
    }
    fault(CreationBoundary::DuringTransactionTwo)?;
    transaction
        .commit()
        .map_err(fault!(StorageError::UncertainOutcome))?;
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
        .map_err(fault!(StorageError::Database))?;
    // Genesis, the creator's group create and its attachment are epochs 0..=2.
    match stored {
        Some((2, stored_version)) if stored_version == version.as_bytes() => Ok(()),
        Some(_) => Err(StorageError::CorruptWorkspace),
        None => transaction
            .execute(
                "INSERT INTO workspace_membership_state (
                    workspace_id, epoch, membership_version
                 ) VALUES (?1, 2, ?2)",
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
        .map_err(fault!(StorageError::Database))?;
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
            .map_err(fault!(StorageError::Database))?;
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
            .map_err(fault!(StorageError::Database))?;
        if dependency_count != 0 {
            return Err(StorageError::CorruptWorkspace);
        }
    }
    let ready_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM workspaces", [], |row| row.get(0))
        .map_err(fault!(StorageError::Database))?;
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
                    genesis_operation_id, genesis_envelope, status, initial_person_name
             FROM workspace_creation_requests WHERE status = ?1 ORDER BY workspace_id"
        }
        None => {
            "SELECT workspace_id, creator_public_identity, name, authored_at_unix_ms,
                    genesis_operation_id, genesis_envelope, status, initial_person_name
             FROM workspace_creation_requests ORDER BY workspace_id"
        }
    };
    let mut statement = connection
        .prepare(sql)
        .map_err(fault!(StorageError::Database))?;
    let map_row = |row: &rusqlite::Row<'_>| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, Vec<u8>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, Vec<u8>>(4)?,
            row.get::<_, Vec<u8>>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, Option<String>>(7)?,
        ))
    };
    let mut requests = Vec::new();
    if let Some(status) = status {
        let rows = statement
            .query_map([status], map_row)
            .map_err(fault!(StorageError::Database))?;
        for row in rows {
            requests.push(request_from_row(
                row.map_err(fault!(StorageError::Database))?,
            )?);
        }
    } else {
        let rows = statement
            .query_map([], map_row)
            .map_err(fault!(StorageError::Database))?;
        for row in rows {
            requests.push(request_from_row(
                row.map_err(fault!(StorageError::Database))?,
            )?);
        }
    }
    Ok(requests)
}

fn request_from_row(row: CreationRequestRow) -> Result<CreationRequest, StorageError> {
    let (
        workspace,
        creator,
        name,
        authored_at,
        genesis_id,
        genesis_envelope,
        status,
        initial_person_name,
    ) = row;
    Ok(CreationRequest {
        workspace_id: WorkspaceId::from_bytes(fixed_bytes(&workspace)?),
        creator: PublicIdentity::from_bytes(fixed_bytes(&creator)?)
            .map_err(|_| StorageError::CorruptWorkspace)?,
        name,
        authored_at_unix_ms: authored_at,
        genesis_id: MembershipOperationId::from_hash(Hash::from_bytes(fixed_bytes(&genesis_id)?)),
        genesis_envelope,
        status,
        initial_person_name,
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
        .map_err(fault!(StorageError::Database))?;
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
                None,
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
            MembershipAction::Revoke {
                target,
                group: MembershipOperationId::digest(b"group"),
            },
            dependencies,
        )
        .unwrap()
    }

    #[test]
    fn interrupted_named_creation_recovers_original_person_and_name() {
        let directory = std::env::temp_dir().join(format!(
            "teamarness-named-creation-boundary-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        let runtime = crate::storage::open(&directory, 0).unwrap();
        let mut connection = runtime.connection;
        let creator = PublicIdentity::from_bytes(*runtime.secret_key.public().as_bytes()).unwrap();
        let result = create_named_with_test_fault(
            &mut connection,
            &runtime.secret_key,
            "Named fault",
            "Initial 🧑‍💻",
            42,
            |boundary| {
                if boundary == CreationBoundary::AfterTransactionOne {
                    Err(StorageError::Database)
                } else {
                    Ok(())
                }
            },
        );
        assert!(result.is_err());
        drop(connection);
        let mut reopened = crate::storage::open(&directory, 0).unwrap();
        let workspace = list(&reopened.connection).unwrap().pop().unwrap();
        let projection = crate::membership::projection::recover(&mut reopened.connection, 0)
            .unwrap()
            .remove(&workspace.id())
            .unwrap();
        let person = crate::person_names::lookup(&reopened.connection, &projection, creator)
            .unwrap()
            .unwrap();
        assert_eq!(person.name(), Some("Initial 🧑‍💻"));
        assert_eq!(
            person.id(),
            crate::person_names::derive(workspace.id(), projection.genesis_id())
        );
        drop(reopened);
        fs::remove_dir_all(directory).unwrap();
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
                "teamarness-creation-boundary-{}-{index}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&directory);
            let runtime = crate::storage::open(&directory, 0).unwrap();
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

            let reopened = crate::storage::open(&directory, 0).unwrap();
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
                // Genesis plus the creator's group create and attachment.
                assert_eq!(
                    recovered,
                    (workspace_id, pending_bytes, pending_timestamp, 3)
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
            std::env::temp_dir().join(format!("teamarness-future-context-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        let runtime = crate::storage::open(&directory, 0).unwrap();
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
        assert_eq!(counts, (1, 3, 2, 1));
        drop(connection);

        let reopened = crate::storage::open(&directory, 0).unwrap();
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
