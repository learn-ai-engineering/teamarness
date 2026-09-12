use std::collections::{BTreeMap, BTreeSet};

use p2panda_auth::group::GroupCrdt;
use p2panda_auth::group::resolver::StrongRemove;
use p2panda_core::Hash;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::error::{MembershipError, StorageError};
use crate::{
    MemberSnapshot, MembershipEpoch, MembershipOperationId, MembershipRole, MembershipSnapshot,
    MembershipSnapshotDigest, MembershipVersion, PublicIdentity, WorkspaceId,
};

use super::{
    AuthId, MembershipAction, MembershipEnvelope, OperationAdapter, authenticate, validate_context,
};

const VERSION_DOMAIN: &[u8] = b"resonance.membership-version/v1";
const SNAPSHOT_DOMAIN: &[u8] = b"resonance.membership-snapshot/v1";

type Resolver = StrongRemove<AuthId, Hash, OperationAdapter, ()>;
type MembershipCrdt = GroupCrdt<AuthId, Hash, OperationAdapter, (), Resolver>;
type OperationRow = (Vec<u8>, Vec<u8>, i64, i64, Vec<u8>);
type WorkspaceRow = (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>);
type WorkspaceAnchor = (WorkspaceId, PublicIdentity, MembershipOperationId, Vec<u8>);
type FutureRow = (Vec<u8>, Vec<u8>, Vec<u8>, i64, Vec<u8>);

#[derive(Clone)]
pub(crate) struct WorkspaceProjection {
    workspace_id: WorkspaceId,
    creator: PublicIdentity,
    genesis_id: MembershipOperationId,
    operations: BTreeMap<MembershipOperationId, MembershipEnvelope>,
    snapshot: MembershipSnapshot,
}

impl WorkspaceProjection {
    pub(crate) fn snapshot(&self) -> MembershipSnapshot {
        self.snapshot.clone()
    }

    pub(crate) fn creator(&self) -> PublicIdentity {
        self.creator
    }

    pub(crate) fn contains_member(&self, identity: PublicIdentity) -> bool {
        self.snapshot
            .members()
            .iter()
            .any(|member| member.identity() == identity)
    }

    pub(crate) fn heads(&self) -> Vec<MembershipOperationId> {
        let dependencies = self
            .operations
            .values()
            .flat_map(|operation| operation.dependencies.iter().copied())
            .collect::<BTreeSet<_>>();
        self.operations
            .keys()
            .filter(|id| !dependencies.contains(id))
            .copied()
            .collect()
    }
}

#[derive(Debug)]
pub(crate) struct AppliedMembership {
    pub(crate) operation_id: MembershipOperationId,
    pub(crate) snapshot: MembershipSnapshot,
    pub(crate) snapshot_digest: MembershipSnapshotDigest,
    pub(crate) accepted: bool,
    pub(crate) quarantined: bool,
}

pub(crate) fn recover(
    connection: &mut Connection,
    now_unix_ms: i64,
) -> Result<BTreeMap<WorkspaceId, WorkspaceProjection>, StorageError> {
    let mut projections = load_and_replay(connection)?;
    repair_all_derived(connection, &projections)?;
    maintain_quarantine(connection, &mut projections, now_unix_ms)
        .map_err(membership_to_storage)?;
    Ok(projections)
}

pub(crate) fn maintain_quarantine(
    connection: &mut Connection,
    projections: &mut BTreeMap<WorkspaceId, WorkspaceProjection>,
    now_unix_ms: i64,
) -> Result<(), MembershipError> {
    loop {
        let rows = load_pending_future(connection)?;
        if rows.is_empty() {
            return Ok(());
        }
        let pending_ids = rows
            .iter()
            .filter_map(|(id, _, _, _, _)| operation_id(id).ok())
            .collect::<BTreeSet<_>>();
        let mut progressed = false;

        for (stored_id, stored_workspace, stored_author, stored_timestamp, bytes) in rows {
            let id = match operation_id(&stored_id) {
                Ok(id) => id,
                Err(_) => {
                    reject_future(connection, &stored_id, "invalid operation id")?;
                    progressed = true;
                    continue;
                }
            };
            let envelope = match authenticate(&bytes) {
                Ok(envelope)
                    if envelope.id == id
                        && envelope.workspace_id.as_bytes() == stored_workspace.as_slice()
                        && envelope.author.as_bytes() == stored_author.as_slice()
                        && envelope.authored_at_unix_ms == stored_timestamp =>
                {
                    envelope
                }
                _ => {
                    reject_future(connection, &stored_id, "authentication or indexes failed")?;
                    progressed = true;
                    continue;
                }
            };
            let stored_dependencies = load_future_dependencies(connection, id)?;
            if stored_dependencies != envelope.dependencies {
                reject_future(connection, &stored_id, "dependency indexes failed")?;
                progressed = true;
                continue;
            }
            let Some(projection) = projections.get(&envelope.workspace_id) else {
                reject_future(connection, &stored_id, "workspace not found")?;
                progressed = true;
                continue;
            };
            let unresolved = envelope.dependencies.iter().any(|dependency| {
                !projection.operations.contains_key(dependency) && pending_ids.contains(dependency)
            });
            if unresolved {
                continue;
            }
            if envelope.authored_at_unix_ms > now_unix_ms.saturating_add(5 * 60 * 1_000) {
                continue;
            }

            match apply_authenticated(connection, projections, envelope, now_unix_ms, Some(id)) {
                Ok(_) => progressed = true,
                Err(error) if is_terminal(&error) => {
                    reject_future(connection, &stored_id, terminal_reason(&error))?;
                    progressed = true;
                }
                Err(error) => return Err(error),
            }
        }

        if !progressed {
            return Ok(());
        }
    }
}

pub(crate) fn apply_canonical(
    connection: &mut Connection,
    projections: &mut BTreeMap<WorkspaceId, WorkspaceProjection>,
    canonical_envelope: &[u8],
    now_unix_ms: i64,
) -> Result<AppliedMembership, MembershipError> {
    let envelope = authenticate(canonical_envelope)?;
    apply_authenticated(connection, projections, envelope, now_unix_ms, None)
}

fn apply_authenticated(
    connection: &mut Connection,
    projections: &mut BTreeMap<WorkspaceId, WorkspaceProjection>,
    envelope: MembershipEnvelope,
    now_unix_ms: i64,
    future_id: Option<MembershipOperationId>,
) -> Result<AppliedMembership, MembershipError> {
    let projection = projections
        .get(&envelope.workspace_id)
        .ok_or(MembershipError::WorkspaceNotFound)?;

    if let Some(existing) = projection.operations.get(&envelope.id) {
        if existing.canonical_bytes() != envelope.canonical_bytes() {
            return Err(MembershipError::ContentHashMismatch);
        }
        if let Some(id) = future_id {
            remove_future(connection, id)?;
        }
        return Ok(applied_result(projection, envelope.id, false, false));
    }

    let dependency_context = dependency_context(connection, projection, &envelope)?;
    match validate_context(
        &envelope,
        projection.workspace_id,
        projection.genesis_id,
        &dependency_context,
        now_unix_ms,
    ) {
        Err(MembershipError::FutureTimestamp) if future_id.is_none() => {
            store_future(connection, &envelope)?;
            return Ok(applied_result(projection, envelope.id, false, true));
        }
        Err(error) => return Err(error),
        Ok(()) => {}
    }

    let mut candidate_operations = projection.operations.clone();
    candidate_operations.insert(envelope.id, envelope.clone());
    let candidate = replay(
        projection.workspace_id,
        projection.creator,
        projection.genesis_id,
        candidate_operations,
    )?;

    persist_candidate(connection, &envelope, &candidate, future_id)?;
    let result = applied_result(&candidate, envelope.id, true, false);
    projections.insert(envelope.workspace_id, candidate);
    Ok(result)
}

fn applied_result(
    projection: &WorkspaceProjection,
    operation_id: MembershipOperationId,
    accepted: bool,
    quarantined: bool,
) -> AppliedMembership {
    AppliedMembership {
        operation_id,
        snapshot: projection.snapshot(),
        snapshot_digest: snapshot_digest(&projection.snapshot),
        accepted,
        quarantined,
    }
}

fn load_and_replay(
    connection: &Connection,
) -> Result<BTreeMap<WorkspaceId, WorkspaceProjection>, StorageError> {
    let anchors = load_anchors(connection)?;
    let mut projections = BTreeMap::new();
    for (workspace_id, creator, genesis_id, genesis_bytes) in anchors {
        let operations = load_operations(connection, workspace_id)?;
        let genesis = operations
            .get(&genesis_id)
            .ok_or(StorageError::CorruptWorkspace)?;
        if genesis.canonical_bytes() != genesis_bytes {
            return Err(StorageError::CorruptWorkspace);
        }
        let projection = replay(workspace_id, creator, genesis_id, operations)
            .map_err(|_| StorageError::CorruptWorkspace)?;
        projections.insert(workspace_id, projection);
    }

    let operation_workspace_count: i64 = connection
        .query_row(
            "SELECT COUNT(DISTINCT workspace_id) FROM membership_operations",
            [],
            |row| row.get(0),
        )
        .map_err(|_| StorageError::Database)?;
    if operation_workspace_count
        != i64::try_from(projections.len()).map_err(|_| StorageError::CorruptWorkspace)?
    {
        return Err(StorageError::CorruptWorkspace);
    }
    Ok(projections)
}

fn load_anchors(connection: &Connection) -> Result<Vec<WorkspaceAnchor>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT workspace_id, creator_public_identity, genesis_operation_id, genesis_envelope
             FROM workspaces ORDER BY workspace_id",
        )
        .map_err(|_| StorageError::Database)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })
        .map_err(|_| StorageError::Database)?;
    let mut anchors = Vec::new();
    for row in rows {
        let (workspace, creator, genesis, bytes): WorkspaceRow =
            row.map_err(|_| StorageError::Database)?;
        anchors.push((
            WorkspaceId::from_bytes(fixed(&workspace)?),
            PublicIdentity::from_bytes(fixed(&creator)?)
                .map_err(|_| StorageError::CorruptWorkspace)?,
            MembershipOperationId::from_hash(Hash::from_bytes(fixed(&genesis)?)),
            bytes,
        ));
    }
    Ok(anchors)
}

fn load_operations(
    connection: &Connection,
    workspace_id: WorkspaceId,
) -> Result<BTreeMap<MembershipOperationId, MembershipEnvelope>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT operation_id, author_public_identity, authored_at_unix_ms,
                    action, canonical_envelope
             FROM membership_operations WHERE workspace_id = ?1 ORDER BY operation_id",
        )
        .map_err(|_| StorageError::Database)?;
    let rows = statement
        .query_map([workspace_id.as_bytes().as_slice()], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Vec<u8>>(4)?,
            ))
        })
        .map_err(|_| StorageError::Database)?;
    let mut operations = BTreeMap::new();
    for row in rows {
        let (stored_id, stored_author, stored_timestamp, stored_action, bytes): OperationRow =
            row.map_err(|_| StorageError::Database)?;
        let envelope = authenticate(&bytes).map_err(|_| StorageError::CorruptWorkspace)?;
        let expected_action = action_tag(&envelope.action);
        if envelope.workspace_id != workspace_id
            || envelope.id.as_bytes() != stored_id.as_slice()
            || envelope.author.as_bytes() != stored_author.as_slice()
            || envelope.authored_at_unix_ms != stored_timestamp
            || expected_action != stored_action
        {
            return Err(StorageError::CorruptWorkspace);
        }
        let stored_dependencies = load_dependencies(connection, envelope.id)?;
        if stored_dependencies != envelope.dependencies {
            return Err(StorageError::CorruptWorkspace);
        }
        if operations.insert(envelope.id, envelope).is_some() {
            return Err(StorageError::CorruptWorkspace);
        }
    }
    Ok(operations)
}

fn load_dependencies(
    connection: &Connection,
    operation_id: MembershipOperationId,
) -> Result<Vec<MembershipOperationId>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT dependency_operation_id FROM membership_dependencies
             WHERE operation_id = ?1 ORDER BY dependency_operation_id",
        )
        .map_err(|_| StorageError::Database)?;
    let rows = statement
        .query_map([operation_id.as_bytes().as_slice()], |row| {
            row.get::<_, Vec<u8>>(0)
        })
        .map_err(|_| StorageError::Database)?;
    rows.map(|row| {
        let bytes = row.map_err(|_| StorageError::Database)?;
        Ok(MembershipOperationId::from_hash(Hash::from_bytes(fixed(
            &bytes,
        )?)))
    })
    .collect()
}

fn replay(
    workspace_id: WorkspaceId,
    creator: PublicIdentity,
    genesis_id: MembershipOperationId,
    operations: BTreeMap<MembershipOperationId, MembershipEnvelope>,
) -> Result<WorkspaceProjection, MembershipError> {
    let genesis = operations
        .get(&genesis_id)
        .ok_or(MembershipError::ConflictingGenesis)?;
    if genesis.workspace_id != workspace_id
        || genesis.author != creator
        || !genesis.dependencies.is_empty()
        || !matches!(genesis.action, MembershipAction::Create)
    {
        return Err(MembershipError::ConflictingGenesis);
    }
    if operations.values().any(|operation| {
        matches!(operation.action, MembershipAction::Create) && operation.id != genesis_id
    }) {
        return Err(MembershipError::ConflictingGenesis);
    }

    let mut state =
        MembershipCrdt::process(MembershipCrdt::init(), &OperationAdapter(genesis.clone()))
            .map_err(|_| MembershipError::AuthorizationRejected)?;
    let mut processed = BTreeSet::from([genesis_id]);

    while processed.len() < operations.len() {
        let ready = operations
            .values()
            .filter(|operation| {
                !processed.contains(&operation.id)
                    && operation
                        .dependencies
                        .iter()
                        .all(|dependency| processed.contains(dependency))
            })
            .map(|operation| operation.id)
            .collect::<Vec<_>>();
        if ready.is_empty() {
            return Err(MembershipError::MissingDependency);
        }

        for id in ready {
            let operation = operations.get(&id).expect("ready operation exists");
            let dependencies = operation
                .dependencies
                .iter()
                .map(|dependency| {
                    let dependency_operation =
                        operations.get(dependency).expect("ready dependency exists");
                    (
                        *dependency,
                        dependency_operation.workspace_id,
                        dependency_operation.authored_at_unix_ms,
                        true,
                    )
                })
                .collect::<Vec<_>>();
            validate_context(operation, workspace_id, genesis_id, &dependencies, i64::MAX)?;
            validate_phase_one(operation, creator, &operations)?;
            state = MembershipCrdt::process(state, &OperationAdapter(operation.clone()))
                .map_err(|_| MembershipError::AuthorizationRejected)?;
            processed.insert(id);
        }
    }

    let mut members = state
        .members(AuthId::workspace(workspace_id))
        .into_iter()
        .map(|(id, access)| {
            let AuthId::Installation(bytes) = id else {
                return Err(MembershipError::InvalidRole);
            };
            let identity =
                PublicIdentity::from_bytes(bytes).map_err(|_| MembershipError::InvalidRole)?;
            let role = if access.is_manage() {
                MembershipRole::Manager
            } else if access.is_write() {
                MembershipRole::Member
            } else {
                return Err(MembershipError::InvalidRole);
            };
            Ok(MemberSnapshot::new(identity, role))
        })
        .collect::<Result<Vec<_>, _>>()?;
    members.sort_by_key(MemberSnapshot::identity);
    if members
        .iter()
        .filter(|member| member.role() == MembershipRole::Manager)
        .map(MemberSnapshot::identity)
        .collect::<Vec<_>>()
        != vec![creator]
    {
        return Err(MembershipError::NotManager);
    }

    let epoch = u64::try_from(operations.len().saturating_sub(1))
        .map_err(|_| MembershipError::AuthorizationRejected)?;
    let version = membership_version(workspace_id, genesis_id, operations.keys().copied());
    let snapshot =
        MembershipSnapshot::new(workspace_id, MembershipEpoch::new(epoch), version, members);
    Ok(WorkspaceProjection {
        workspace_id,
        creator,
        genesis_id,
        operations,
        snapshot,
    })
}

fn validate_phase_one(
    operation: &MembershipEnvelope,
    creator: PublicIdentity,
    operations: &BTreeMap<MembershipOperationId, MembershipEnvelope>,
) -> Result<(), MembershipError> {
    if operation.author != creator {
        return Err(MembershipError::NotManager);
    }
    match &operation.action {
        MembershipAction::Create => Err(MembershipError::ConflictingGenesis),
        MembershipAction::Admit { target, proof } => {
            if proof.claim.token.inviter != operation.author
                || proof.claim.token.workspace_id != operation.workspace_id
                || proof.claim.token.issued_at_unix_ms > operation.authored_at_unix_ms
                || proof.claim.token.expires_at_unix_ms < operation.authored_at_unix_ms
                || proof.claim.invitee != *target
            {
                return Err(MembershipError::InvalidAdmissionProof);
            }
            if causal_ancestry_contains_revoke(operation, *target, operations) {
                return Err(MembershipError::CausalReadmission);
            }
            Ok(())
        }
        MembershipAction::Revoke { target } => {
            if *target == creator {
                return Err(MembershipError::CannotRevokeManager);
            }
            Ok(())
        }
    }
}

fn causal_ancestry_contains_revoke(
    operation: &MembershipEnvelope,
    target: PublicIdentity,
    operations: &BTreeMap<MembershipOperationId, MembershipEnvelope>,
) -> bool {
    let mut pending = operation.dependencies.clone();
    let mut visited = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !visited.insert(id) {
            continue;
        }
        let Some(ancestor) = operations.get(&id) else {
            continue;
        };
        if matches!(ancestor.action, MembershipAction::Revoke { target: revoked } if revoked == target)
        {
            return true;
        }
        pending.extend(ancestor.dependencies.iter().copied());
    }
    false
}

fn dependency_context(
    connection: &Connection,
    projection: &WorkspaceProjection,
    envelope: &MembershipEnvelope,
) -> Result<Vec<(MembershipOperationId, WorkspaceId, i64, bool)>, MembershipError> {
    envelope
        .dependencies
        .iter()
        .map(|id| {
            if let Some(dependency) = projection.operations.get(id) {
                return Ok((
                    *id,
                    dependency.workspace_id,
                    dependency.authored_at_unix_ms,
                    true,
                ));
            }
            let future: Option<Vec<u8>> = connection
                .query_row(
                    "SELECT canonical_envelope FROM future_membership_operations
                     WHERE operation_id = ?1 AND rejection_reason IS NULL",
                    [id.as_bytes().as_slice()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| MembershipError::Storage(StorageError::Database))?;
            let bytes = future.ok_or(MembershipError::MissingDependency)?;
            let dependency = authenticate(&bytes)?;
            if dependency.id != *id {
                return Err(MembershipError::ContentHashMismatch);
            }
            Ok((
                *id,
                dependency.workspace_id,
                dependency.authored_at_unix_ms,
                true,
            ))
        })
        .collect()
}

fn persist_candidate(
    connection: &mut Connection,
    envelope: &MembershipEnvelope,
    candidate: &WorkspaceProjection,
    future_id: Option<MembershipOperationId>,
) -> Result<(), MembershipError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    transaction
        .execute(
            "INSERT INTO membership_operations (
                operation_id, workspace_id, author_public_identity,
                authored_at_unix_ms, action, canonical_envelope
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                envelope.id.as_bytes().as_slice(),
                envelope.workspace_id.as_bytes().as_slice(),
                envelope.author.as_bytes().as_slice(),
                envelope.authored_at_unix_ms,
                action_tag(&envelope.action),
                envelope.canonical_bytes(),
            ],
        )
        .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    for dependency in &envelope.dependencies {
        transaction
            .execute(
                "INSERT INTO membership_dependencies (operation_id, dependency_operation_id)
                 VALUES (?1, ?2)",
                params![
                    envelope.id.as_bytes().as_slice(),
                    dependency.as_bytes().as_slice()
                ],
            )
            .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    }
    if let MembershipAction::Admit { target, .. } | MembershipAction::Revoke { target } =
        &envelope.action
    {
        transaction
            .execute(
                "INSERT INTO identity_subjects (installation_public_identity, human_identity_id)
                 VALUES (?1, NULL) ON CONFLICT(installation_public_identity) DO NOTHING",
                [target.as_bytes().as_slice()],
            )
            .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    }
    write_derived(&transaction, candidate)?;
    if let Some(id) = future_id {
        transaction
            .execute(
                "DELETE FROM future_membership_dependencies WHERE operation_id = ?1",
                [id.as_bytes().as_slice()],
            )
            .map_err(|_| MembershipError::Storage(StorageError::Database))?;
        transaction
            .execute(
                "DELETE FROM future_membership_operations WHERE operation_id = ?1",
                [id.as_bytes().as_slice()],
            )
            .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    }
    transaction
        .commit()
        .map_err(|_| MembershipError::Storage(StorageError::UncertainOutcome))
}

fn repair_all_derived(
    connection: &mut Connection,
    projections: &BTreeMap<WorkspaceId, WorkspaceProjection>,
) -> Result<(), StorageError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| StorageError::Database)?;
    for projection in projections.values() {
        write_derived(&transaction, projection).map_err(membership_to_storage)?;
    }
    transaction
        .commit()
        .map_err(|_| StorageError::UncertainOutcome)
}

fn write_derived(
    transaction: &Transaction<'_>,
    projection: &WorkspaceProjection,
) -> Result<(), MembershipError> {
    let epoch = i64::try_from(projection.snapshot.epoch().value())
        .map_err(|_| MembershipError::Storage(StorageError::CorruptWorkspace))?;
    transaction
        .execute(
            "INSERT INTO workspace_membership_state (workspace_id, epoch, membership_version)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(workspace_id) DO UPDATE SET
                 epoch = excluded.epoch,
                 membership_version = excluded.membership_version",
            params![
                projection.workspace_id.as_bytes().as_slice(),
                epoch,
                projection.snapshot.version().as_bytes().as_slice(),
            ],
        )
        .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    transaction
        .execute(
            "DELETE FROM membership_snapshot_members WHERE workspace_id = ?1",
            [projection.workspace_id.as_bytes().as_slice()],
        )
        .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    for member in projection.snapshot.members() {
        transaction
            .execute(
                "INSERT INTO membership_snapshot_members (workspace_id, member_identity, role)
                 VALUES (?1, ?2, ?3)",
                params![
                    projection.workspace_id.as_bytes().as_slice(),
                    member.identity().as_bytes().as_slice(),
                    match member.role() {
                        MembershipRole::Member => 0,
                        MembershipRole::Manager => 1,
                    },
                ],
            )
            .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    }
    Ok(())
}

fn store_future(
    connection: &mut Connection,
    envelope: &MembershipEnvelope,
) -> Result<(), MembershipError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    let existing: Option<Vec<u8>> = transaction
        .query_row(
            "SELECT canonical_envelope FROM future_membership_operations WHERE operation_id = ?1",
            [envelope.id.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    match existing {
        Some(bytes) if bytes == envelope.canonical_bytes() => return Ok(()),
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
        .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    for dependency in &envelope.dependencies {
        transaction
            .execute(
                "INSERT INTO future_membership_dependencies (operation_id, dependency_operation_id)
                 VALUES (?1, ?2)",
                params![
                    envelope.id.as_bytes().as_slice(),
                    dependency.as_bytes().as_slice()
                ],
            )
            .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    }
    transaction
        .commit()
        .map_err(|_| MembershipError::Storage(StorageError::UncertainOutcome))
}

fn load_pending_future(connection: &Connection) -> Result<Vec<FutureRow>, MembershipError> {
    let mut statement = connection
        .prepare(
            "SELECT operation_id, workspace_id, author_public_identity,
                    authored_at_unix_ms, canonical_envelope
             FROM future_membership_operations
             WHERE rejection_reason IS NULL ORDER BY authored_at_unix_ms, operation_id",
        )
        .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| MembershipError::Storage(StorageError::Database))
}

fn load_future_dependencies(
    connection: &Connection,
    id: MembershipOperationId,
) -> Result<Vec<MembershipOperationId>, MembershipError> {
    let mut statement = connection
        .prepare(
            "SELECT dependency_operation_id FROM future_membership_dependencies
             WHERE operation_id = ?1 ORDER BY dependency_operation_id",
        )
        .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    let rows = statement
        .query_map([id.as_bytes().as_slice()], |row| row.get::<_, Vec<u8>>(0))
        .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    rows.map(|row| {
        let bytes = row.map_err(|_| MembershipError::Storage(StorageError::Database))?;
        operation_id(&bytes)
    })
    .collect()
}

fn reject_future(
    connection: &Connection,
    operation_id: &[u8],
    reason: &str,
) -> Result<(), MembershipError> {
    connection
        .execute(
            "UPDATE future_membership_operations SET rejection_reason = ?2
             WHERE operation_id = ?1 AND rejection_reason IS NULL",
            params![operation_id, reason],
        )
        .map(|_| ())
        .map_err(|_| MembershipError::Storage(StorageError::Database))
}

fn remove_future(
    connection: &mut Connection,
    operation_id: MembershipOperationId,
) -> Result<(), MembershipError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    transaction
        .execute(
            "DELETE FROM future_membership_dependencies WHERE operation_id = ?1",
            [operation_id.as_bytes().as_slice()],
        )
        .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    transaction
        .execute(
            "DELETE FROM future_membership_operations WHERE operation_id = ?1",
            [operation_id.as_bytes().as_slice()],
        )
        .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    transaction
        .commit()
        .map_err(|_| MembershipError::Storage(StorageError::UncertainOutcome))
}

fn membership_version(
    workspace_id: WorkspaceId,
    genesis_id: MembershipOperationId,
    operation_ids: impl IntoIterator<Item = MembershipOperationId>,
) -> MembershipVersion {
    let ids = operation_ids.into_iter().collect::<BTreeSet<_>>();
    let mut bytes = Vec::with_capacity(VERSION_DOMAIN.len() + 16 + 32 + ids.len() * 32);
    bytes.extend_from_slice(VERSION_DOMAIN);
    bytes.extend_from_slice(workspace_id.as_bytes());
    bytes.extend_from_slice(genesis_id.as_bytes());
    for id in ids {
        bytes.extend_from_slice(id.as_bytes());
    }
    MembershipVersion::from_bytes(*Hash::digest(bytes).as_bytes())
}

pub(crate) fn snapshot_digest(snapshot: &MembershipSnapshot) -> MembershipSnapshotDigest {
    let mut bytes = Vec::with_capacity(SNAPSHOT_DOMAIN.len() + 16 + snapshot.members().len() * 33);
    bytes.extend_from_slice(SNAPSHOT_DOMAIN);
    bytes.extend_from_slice(snapshot.workspace_id().as_bytes());
    for member in snapshot.members() {
        bytes.extend_from_slice(member.identity().as_bytes());
        bytes.push(match member.role() {
            MembershipRole::Member => 0,
            MembershipRole::Manager => 1,
        });
    }
    MembershipSnapshotDigest::from_bytes(*Hash::digest(bytes).as_bytes())
}

fn action_tag(action: &MembershipAction) -> i64 {
    match action {
        MembershipAction::Create => 0,
        MembershipAction::Admit { .. } => 1,
        MembershipAction::Revoke { .. } => 2,
    }
}

fn operation_id(bytes: &[u8]) -> Result<MembershipOperationId, MembershipError> {
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| MembershipError::ContentHashMismatch)?;
    Ok(MembershipOperationId::from_hash(Hash::from_bytes(bytes)))
}

fn fixed<const N: usize>(bytes: &[u8]) -> Result<[u8; N], StorageError> {
    bytes.try_into().map_err(|_| StorageError::CorruptWorkspace)
}

fn membership_to_storage(error: MembershipError) -> StorageError {
    match error {
        MembershipError::Storage(error) => error,
        _ => StorageError::CorruptWorkspace,
    }
}

fn is_terminal(error: &MembershipError) -> bool {
    !matches!(
        error,
        MembershipError::FutureTimestamp
            | MembershipError::Storage(_)
            | MembershipError::RuntimeUnavailable
    )
}

fn terminal_reason(error: &MembershipError) -> &'static str {
    match error {
        MembershipError::MissingDependency => "missing dependency",
        MembershipError::WorkspaceMismatch => "workspace mismatch",
        MembershipError::CrossWorkspaceDependency => "cross-workspace dependency",
        MembershipError::NonDescendantDependency => "non-descendant dependency",
        MembershipError::NonMonotonicTimestamp => "non-monotonic timestamp",
        MembershipError::NotManager => "author is not manager",
        MembershipError::InvalidAdmissionProof => "invalid admission proof",
        MembershipError::CausalReadmission => "causal re-admission",
        MembershipError::CannotRevokeManager => "cannot revoke manager",
        MembershipError::AuthorizationRejected => "authorization rejected",
        _ => "invalid membership operation",
    }
}
