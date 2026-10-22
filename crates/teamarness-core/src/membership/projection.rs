use std::collections::{BTreeMap, BTreeSet};

use iroh_base::SecretKey;
use p2panda_auth::group::GroupCrdt;
use p2panda_auth::group::resolver::StrongRemove;
use p2panda_core::Hash;
use rusqlite::{
    Connection, ErrorCode, OptionalExtension, Transaction, TransactionBehavior, params,
};

use crate::error::{InvitationError, MembershipError, StorageError};
use crate::invitation::{VerifiedInvitationClaim, VerifiedInvitationToken};
use crate::onboarding::wire::{
    CodecError, MAX_OPERATIONS, MAX_RESPONSE_BODY_BYTES, SuccessResponse,
    encode_success_with_limits,
};
use crate::{
    MemberSnapshot, MembershipChange, MembershipEpoch, MembershipOperationId, MembershipRole,
    MembershipSnapshot, MembershipSnapshotDigest, MembershipVersion, PublicIdentity, WorkspaceId,
};

use super::authority;
use super::{
    AdmissionProof, AuthId, Ledger, MembershipAction, MembershipEnvelope, OperationAdapter,
    authenticate, authenticate_canonical_copy, validate_context,
};

const VERSION_DOMAIN: &[u8] = b"teamarness.membership-version/v1";
const SNAPSHOT_DOMAIN: &[u8] = b"teamarness.membership-snapshot/v1";

type Resolver = StrongRemove<AuthId, Hash, OperationAdapter, ()>;
type MembershipCrdt = GroupCrdt<AuthId, Hash, OperationAdapter, (), Resolver>;
type OperationRow = (Vec<u8>, Vec<u8>, i64, i64, Vec<u8>);
type WorkspaceRow = (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>);
type WorkspaceAnchor = (WorkspaceId, PublicIdentity, MembershipOperationId, Vec<u8>);
type FutureRow = (Vec<u8>, Vec<u8>, Vec<u8>, i64, Vec<u8>);
type CreationRequestEvidence = (Vec<u8>, String, i64, Vec<u8>, Vec<u8>, String);

pub(crate) type PrincipalAnchor = (
    MembershipOperationId,
    PublicIdentity,
    Option<(MembershipOperationId, PublicIdentity)>,
);

#[derive(Clone)]
pub(crate) struct WorkspaceProjection {
    pub(super) workspace_id: WorkspaceId,
    pub(super) creator: PublicIdentity,
    pub(super) genesis_id: MembershipOperationId,
    pub(super) operations: BTreeMap<MembershipOperationId, MembershipEnvelope>,
    snapshot: MembershipSnapshot,
    /// Every key ever added as an Agent key. Agent keys are Members but never
    /// peers: grants, key demands and sessions address Installation keys only.
    agent_keys: BTreeSet<PublicIdentity>,
    /// Keys with multiple independently accepted admission anchors. This is
    /// derived from signed facts, not the order in which peers saw them.
    conflicted_keys: BTreeSet<PublicIdentity>,
}

impl WorkspaceProjection {
    pub(crate) fn snapshot(&self) -> MembershipSnapshot {
        self.snapshot.clone()
    }

    /// The current Members that are Installation keys: the creator and every
    /// admitted Person's key. These are the only keys that connect, receive
    /// Protection Grants, or are asked for Content Keys.
    pub(crate) fn installation_members(&self) -> Vec<PublicIdentity> {
        self.snapshot
            .members()
            .iter()
            .map(MemberSnapshot::identity)
            .filter(|identity| !self.agent_keys.contains(identity))
            .collect()
    }

    /// The currently admitted Person key group managed by `identity`, if any.
    pub(crate) fn person_group(&self, identity: PublicIdentity) -> Option<MembershipOperationId> {
        let revoked = self
            .operations
            .values()
            .filter_map(|operation| match &operation.action {
                MembershipAction::Revoke { group, .. } => Some(*group),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        self.operations
            .values()
            .filter_map(|operation| match &operation.action {
                MembershipAction::Admit { target, proof } if *target == identity => {
                    Some(proof.claim.create_person.id)
                }
                MembershipAction::AttachPerson { group } if operation.author == identity => {
                    Some(*group)
                }
                _ => None,
            })
            .find(|group| !revoked.contains(group))
    }

    pub(crate) fn principal_anchors(&self) -> Vec<PrincipalAnchor> {
        let mut result = Vec::new();
        for operation in self.operations.values() {
            if matches!(operation.action, MembershipAction::CreatePerson { .. })
                && self.person_group(operation.author) == Some(operation.id)
                && self.contains_member(operation.author)
                && !self.conflicted_keys.contains(&operation.author)
            {
                result.push((operation.id, operation.author, None));
            }
        }
        for operation in self.operations.values() {
            if let MembershipAction::AddAgentKey { group, agent, .. } = operation.action
                && self.contains_member(agent)
                && !self.conflicted_keys.contains(&agent)
                && let Some((_, steward, _)) = result.iter().find(|(id, _, _)| *id == group)
            {
                result.push((operation.id, agent, Some((group, *steward))));
            }
        }
        result
    }

    pub(crate) fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }
    pub(crate) fn genesis_id(&self) -> MembershipOperationId {
        self.genesis_id
    }
    pub(crate) fn has_admission_for_token(
        &self,
        identity: PublicIdentity,
        token: &[u8; 32],
    ) -> bool {
        self.contains_member(identity) && self.operations.values().any(|operation| matches!(&operation.action,
            MembershipAction::Admit { target, proof } if *target == identity && proof.claim.token.id.as_bytes() == token))
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

    pub(crate) fn has_membership_history(&self, identity: PublicIdentity) -> bool {
        self.creator == identity
            || self.operations.values().any(|operation| {
                matches!(
                    &operation.action,
                    MembershipAction::Admit { target, .. } if *target == identity
                ) || matches!(
                    &operation.action,
                    MembershipAction::AddAgentKey { agent, .. } if *agent == identity
                )
            })
    }

    /// Builds a projection straight from envelopes, for the pure replication
    /// tests that exercise export order and disclosure without storage.
    #[cfg(test)]
    pub(crate) fn for_replication_test(
        workspace_id: WorkspaceId,
        creator: PublicIdentity,
        genesis_id: MembershipOperationId,
        operations: BTreeMap<MembershipOperationId, MembershipEnvelope>,
        members: Vec<PublicIdentity>,
    ) -> Self {
        Self {
            workspace_id,
            creator,
            genesis_id,
            operations,
            snapshot: MembershipSnapshot::new(
                workspace_id,
                MembershipEpoch::new(0),
                MembershipVersion::from_bytes([0; 32]),
                members
                    .into_iter()
                    .map(|identity| MemberSnapshot::new(identity, MembershipRole::Member))
                    .collect(),
            ),
            agent_keys: BTreeSet::new(),
            conflicted_keys: BTreeSet::new(),
        }
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
    /// The change the causing operation made, or `None` when that operation is
    /// a workspace `create`, which starts membership rather than changing it.
    pub(crate) change: Option<MembershipChange>,
    pub(crate) author: PublicIdentity,
}

/// Derives the reported change from a membership action. A `create` action
/// starts a workspace and reports no change.
pub(crate) fn effective_change_from_action(
    action: &MembershipAction,
    snapshot: &MembershipSnapshot,
) -> Option<MembershipChange> {
    if matches!(
        action,
        MembershipAction::Admit { .. } | MembershipAction::AddAgentKey { .. }
    ) && let Some(subject) = action.subject()
        && !snapshot
            .members()
            .iter()
            .any(|member| member.identity() == subject)
    {
        return Some(MembershipChange::Revoked { member: subject });
    }
    change_from_action(action)
}

pub(crate) fn change_from_action(action: &MembershipAction) -> Option<MembershipChange> {
    match action {
        MembershipAction::Create
        | MembershipAction::CreatePerson { .. }
        | MembershipAction::AttachPerson { .. } => None,
        MembershipAction::Admit { target, .. } => {
            Some(MembershipChange::Admitted { member: *target })
        }
        MembershipAction::Revoke { target, .. } => {
            Some(MembershipChange::Revoked { member: *target })
        }
        MembershipAction::AddAgentKey { agent, .. } => {
            Some(MembershipChange::Admitted { member: *agent })
        }
        MembershipAction::RemoveAgentKey { agent, .. } => {
            Some(MembershipChange::Revoked { member: *agent })
        }
    }
}

/// Whether a parked operation is a Person group create that waits for the
/// operation attaching it, rather than for its own timestamp.
fn is_waiting_person_create(envelope: &MembershipEnvelope, creator: PublicIdentity) -> bool {
    matches!(envelope.action, MembershipAction::CreatePerson { .. }) && envelope.author != creator
}

pub(crate) fn recover(
    connection: &mut Connection,
    now_unix_ms: i64,
) -> Result<BTreeMap<WorkspaceId, WorkspaceProjection>, StorageError> {
    let mut projections = load_and_replay(connection)?;
    verify_transition_evidence(connection, &projections)?;
    repair_all_derived(connection, &projections)?;
    let _ = maintain_quarantine(connection, &mut projections, now_unix_ms)
        .map_err(membership_to_storage)?;
    Ok(projections)
}

pub(crate) fn maintain_quarantine(
    connection: &mut Connection,
    projections: &mut BTreeMap<WorkspaceId, WorkspaceProjection>,
    now_unix_ms: i64,
) -> Result<Vec<AppliedMembership>, MembershipError> {
    let mut applied = Vec::new();
    loop {
        let rows = load_pending_future(connection)?;
        if rows.is_empty() {
            return Ok(applied);
        }
        let pending_ids = rows
            .iter()
            .filter_map(|(id, _, _, _, _)| operation_id(id).ok())
            .collect::<BTreeSet<_>>();
        // Parked group creates resolve through their dependents, so they do not
        // hold back the operations that depend on them.
        let waiting_creates = rows
            .iter()
            .filter_map(|(id, _, _, _, bytes)| {
                let envelope = authenticate(bytes).ok()?;
                let creator = projections.get(&envelope.workspace_id)?.creator;
                is_waiting_person_create(&envelope, creator).then(|| operation_id(id).ok())?
            })
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
            if is_waiting_person_create(&envelope, projection.creator) {
                continue;
            }
            let unresolved = envelope.dependencies.iter().any(|dependency| {
                !projection.operations.contains_key(dependency)
                    && pending_ids.contains(dependency)
                    && !waiting_creates.contains(dependency)
            });
            if unresolved {
                continue;
            }
            if envelope.authored_at_unix_ms > now_unix_ms.saturating_add(5 * 60 * 1_000) {
                continue;
            }

            match apply_authenticated(connection, projections, envelope, now_unix_ms, Some(id)) {
                Ok(result) => {
                    if result.accepted {
                        applied.push(result);
                    }
                    progressed = true;
                }
                Err(error) if is_terminal(&error) => {
                    reject_future(connection, &stored_id, terminal_reason(&error))?;
                    progressed = true;
                }
                Err(error) => return Err(error),
            }
        }

        if !progressed {
            return Ok(applied);
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

pub(crate) fn apply_canonical_with_limits(
    connection: &mut Connection,
    projections: &mut BTreeMap<WorkspaceId, WorkspaceProjection>,
    canonical_envelope: &[u8],
    now_unix_ms: i64,
    limits: BootstrapLimits,
) -> Result<AppliedMembership, MembershipError> {
    let envelope = authenticate(canonical_envelope)?;
    apply_authenticated_with_limits(connection, projections, envelope, now_unix_ms, None, limits)
}

#[derive(Clone, Copy)]
pub(crate) struct AdmissionContext {
    pub(crate) authenticated_peer: PublicIdentity,
    pub(crate) now_unix_ms: i64,
    pub(crate) limits: BootstrapLimits,
}

#[derive(Clone, Copy)]
pub(crate) struct BootstrapLimits {
    pub(crate) max_body_bytes: usize,
    pub(crate) max_operations: usize,
}

impl BootstrapLimits {
    pub(crate) const CURRENT: Self = Self {
        max_body_bytes: MAX_RESPONSE_BODY_BYTES,
        max_operations: MAX_OPERATIONS,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RedemptionBoundary {
    BeforeCommit,
    AfterCommit,
}

#[derive(Debug)]
pub(crate) struct AdmissionExport {
    pub(crate) applied: AppliedMembership,
    pub(crate) response: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImportBoundary {
    LocalReplay,
    TransitionVerification,
    CreationRequest,
    WorkspaceAnchor,
    Operations,
    Dependencies,
    IdentitySubjects,
    Transitions,
    CurrentState,
    MemberSnapshot,
    BeforeCommit,
    AfterCommit,
}

pub(crate) struct ValidatedBootstrap {
    response: SuccessResponse,
    projection: WorkspaceProjection,
    prefixes: Vec<WorkspaceProjection>,
}

pub(crate) struct ImportedWorkspace {
    pub(crate) summary: crate::WorkspaceSummary,
    pub(crate) projection: WorkspaceProjection,
}

pub(crate) fn validate_bootstrap(
    response: SuccessResponse,
    token: &VerifiedInvitationToken,
    claim: &VerifiedInvitationClaim,
    local_identity: PublicIdentity,
) -> Result<ValidatedBootstrap, crate::OnboardingError> {
    crate::workspace::validate_name(&response.name)
        .map_err(|_| crate::OnboardingError::InvalidBootstrap)?;
    authenticate_canonical_copy(&response.genesis)
        .map_err(|_| crate::OnboardingError::InvalidBootstrap)?;
    for envelope in &response.post_genesis {
        authenticate_canonical_copy(envelope)
            .map_err(|_| crate::OnboardingError::InvalidBootstrap)?;
    }
    if response.workspace_id != token.workspace_id
        || response.creator != token.inviter
        || response.genesis_id != response.genesis.id
        || response.genesis.workspace_id != response.workspace_id
        || response.genesis.author != response.creator
        || response.genesis.authored_at_unix_ms != response.created_at_unix_ms
        || !response.genesis.dependencies.is_empty()
        || !matches!(response.genesis.action, MembershipAction::Create)
        || claim.token.bytes != token.bytes
        || claim.invitee != local_identity
    {
        return Err(crate::OnboardingError::InvalidBootstrap);
    }

    let mut operations = BTreeMap::from([(response.genesis_id, response.genesis.clone())]);
    let mut prefixes = Vec::with_capacity(response.post_genesis.len());
    for envelope in &response.post_genesis {
        if envelope.workspace_id != response.workspace_id
            || matches!(envelope.action, MembershipAction::Create)
            || operations.contains_key(&envelope.id)
            || envelope.dependencies.is_empty()
            || !envelope
                .dependencies
                .iter()
                .all(|dependency| operations.contains_key(dependency))
        {
            return Err(crate::OnboardingError::InvalidBootstrap);
        }
        operations.insert(envelope.id, envelope.clone());
        prefixes.push(
            replay(
                response.workspace_id,
                response.creator,
                response.genesis_id,
                operations.clone(),
            )
            .map_err(|_| crate::OnboardingError::InvalidBootstrap)?,
        );
    }
    let projection = if let Some(last) = prefixes.last() {
        last.clone()
    } else {
        replay(
            response.workspace_id,
            response.creator,
            response.genesis_id,
            operations,
        )
        .map_err(|_| crate::OnboardingError::InvalidBootstrap)?
    };
    let admission = projection
        .operations
        .get(&response.admission_id)
        .ok_or(crate::OnboardingError::InvalidBootstrap)?;
    let MembershipAction::Admit { target, proof } = &admission.action else {
        return Err(crate::OnboardingError::InvalidBootstrap);
    };
    let group_create = projection
        .operations
        .get(&claim.create_person.id)
        .ok_or(crate::OnboardingError::InvalidBootstrap)?;
    if admission.author != token.inviter
        || *target != local_identity
        || proof.claim.bytes != claim.bytes
        || proof.claim.token.bytes != token.bytes
        || group_create.canonical_bytes() != claim.create_person.canonical_bytes()
        || projection.snapshot.version() != response.membership_version
        || !projection.contains_member(local_identity)
    {
        return Err(crate::OnboardingError::InvalidBootstrap);
    }
    Ok(ValidatedBootstrap {
        response,
        projection,
        prefixes,
    })
}

fn ensure_import_deadline(
    deadline: Option<std::time::Instant>,
) -> Result<(), crate::OnboardingError> {
    if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
        Err(crate::OnboardingError::Timeout)
    } else {
        Ok(())
    }
}

fn import_boundary(
    fault: &mut impl FnMut(ImportBoundary) -> Result<(), StorageError>,
    boundary: ImportBoundary,
    deadline: Option<std::time::Instant>,
) -> Result<(), crate::OnboardingError> {
    ensure_import_deadline(deadline)?;
    fault(boundary).map_err(import_fault)?;
    ensure_import_deadline(deadline)
}

pub(crate) fn import_bootstrap_with_fault(
    connection: &mut Connection,
    bootstrap: &ValidatedBootstrap,
    deadline: Option<std::time::Instant>,
    mut fault: impl FnMut(ImportBoundary) -> Result<(), StorageError>,
) -> Result<ImportedWorkspace, crate::OnboardingError> {
    ensure_import_deadline(deadline)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| crate::OnboardingError::LocalConflict)?;
    ensure_import_deadline(deadline)?;
    let local = load_and_replay_until(&transaction, deadline).map_err(import_storage_error)?;
    import_boundary(&mut fault, ImportBoundary::LocalReplay, deadline)?;
    verify_transition_evidence_until(&transaction, &local, deadline)
        .map_err(import_storage_error)?;
    import_boundary(&mut fault, ImportBoundary::TransitionVerification, deadline)?;
    let response = &bootstrap.response;
    let remote_order = std::iter::once(response.genesis_id)
        .chain(response.post_genesis.iter().map(|envelope| envelope.id))
        .collect::<Vec<_>>();

    let mut imported_member_change = false;
    let (imported_projection, writes_derived_state) =
        if let Some(existing) = local.get(&response.workspace_id) {
            let (name, creator, created_at, genesis_id, genesis_bytes): (
                String,
                Vec<u8>,
                i64,
                Vec<u8>,
                Vec<u8>,
            ) = transaction
                .query_row(
                    "SELECT name, creator_public_identity, authored_at_unix_ms,
                        genesis_operation_id, genesis_envelope
                 FROM workspaces WHERE workspace_id = ?1",
                    [response.workspace_id.as_bytes().as_slice()],
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
                .map_err(|_| crate::OnboardingError::LocalConflict)?;
            if name != response.name
                || creator != response.creator.as_bytes()
                || created_at != response.created_at_unix_ms
                || genesis_id != response.genesis_id.as_bytes()
                || genesis_bytes != response.genesis.canonical_bytes()
            {
                return Err(crate::OnboardingError::LocalConflict);
            }
            verify_creation_request(&transaction, response)?;
            let local_order = transition_order(
                &transaction,
                response.workspace_id,
                existing.genesis_id,
                deadline,
            )
            .map_err(import_storage_error)?;
            let shared = local_order.len().min(remote_order.len());
            for index in 0..shared {
                ensure_import_deadline(deadline)?;
                let local_id = local_order[index];
                let remote_id = remote_order[index];
                if local_id != remote_id {
                    return Err(crate::OnboardingError::LocalConflict);
                }
                let local_envelope = existing
                    .operations
                    .get(&local_id)
                    .ok_or(crate::OnboardingError::LocalConflict)?;
                let remote_envelope = bootstrap
                    .projection
                    .operations
                    .get(&remote_id)
                    .ok_or(crate::OnboardingError::InvalidBootstrap)?;
                if local_envelope.canonical_bytes() != remote_envelope.canonical_bytes() {
                    return Err(crate::OnboardingError::LocalConflict);
                }
            }
            if local_order.len() >= remote_order.len() {
                (existing.clone(), false)
            } else {
                let mut candidate = existing.clone();
                for (index, remote_id) in remote_order
                    .iter()
                    .copied()
                    .enumerate()
                    .skip(local_order.len())
                {
                    let envelope = bootstrap
                        .projection
                        .operations
                        .get(&remote_id)
                        .ok_or(crate::OnboardingError::InvalidBootstrap)?;
                    let expected = &bootstrap.prefixes[index - 1];
                    if !candidate
                        .operations
                        .keys()
                        .all(|id| expected.operations.contains_key(id))
                    {
                        return Err(crate::OnboardingError::LocalConflict);
                    }
                    ensure_import_deadline(deadline)?;
                    insert_operation(&transaction, envelope)?;
                    ensure_import_deadline(deadline)?;
                    insert_dependencies(&transaction, envelope, deadline)?;
                    insert_identity_subject(&transaction, envelope)?;
                    ensure_import_deadline(deadline)?;
                    insert_transition(&transaction, expected, envelope.id)?;
                    imported_member_change |= envelope.action.changes_roster();
                    candidate = expected.clone();
                }
                (candidate, true)
            }
        } else {
            if transaction
                .query_row(
                    "SELECT COUNT(*) FROM workspace_creation_requests WHERE workspace_id = ?1",
                    [response.workspace_id.as_bytes().as_slice()],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|_| crate::OnboardingError::LocalConflict)?
                != 0
            {
                return Err(crate::OnboardingError::LocalConflict);
            }
            transaction
                .execute(
                    "INSERT INTO workspace_creation_requests (
                    workspace_id, creator_public_identity, name, authored_at_unix_ms,
                    genesis_operation_id, genesis_envelope, status
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'Complete')",
                    params![
                        response.workspace_id.as_bytes().as_slice(),
                        response.creator.as_bytes().as_slice(),
                        response.name,
                        response.created_at_unix_ms,
                        response.genesis_id.as_bytes().as_slice(),
                        response.genesis.canonical_bytes(),
                    ],
                )
                .map_err(|_| crate::OnboardingError::LocalConflict)?;
            import_boundary(&mut fault, ImportBoundary::CreationRequest, deadline)?;
            transaction
                .execute(
                    "INSERT INTO workspaces (
                    workspace_id, creator_public_identity, name, authored_at_unix_ms,
                    genesis_operation_id, genesis_envelope
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        response.workspace_id.as_bytes().as_slice(),
                        response.creator.as_bytes().as_slice(),
                        response.name,
                        response.created_at_unix_ms,
                        response.genesis_id.as_bytes().as_slice(),
                        response.genesis.canonical_bytes(),
                    ],
                )
                .map_err(|_| crate::OnboardingError::LocalConflict)?;
            import_boundary(&mut fault, ImportBoundary::WorkspaceAnchor, deadline)?;
            for envelope in std::iter::once(&response.genesis).chain(response.post_genesis.iter()) {
                ensure_import_deadline(deadline)?;
                insert_operation(&transaction, envelope)?;
            }
            import_boundary(&mut fault, ImportBoundary::Operations, deadline)?;
            for envelope in &response.post_genesis {
                ensure_import_deadline(deadline)?;
                insert_dependencies(&transaction, envelope, deadline)?;
            }
            import_boundary(&mut fault, ImportBoundary::Dependencies, deadline)?;
            for envelope in std::iter::once(&response.genesis).chain(response.post_genesis.iter()) {
                ensure_import_deadline(deadline)?;
                insert_identity_subject(&transaction, envelope)?;
            }
            import_boundary(&mut fault, ImportBoundary::IdentitySubjects, deadline)?;
            for (envelope, prefix) in response.post_genesis.iter().zip(&bootstrap.prefixes) {
                ensure_import_deadline(deadline)?;
                insert_transition(&transaction, prefix, envelope.id)?;
            }
            import_boundary(&mut fault, ImportBoundary::Transitions, deadline)?;
            (bootstrap.projection.clone(), true)
        };

    if writes_derived_state {
        ensure_import_deadline(deadline)?;
        write_current_state(&transaction, &imported_projection)?;
        import_boundary(&mut fault, ImportBoundary::CurrentState, deadline)?;
        write_member_snapshot(&transaction, &imported_projection, deadline)?;
        import_boundary(&mut fault, ImportBoundary::MemberSnapshot, deadline)?;
        if imported_member_change {
            crate::principals::edit::advance_generation(&transaction, response.workspace_id)
                .map_err(|_| crate::OnboardingError::LocalConflict)?;
        }
    }
    // A new joiner advertises itself to every Member it learned about, so the
    // imported set is due for replication immediately.
    transaction
        .execute(
            "INSERT INTO membership_sync_wakes(workspace_id, wake_generation) VALUES(?1, 1)
             ON CONFLICT(workspace_id) DO UPDATE SET wake_generation = wake_generation + 1",
            [response.workspace_id.as_bytes().as_slice()],
        )
        .map_err(|_| crate::OnboardingError::UncertainStorageOutcome)?;
    import_boundary(&mut fault, ImportBoundary::BeforeCommit, deadline)?;
    // This is the last rollback-safe check. Never turn a completed commit into a timeout.
    ensure_import_deadline(deadline)?;
    transaction
        .commit()
        .map_err(|_| crate::OnboardingError::UncertainStorageOutcome)?;
    let post_commit = fault(ImportBoundary::AfterCommit).map_err(import_fault);
    if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
        return Err(crate::OnboardingError::UncertainStorageOutcome);
    }
    post_commit?;

    Ok(ImportedWorkspace {
        summary: crate::WorkspaceSummary::new(
            response.workspace_id,
            response.name.clone(),
            response.creator,
            response.created_at_unix_ms,
        ),
        projection: imported_projection,
    })
}

fn verify_creation_request(
    connection: &Connection,
    response: &SuccessResponse,
) -> Result<(), crate::OnboardingError> {
    let row: Option<CreationRequestEvidence> = connection
        .query_row(
            "SELECT creator_public_identity, name, authored_at_unix_ms,
                    genesis_operation_id, genesis_envelope, status
             FROM workspace_creation_requests WHERE workspace_id = ?1",
            [response.workspace_id.as_bytes().as_slice()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()
        .map_err(|_| crate::OnboardingError::LocalConflict)?;
    if !matches!(row, Some((creator, name, created_at, genesis_id, genesis, status))
        if creator == response.creator.as_bytes()
            && name == response.name
            && created_at == response.created_at_unix_ms
            && genesis_id == response.genesis_id.as_bytes()
            && genesis == response.genesis.canonical_bytes()
            && status == "Complete")
    {
        return Err(crate::OnboardingError::LocalConflict);
    }
    Ok(())
}

fn transition_order(
    connection: &Connection,
    workspace_id: WorkspaceId,
    genesis_id: MembershipOperationId,
    deadline: Option<std::time::Instant>,
) -> Result<Vec<MembershipOperationId>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT cause_operation_id FROM membership_transitions
             WHERE workspace_id = ?1 ORDER BY epoch",
        )
        .map_err(|_| StorageError::Database)?;
    let rows = statement
        .query_map([workspace_id.as_bytes().as_slice()], |row| {
            row.get::<_, Vec<u8>>(0)
        })
        .map_err(|_| StorageError::Database)?;
    let mut order = vec![genesis_id];
    for row in rows {
        ensure_storage_deadline(deadline)?;
        order.push(
            operation_id(&row.map_err(|_| StorageError::Database)?)
                .map_err(membership_to_storage)?,
        );
    }
    Ok(order)
}

fn insert_operation(
    transaction: &Transaction<'_>,
    envelope: &MembershipEnvelope,
) -> Result<(), crate::OnboardingError> {
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
        .map(|_| ())
        .map_err(|_| crate::OnboardingError::LocalConflict)
}

fn insert_dependencies(
    transaction: &Transaction<'_>,
    envelope: &MembershipEnvelope,
    deadline: Option<std::time::Instant>,
) -> Result<(), crate::OnboardingError> {
    for dependency in &envelope.dependencies {
        ensure_import_deadline(deadline)?;
        transaction
            .execute(
                "INSERT INTO membership_dependencies (operation_id, dependency_operation_id)
                 VALUES (?1, ?2)",
                params![
                    envelope.id.as_bytes().as_slice(),
                    dependency.as_bytes().as_slice()
                ],
            )
            .map_err(|_| crate::OnboardingError::LocalConflict)?;
    }
    Ok(())
}

fn insert_identity_subject(
    transaction: &Transaction<'_>,
    envelope: &MembershipEnvelope,
) -> Result<(), crate::OnboardingError> {
    transaction
        .execute(
            "INSERT INTO identity_subjects (installation_public_identity, human_identity_id)
             VALUES (?1, NULL) ON CONFLICT(installation_public_identity) DO NOTHING",
            [envelope.author.as_bytes().as_slice()],
        )
        .map_err(|_| crate::OnboardingError::LocalConflict)?;
    if let Some(subject) = envelope.action.subject() {
        transaction
            .execute(
                "INSERT INTO identity_subjects (installation_public_identity, human_identity_id)
                 VALUES (?1, NULL) ON CONFLICT(installation_public_identity) DO NOTHING",
                [subject.as_bytes().as_slice()],
            )
            .map_err(|_| crate::OnboardingError::LocalConflict)?;
    }
    Ok(())
}

fn insert_transition(
    transaction: &Transaction<'_>,
    projection: &WorkspaceProjection,
    cause: MembershipOperationId,
) -> Result<(), crate::OnboardingError> {
    let epoch = i64::try_from(projection.snapshot.epoch().value())
        .map_err(|_| crate::OnboardingError::InvalidBootstrap)?;
    transaction
        .execute(
            "INSERT INTO membership_transitions (
                workspace_id, epoch, membership_version, cause_operation_id, snapshot_digest
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                projection.workspace_id.as_bytes().as_slice(),
                epoch,
                projection.snapshot.version().as_bytes().as_slice(),
                cause.as_bytes().as_slice(),
                snapshot_digest(&projection.snapshot).as_bytes().as_slice(),
            ],
        )
        .map(|_| ())
        .map_err(|_| crate::OnboardingError::LocalConflict)
}

fn write_current_state(
    transaction: &Transaction<'_>,
    projection: &WorkspaceProjection,
) -> Result<(), crate::OnboardingError> {
    let epoch = i64::try_from(projection.snapshot.epoch().value())
        .map_err(|_| crate::OnboardingError::InvalidBootstrap)?;
    transaction
        .execute(
            "INSERT INTO workspace_membership_state (workspace_id, epoch, membership_version)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(workspace_id) DO UPDATE SET
                 epoch = excluded.epoch, membership_version = excluded.membership_version",
            params![
                projection.workspace_id.as_bytes().as_slice(),
                epoch,
                projection.snapshot.version().as_bytes().as_slice(),
            ],
        )
        .map(|_| ())
        .map_err(|_| crate::OnboardingError::LocalConflict)
}

fn write_member_snapshot(
    transaction: &Transaction<'_>,
    projection: &WorkspaceProjection,
    deadline: Option<std::time::Instant>,
) -> Result<(), crate::OnboardingError> {
    transaction
        .execute(
            "DELETE FROM membership_snapshot_members WHERE workspace_id = ?1",
            [projection.workspace_id.as_bytes().as_slice()],
        )
        .map_err(|_| crate::OnboardingError::LocalConflict)?;
    for member in projection.snapshot.members() {
        ensure_import_deadline(deadline)?;
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
            .map_err(|_| crate::OnboardingError::LocalConflict)?;
    }
    Ok(())
}

fn import_storage_error(error: StorageError) -> crate::OnboardingError {
    match error {
        StorageError::UncertainOutcome => crate::OnboardingError::UncertainStorageOutcome,
        StorageError::CorruptWorkspace => crate::OnboardingError::LocalConflict,
        _ => crate::OnboardingError::LocalConflict,
    }
}

fn import_fault(error: StorageError) -> crate::OnboardingError {
    if error == StorageError::UncertainOutcome {
        crate::OnboardingError::UncertainStorageOutcome
    } else {
        crate::OnboardingError::LocalConflict
    }
}

pub(crate) fn redeem_invitation(
    connection: &mut Connection,
    projections: &mut BTreeMap<WorkspaceId, WorkspaceProjection>,
    secret_key: &SecretKey,
    claim: VerifiedInvitationClaim,
    authenticated_peer: PublicIdentity,
    now_unix_ms: i64,
) -> Result<AdmissionExport, InvitationError> {
    redeem_invitation_with_fault(
        connection,
        projections,
        secret_key,
        claim,
        authenticated_peer,
        now_unix_ms,
        |_| Ok(()),
    )
}

pub(crate) fn redeem_invitation_with_fault(
    connection: &mut Connection,
    projections: &mut BTreeMap<WorkspaceId, WorkspaceProjection>,
    secret_key: &SecretKey,
    claim: VerifiedInvitationClaim,
    authenticated_peer: PublicIdentity,
    now_unix_ms: i64,
    fault: impl FnMut(RedemptionBoundary) -> Result<(), StorageError>,
) -> Result<AdmissionExport, InvitationError> {
    redeem_invitation_with_fault_and_limits(
        connection,
        projections,
        secret_key,
        claim,
        AdmissionContext {
            authenticated_peer,
            now_unix_ms,
            limits: BootstrapLimits::CURRENT,
        },
        fault,
    )
}

pub(crate) fn redeem_invitation_with_fault_and_limits(
    connection: &mut Connection,
    projections: &mut BTreeMap<WorkspaceId, WorkspaceProjection>,
    secret_key: &SecretKey,
    claim: VerifiedInvitationClaim,
    context: AdmissionContext,
    mut fault: impl FnMut(RedemptionBoundary) -> Result<(), StorageError>,
) -> Result<AdmissionExport, InvitationError> {
    let AdmissionContext {
        authenticated_peer,
        now_unix_ms,
        limits,
    } = context;
    if authenticated_peer != claim.invitee {
        return Err(InvitationError::ClaimTargetMismatch);
    }
    let local_identity = PublicIdentity::from_bytes(*secret_key.public().as_bytes())
        .map_err(|_| InvitationError::InvalidRole)?;
    if claim.token.inviter != local_identity {
        return Err(InvitationError::WrongIssuer);
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| InvitationError::Storage(StorageError::Database))?;
    let mut durable = load_and_replay(&transaction).map_err(InvitationError::Storage)?;
    verify_transition_evidence(&transaction, &durable).map_err(InvitationError::Storage)?;
    let spent: Option<(Vec<u8>, Vec<u8>, Vec<u8>)> = transaction
        .query_row(
            "SELECT workspace_id, invitation_token_id, admission_operation_id
             FROM used_invitation_nonces WHERE nonce = ?1",
            [claim.token.nonce.as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|_| InvitationError::Storage(StorageError::Database))?;
    if let Some((stored_workspace, stored_token_id, stored_admission_id)) = spent {
        let stored_workspace = WorkspaceId::from_bytes(
            fixed(&stored_workspace).map_err(|_| corrupt_invitation_evidence())?,
        );
        let stored_admission_id =
            operation_id(&stored_admission_id).map_err(|_| corrupt_invitation_evidence())?;
        let stored_projection = durable
            .get(&stored_workspace)
            .ok_or_else(corrupt_invitation_evidence)?;
        let admission = stored_projection
            .operations
            .get(&stored_admission_id)
            .ok_or_else(corrupt_invitation_evidence)?;
        let MembershipAction::Admit { target, proof } = &admission.action else {
            return Err(corrupt_invitation_evidence());
        };
        if admission.author != local_identity
            || admission.workspace_id != stored_workspace
            || *target != proof.claim.invitee
            || proof.claim.token.nonce != claim.token.nonce
            || proof.claim.token.id.as_bytes() != stored_token_id.as_slice()
            || proof.claim.token.workspace_id != stored_workspace
        {
            return Err(corrupt_invitation_evidence());
        }
        if stored_workspace != claim.token.workspace_id
            || stored_token_id != claim.token.id.as_bytes()
            || proof.claim.bytes != claim.bytes
            || proof.claim.invitee != authenticated_peer
        {
            return Err(InvitationError::AlreadyUsed);
        }
        if !stored_projection.contains_member(claim.invitee) {
            return Err(InvitationError::Membership(MembershipError::MemberNotFound));
        }
        let response = encode_current_bootstrap_with_limits(
            &transaction,
            stored_projection,
            stored_admission_id,
            limits,
        )
        .map_err(InvitationError::Membership)?;
        let applied = applied_result(stored_projection, admission, false, false);
        transaction
            .commit()
            .map_err(|_| InvitationError::Storage(StorageError::UncertainOutcome))?;
        *projections = durable;
        return Ok(AdmissionExport { applied, response });
    }

    let projection = durable
        .get(&claim.token.workspace_id)
        .ok_or(InvitationError::WorkspaceNotFound)?;
    if claim.token.issued_at_unix_ms > now_unix_ms || claim.token.expires_at_unix_ms < now_unix_ms {
        return Err(InvitationError::Expired);
    }
    let local_is_manager = projection.snapshot.members().iter().any(|member| {
        member.identity() == local_identity && member.role() == MembershipRole::Manager
    });
    if !local_is_manager {
        return Err(InvitationError::NotManager);
    }

    // The invitee's group create and its admission commit together: the
    // group create first, so the admission can depend on it.
    let create_person = claim.create_person.clone();
    let mut candidate_operations = projection.operations.clone();
    let mut stages = Vec::new();
    let with_group = if projection.operations.contains_key(&create_person.id) {
        projection.clone()
    } else {
        let group_context = dependency_context(&transaction, projection, &create_person)
            .map_err(InvitationError::Membership)?;
        validate_context(
            &create_person,
            projection.workspace_id,
            projection.genesis_id,
            &group_context,
            now_unix_ms,
        )
        .map_err(InvitationError::Membership)?;
        candidate_operations.insert(create_person.id, create_person.clone());
        let with_group = replay(
            projection.workspace_id,
            projection.creator,
            projection.genesis_id,
            candidate_operations.clone(),
        )
        .map_err(InvitationError::Membership)?;
        stages.push((create_person.clone(), with_group.clone()));
        with_group
    };
    let envelope = MembershipEnvelope::sign(
        secret_key,
        claim.token.workspace_id,
        now_unix_ms,
        MembershipAction::Admit {
            target: claim.invitee,
            proof: Box::new(AdmissionProof {
                claim: claim.clone(),
            }),
        },
        with_group.heads(),
    )
    .map_err(InvitationError::Membership)?;
    let dependency_context = dependency_context(&transaction, &with_group, &envelope)
        .map_err(InvitationError::Membership)?;
    validate_context(
        &envelope,
        projection.workspace_id,
        projection.genesis_id,
        &dependency_context,
        now_unix_ms,
    )
    .map_err(InvitationError::Membership)?;
    candidate_operations.insert(envelope.id, envelope.clone());
    let candidate = replay(
        projection.workspace_id,
        projection.creator,
        projection.genesis_id,
        candidate_operations,
    )
    .map_err(InvitationError::Membership)?;

    for (staged_envelope, staged) in &stages {
        persist_candidate_rows(&transaction, staged_envelope, staged)
            .map_err(InvitationError::Membership)?;
    }
    persist_candidate_rows(&transaction, &envelope, &candidate)
        .map_err(InvitationError::Membership)?;
    // Encode the exact candidate bootstrap before spending the invitation or committing.
    encode_current_bootstrap_with_limits(&transaction, &candidate, envelope.id, limits)
        .map_err(InvitationError::Membership)?;
    transaction
        .execute(
            "INSERT INTO used_invitation_nonces (
                nonce, workspace_id, invitation_token_id, admission_operation_id
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                claim.token.nonce.as_slice(),
                claim.token.workspace_id.as_bytes().as_slice(),
                claim.token.id.as_bytes().as_slice(),
                envelope.id.as_bytes().as_slice(),
            ],
        )
        .map_err(|error| {
            if matches!(
                error,
                rusqlite::Error::SqliteFailure(ref failure, _)
                    if failure.code == ErrorCode::ConstraintViolation
            ) {
                InvitationError::AlreadyUsed
            } else {
                InvitationError::Storage(StorageError::Database)
            }
        })?;
    fault(RedemptionBoundary::BeforeCommit).map_err(InvitationError::Storage)?;
    transaction
        .commit()
        .map_err(|_| InvitationError::Storage(StorageError::UncertainOutcome))?;
    fault(RedemptionBoundary::AfterCommit).map_err(InvitationError::Storage)?;

    // Reload and encode durable current evidence while the caller still owns the consistency lock.
    let response =
        encode_current_bootstrap_with_limits(connection, &candidate, envelope.id, limits)
            .map_err(InvitationError::Membership)?;
    let result = applied_result(&candidate, &envelope, true, false);
    durable.insert(envelope.workspace_id, candidate);
    *projections = durable;
    Ok(AdmissionExport {
        applied: result,
        response,
    })
}

fn corrupt_invitation_evidence() -> InvitationError {
    InvitationError::Storage(StorageError::CorruptWorkspace)
}

pub(crate) fn encode_current_bootstrap_with_limits(
    connection: &Connection,
    projection: &WorkspaceProjection,
    admission_id: MembershipOperationId,
    limits: BootstrapLimits,
) -> Result<Vec<u8>, MembershipError> {
    verify_transition_evidence(
        connection,
        &BTreeMap::from([(projection.workspace_id, projection.clone())]),
    )
    .map_err(MembershipError::Storage)?;
    let (name, created_at_unix_ms): (String, i64) = connection
        .query_row(
            "SELECT name, authored_at_unix_ms FROM workspaces WHERE workspace_id = ?1",
            [projection.workspace_id.as_bytes().as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    let genesis = projection
        .operations
        .get(&projection.genesis_id)
        .ok_or(MembershipError::Storage(StorageError::CorruptWorkspace))?
        .clone();
    let mut statement = connection
        .prepare(
            "SELECT cause_operation_id FROM membership_transitions
             WHERE workspace_id = ?1 ORDER BY epoch",
        )
        .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    let rows = statement
        .query_map([projection.workspace_id.as_bytes().as_slice()], |row| {
            row.get::<_, Vec<u8>>(0)
        })
        .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    let mut post_genesis = Vec::new();
    for row in rows {
        let bytes = row.map_err(|_| MembershipError::Storage(StorageError::Database))?;
        let id = operation_id(&bytes)?;
        post_genesis.push(
            projection
                .operations
                .get(&id)
                .ok_or(MembershipError::Storage(StorageError::CorruptWorkspace))?
                .clone(),
        );
    }
    let response = SuccessResponse {
        workspace_id: projection.workspace_id,
        creator: projection.creator,
        created_at_unix_ms,
        name,
        genesis_id: projection.genesis_id,
        genesis,
        post_genesis,
        admission_id,
        membership_version: projection.snapshot.version(),
    };
    encode_success_with_limits(&response, limits.max_body_bytes, limits.max_operations).map_err(
        |error| match error {
            CodecError::Limit => MembershipError::DecodeLimit,
            _ => MembershipError::Storage(StorageError::CorruptWorkspace),
        },
    )
}

fn apply_authenticated(
    connection: &mut Connection,
    projections: &mut BTreeMap<WorkspaceId, WorkspaceProjection>,
    envelope: MembershipEnvelope,
    now_unix_ms: i64,
    future_id: Option<MembershipOperationId>,
) -> Result<AppliedMembership, MembershipError> {
    apply_authenticated_with_limits(
        connection,
        projections,
        envelope,
        now_unix_ms,
        future_id,
        BootstrapLimits::CURRENT,
    )
}

fn apply_authenticated_with_limits(
    connection: &mut Connection,
    projections: &mut BTreeMap<WorkspaceId, WorkspaceProjection>,
    envelope: MembershipEnvelope,
    now_unix_ms: i64,
    future_id: Option<MembershipOperationId>,
    limits: BootstrapLimits,
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
        return Ok(applied_result(projection, &envelope, false, false));
    }

    let context = dependency_context(connection, projection, &envelope)?;
    match validate_context(
        &envelope,
        projection.workspace_id,
        projection.genesis_id,
        &context,
        now_unix_ms,
    ) {
        Err(MembershipError::FutureTimestamp) if future_id.is_none() => {
            store_future(connection, &envelope)?;
            return Ok(applied_result(projection, &envelope, false, true));
        }
        Err(error) => return Err(error),
        Ok(()) => {}
    }
    // A Person group create by anyone but the creator grants nothing on its
    // own and enters the accepted set only with the operation that attaches
    // it. Alone it is parked, like an operation whose time has not come.
    if is_waiting_person_create(&envelope, projection.creator) && future_id.is_none() {
        store_future(connection, &envelope)?;
        return Ok(applied_result(projection, &envelope, false, true));
    }

    // Parked group creates this operation depends on come along with it.
    let parked = parked_person_creates(connection, projection, &envelope)?;
    let mut candidate_operations = projection.operations.clone();
    let mut stages = Vec::with_capacity(parked.len());
    for parked_envelope in &parked {
        let parked_context = dependency_context(connection, projection, parked_envelope)?;
        validate_context(
            parked_envelope,
            projection.workspace_id,
            projection.genesis_id,
            &parked_context,
            now_unix_ms,
        )?;
        candidate_operations.insert(parked_envelope.id, parked_envelope.clone());
        let staged = replay(
            projection.workspace_id,
            projection.creator,
            projection.genesis_id,
            candidate_operations.clone(),
        )?;
        stages.push((parked_envelope.clone(), staged));
    }
    candidate_operations.insert(envelope.id, envelope.clone());
    let candidate = replay(
        projection.workspace_id,
        projection.creator,
        projection.genesis_id,
        candidate_operations,
    )?;

    let mut released = parked.iter().map(|parked| parked.id).collect::<Vec<_>>();
    released.extend(future_id);
    persist_candidate(
        connection, &stages, &envelope, &candidate, &released, limits,
    )?;
    let result = applied_result(&candidate, &envelope, true, false);
    projections.insert(envelope.workspace_id, candidate);
    Ok(result)
}

/// The parked Person group creates among `envelope`'s dependencies, so an
/// attachment that arrives after the group create it depends on can take the
/// create into the accepted set with it. Any other missing dependency is
/// reported by replay as usual.
fn parked_person_creates(
    connection: &Connection,
    projection: &WorkspaceProjection,
    envelope: &MembershipEnvelope,
) -> Result<Vec<MembershipEnvelope>, MembershipError> {
    let mut parked = Vec::new();
    for id in &envelope.dependencies {
        if projection.operations.contains_key(id) {
            continue;
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
        let Some(bytes) = future else {
            continue;
        };
        let dependency = authenticate(&bytes)?;
        if dependency.id == *id && is_waiting_person_create(&dependency, projection.creator) {
            parked.push(dependency);
        }
    }
    parked.sort_by_key(|dependency| dependency.id);
    Ok(parked)
}

fn applied_result(
    projection: &WorkspaceProjection,
    envelope: &MembershipEnvelope,
    accepted: bool,
    quarantined: bool,
) -> AppliedMembership {
    AppliedMembership {
        operation_id: envelope.id,
        snapshot: projection.snapshot(),
        snapshot_digest: snapshot_digest(&projection.snapshot),
        accepted,
        quarantined,
        change: effective_change_from_action(&envelope.action, &projection.snapshot),
        author: envelope.author,
    }
}

fn load_and_replay(
    connection: &Connection,
) -> Result<BTreeMap<WorkspaceId, WorkspaceProjection>, StorageError> {
    load_and_replay_until(connection, None)
}

fn ensure_storage_deadline(deadline: Option<std::time::Instant>) -> Result<(), StorageError> {
    if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
        Err(StorageError::Database)
    } else {
        Ok(())
    }
}

fn load_and_replay_until(
    connection: &Connection,
    deadline: Option<std::time::Instant>,
) -> Result<BTreeMap<WorkspaceId, WorkspaceProjection>, StorageError> {
    ensure_storage_deadline(deadline)?;
    let anchors = load_anchors(connection, deadline)?;
    let mut projections = BTreeMap::new();
    for (workspace_id, creator, genesis_id, genesis_bytes) in anchors {
        ensure_storage_deadline(deadline)?;
        let operations = load_operations(connection, workspace_id, deadline)?;
        let genesis = operations
            .get(&genesis_id)
            .ok_or(StorageError::CorruptWorkspace)?;
        if genesis.canonical_bytes() != genesis_bytes {
            return Err(StorageError::CorruptWorkspace);
        }
        let projection = replay_until(workspace_id, creator, genesis_id, operations, deadline)
            .map_err(|error| match error {
                MembershipError::Storage(storage) => storage,
                _ => StorageError::CorruptWorkspace,
            })?;
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

fn load_anchors(
    connection: &Connection,
    deadline: Option<std::time::Instant>,
) -> Result<Vec<WorkspaceAnchor>, StorageError> {
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
        ensure_storage_deadline(deadline)?;
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
    deadline: Option<std::time::Instant>,
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
        ensure_storage_deadline(deadline)?;
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
        let stored_dependencies = load_dependencies(connection, envelope.id, deadline)?;
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
    deadline: Option<std::time::Instant>,
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
    let mut dependencies = Vec::new();
    for row in rows {
        ensure_storage_deadline(deadline)?;
        let bytes = row.map_err(|_| StorageError::Database)?;
        dependencies.push(MembershipOperationId::from_hash(Hash::from_bytes(fixed(
            &bytes,
        )?)));
    }
    Ok(dependencies)
}

pub(crate) fn replay(
    workspace_id: WorkspaceId,
    creator: PublicIdentity,
    genesis_id: MembershipOperationId,
    operations: BTreeMap<MembershipOperationId, MembershipEnvelope>,
) -> Result<WorkspaceProjection, MembershipError> {
    replay_until(workspace_id, creator, genesis_id, operations, None)
}

fn ensure_membership_deadline(deadline: Option<std::time::Instant>) -> Result<(), MembershipError> {
    ensure_storage_deadline(deadline).map_err(MembershipError::Storage)
}

fn replay_until(
    workspace_id: WorkspaceId,
    creator: PublicIdentity,
    genesis_id: MembershipOperationId,
    operations: BTreeMap<MembershipOperationId, MembershipEnvelope>,
    deadline: Option<std::time::Instant>,
) -> Result<WorkspaceProjection, MembershipError> {
    ensure_membership_deadline(deadline)?;
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
        ensure_membership_deadline(deadline)?;
        let mut ready = Vec::new();
        for operation in operations.values() {
            ensure_membership_deadline(deadline)?;
            if !processed.contains(&operation.id)
                && operation
                    .dependencies
                    .iter()
                    .all(|dependency| processed.contains(dependency))
            {
                ready.push(operation.id);
            }
        }
        if ready.is_empty() {
            return Err(MembershipError::MissingDependency);
        }

        for id in ready {
            ensure_membership_deadline(deadline)?;
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
            let ancestry = Ledger::ancestry_of(operation, &operations, || {
                ensure_membership_deadline(deadline)
            })?;
            authority::authorize(operation, creator, &ancestry)?;
            state = MembershipCrdt::process(state, &OperationAdapter(operation.clone()))
                .map_err(|_| MembershipError::AuthorizationRejected)?;
            processed.insert(id);
        }
    }

    ensure_membership_deadline(deadline)?;
    // Count independently accepted admission anchors, even when one branch
    // has since been removed. A revocation cannot resurrect the other claim.
    let mut admitted = BTreeSet::new();
    let mut conflicted_keys = BTreeSet::new();
    for operation in operations.values() {
        let key = match &operation.action {
            MembershipAction::Admit { target, .. } => Some(*target),
            MembershipAction::AttachPerson { .. } => Some(operation.author),
            MembershipAction::AddAgentKey { agent, .. } => Some(*agent),
            _ => None,
        };
        if let Some(key) = key
            && key != creator
            && !admitted.insert(key)
        {
            conflicted_keys.insert(key);
        }
    }
    let conflicted_stewards = operations
        .values()
        .filter_map(|operation| match &operation.action {
            MembershipAction::Admit { target, proof } if conflicted_keys.contains(target) => {
                Some(proof.claim.create_person.id)
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let ineffective_agents = operations
        .values()
        .filter_map(|operation| match &operation.action {
            MembershipAction::AddAgentKey { group, agent, .. }
                if conflicted_stewards.contains(group) =>
            {
                Some(*agent)
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    // The snapshot flattens every nested Person group: each effective key is
    // a Member, and only the creator holds manage access.
    let mut members = Vec::new();
    for (id, access) in state.members(AuthId::workspace(workspace_id)) {
        ensure_membership_deadline(deadline)?;
        let AuthId::Installation(bytes) = id else {
            return Err(MembershipError::InvalidRole);
        };
        let identity =
            PublicIdentity::from_bytes(bytes).map_err(|_| MembershipError::InvalidRole)?;
        if conflicted_keys.contains(&identity) || ineffective_agents.contains(&identity) {
            continue;
        }
        let role = if access.is_manage() {
            MembershipRole::Manager
        } else if access.is_write() {
            MembershipRole::Member
        } else {
            return Err(MembershipError::InvalidRole);
        };
        members.push(MemberSnapshot::new(identity, role));
    }
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
    let agent_keys = authority::agent_keys(&operations);
    Ok(WorkspaceProjection {
        workspace_id,
        creator,
        genesis_id,
        operations,
        snapshot,
        agent_keys,
        conflicted_keys,
    })
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
    stages: &[(MembershipEnvelope, WorkspaceProjection)],
    envelope: &MembershipEnvelope,
    candidate: &WorkspaceProjection,
    released_future_ids: &[MembershipOperationId],
    limits: BootstrapLimits,
) -> Result<(), MembershipError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    for (staged_envelope, staged) in stages {
        persist_candidate_rows(&transaction, staged_envelope, staged)?;
    }
    persist_candidate_rows(&transaction, envelope, candidate)?;
    // Every accepted mutation must leave a complete onboarding bootstrap encodable.
    encode_current_bootstrap_with_limits(&transaction, candidate, candidate.genesis_id, limits)?;
    for id in released_future_ids {
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

pub(crate) fn persist_candidate_rows(
    transaction: &Transaction<'_>,
    envelope: &MembershipEnvelope,
    candidate: &WorkspaceProjection,
) -> Result<(), MembershipError> {
    // Both direct membership acceptance and invitation redemption use this
    // transaction. Never commit a roster change without its name invalidation.
    crate::principals::refresh(transaction, candidate).map_err(MembershipError::Storage)?;
    if envelope.action.changes_roster() {
        crate::principals::edit::advance_generation(transaction, envelope.workspace_id).map_err(
            |error| match error {
                crate::principals::edit::EditError::Storage(storage) => {
                    MembershipError::Storage(storage)
                }
                _ => MembershipError::Storage(StorageError::CorruptWorkspace),
            },
        )?;
    }
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
    for subject in std::iter::once(envelope.author).chain(envelope.action.subject()) {
        transaction
            .execute(
                "INSERT INTO identity_subjects (installation_public_identity, human_identity_id)
                 VALUES (?1, NULL) ON CONFLICT(installation_public_identity) DO NOTHING",
                [subject.as_bytes().as_slice()],
            )
            .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    }
    // Every accepted membership operation bumps the Workspace's replication
    // wake in the same transaction, so propagation cannot be lost on a crash.
    transaction
        .execute(
            "INSERT INTO membership_sync_wakes(workspace_id, wake_generation) VALUES(?1, 1)
             ON CONFLICT(workspace_id) DO UPDATE SET wake_generation = wake_generation + 1",
            [envelope.workspace_id.as_bytes().as_slice()],
        )
        .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    write_derived(transaction, candidate)?;
    let epoch = i64::try_from(candidate.snapshot.epoch().value())
        .map_err(|_| MembershipError::Storage(StorageError::CorruptWorkspace))?;
    let digest = snapshot_digest(&candidate.snapshot);
    transaction
        .execute(
            "INSERT INTO membership_transitions (
                workspace_id, epoch, membership_version, cause_operation_id, snapshot_digest
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                candidate.workspace_id.as_bytes().as_slice(),
                epoch,
                candidate.snapshot.version().as_bytes().as_slice(),
                envelope.id.as_bytes().as_slice(),
                digest.as_bytes().as_slice(),
            ],
        )
        .map_err(|_| MembershipError::Storage(StorageError::Database))?;
    Ok(())
}

fn verify_transition_evidence(
    connection: &Connection,
    projections: &BTreeMap<WorkspaceId, WorkspaceProjection>,
) -> Result<(), StorageError> {
    verify_transition_evidence_until(connection, projections, None)
}

fn verify_transition_evidence_until(
    connection: &Connection,
    projections: &BTreeMap<WorkspaceId, WorkspaceProjection>,
    deadline: Option<std::time::Instant>,
) -> Result<(), StorageError> {
    for projection in projections.values() {
        ensure_storage_deadline(deadline)?;
        let mut statement = connection
            .prepare(
                "SELECT epoch, membership_version, cause_operation_id, snapshot_digest
                 FROM membership_transitions WHERE workspace_id = ?1 ORDER BY epoch",
            )
            .map_err(|_| StorageError::Database)?;
        let rows = statement
            .query_map([projection.workspace_id.as_bytes().as_slice()], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            })
            .map_err(|_| StorageError::Database)?;
        let mut events = Vec::new();
        for row in rows {
            ensure_storage_deadline(deadline)?;
            events.push(row.map_err(|_| StorageError::Database)?);
        }

        let genesis = projection
            .operations
            .get(&projection.genesis_id)
            .ok_or(StorageError::CorruptWorkspace)?
            .clone();
        let mut prefix = BTreeMap::from([(projection.genesis_id, genesis)]);
        for (index, (stored_epoch, stored_version, stored_cause, stored_digest)) in
            events.into_iter().enumerate()
        {
            ensure_storage_deadline(deadline)?;
            let expected_epoch =
                i64::try_from(index + 1).map_err(|_| StorageError::CorruptWorkspace)?;
            if stored_epoch != expected_epoch {
                return Err(StorageError::CorruptWorkspace);
            }
            let cause = operation_id(&stored_cause).map_err(membership_to_storage)?;
            let operation = projection
                .operations
                .get(&cause)
                .ok_or(StorageError::CorruptWorkspace)?;
            if cause == projection.genesis_id
                || prefix.contains_key(&cause)
                || !operation
                    .dependencies
                    .iter()
                    .all(|dependency| prefix.contains_key(dependency))
            {
                return Err(StorageError::CorruptWorkspace);
            }
            prefix.insert(cause, operation.clone());
            let at_event = replay_until(
                projection.workspace_id,
                projection.creator,
                projection.genesis_id,
                prefix.clone(),
                deadline,
            )
            .map_err(|error| match error {
                MembershipError::Storage(storage) => storage,
                _ => StorageError::CorruptWorkspace,
            })?;
            if at_event.snapshot.epoch().value()
                != u64::try_from(expected_epoch).map_err(|_| StorageError::CorruptWorkspace)?
                || at_event.snapshot.version()
                    != MembershipVersion::from_bytes(fixed(&stored_version)?)
                || snapshot_digest(&at_event.snapshot)
                    != MembershipSnapshotDigest::from_bytes(fixed(&stored_digest)?)
            {
                return Err(StorageError::CorruptWorkspace);
            }
        }
        if prefix.len() != projection.operations.len() {
            return Err(StorageError::CorruptWorkspace);
        }
    }
    Ok(())
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
        crate::principals::refresh(&transaction, projection)?;
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

pub(crate) fn membership_version(
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

/// The stored action discriminator. Roster-changing actions are 1, 2, 5 and 6;
/// the subscription loader keys on that set.
pub(crate) fn action_tag(action: &MembershipAction) -> i64 {
    match action {
        MembershipAction::Create => 0,
        MembershipAction::Admit { .. } => 1,
        MembershipAction::Revoke { .. } => 2,
        MembershipAction::CreatePerson { .. } => 3,
        MembershipAction::AttachPerson { .. } => 4,
        MembershipAction::AddAgentKey { .. } => 5,
        MembershipAction::RemoveAgentKey { .. } => 6,
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
        MembershipError::UnknownPersonGroup => "unknown person group",
        MembershipError::PersonNotAdmitted => "person group not admitted",
        MembershipError::DuplicateMemberKey => "duplicate member key",
        MembershipError::AgentKeyCannotAuthor => "agent key cannot author",
        _ => "invalid membership operation",
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::invitation::{sign_claim, sign_token};

    fn count_redemption_rows(connection: &Connection) -> (i64, i64, i64) {
        connection
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM used_invitation_nonces),
                    (SELECT COUNT(*) FROM membership_operations WHERE action = 1),
                    (SELECT epoch FROM workspace_membership_state LIMIT 1)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap()
    }

    #[test]
    fn a_non_manager_inviter_cannot_redeem_or_consume_a_nonce() {
        let directory = std::env::temp_dir().join(format!(
            "teamarness-invitation-non-manager-redeem-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        let stored = crate::storage::open(&directory, 0).unwrap();
        let mut connection = stored.connection;
        let manager = stored.secret_key;
        let workspace =
            crate::workspace::create(&mut connection, &manager, "Manager check", 1_000).unwrap();
        let mut projections = recover(&mut connection, 1_100).unwrap();
        let member = SecretKey::from_bytes(&[61; 32]);
        let manager_token = sign_token(&manager, workspace.id(), 1_000, 2_000, [1; 32]).unwrap();
        let genesis = projections[&workspace.id()].genesis_id;
        let manager_claim = sign_claim(&member, &manager_token, genesis).unwrap();
        redeem_invitation(
            &mut connection,
            &mut projections,
            &manager,
            manager_claim.clone(),
            manager_claim.invitee,
            1_100,
        )
        .unwrap();
        let before = count_redemption_rows(&connection);

        let member_token = sign_token(&member, workspace.id(), 1_101, 2_000, [2; 32]).unwrap();
        let target = SecretKey::from_bytes(&[62; 32]);
        let member_claim = sign_claim(&target, &member_token, genesis).unwrap();
        assert_eq!(
            redeem_invitation(
                &mut connection,
                &mut projections,
                &member,
                member_claim.clone(),
                member_claim.invitee,
                1_102,
            )
            .unwrap_err(),
            InvitationError::NotManager
        );
        assert_eq!(count_redemption_rows(&connection), before);

        drop(connection);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invitation_commit_faults_consume_nothing_or_recover_one_stable_admission() {
        for (index, boundary) in [
            RedemptionBoundary::BeforeCommit,
            RedemptionBoundary::AfterCommit,
        ]
        .into_iter()
        .enumerate()
        {
            let directory = std::env::temp_dir().join(format!(
                "teamarness-invitation-fault-{}-{index}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&directory);
            let stored = crate::storage::open(&directory, 0).unwrap();
            let mut connection = stored.connection;
            let manager = stored.secret_key;
            let workspace =
                crate::workspace::create(&mut connection, &manager, "Invitation fault", 1_000)
                    .unwrap();
            let mut projections = recover(&mut connection, 1_100).unwrap();
            let token = sign_token(
                &manager,
                workspace.id(),
                1_000,
                2_000,
                [u8::try_from(index + 1).unwrap(); 32],
            )
            .unwrap();
            let invitee = SecretKey::from_bytes(&[u8::try_from(index + 41).unwrap(); 32]);
            let genesis = projections[&workspace.id()].genesis_id;
            let claim = sign_claim(&invitee, &token, genesis).unwrap();

            let result = redeem_invitation_with_fault(
                &mut connection,
                &mut projections,
                &manager,
                claim.clone(),
                claim.invitee,
                1_100,
                |current| {
                    if current == boundary {
                        Err(if current == RedemptionBoundary::AfterCommit {
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
            // Genesis, the creator's group create and its attachment: epoch 2.
            assert_eq!(
                projections[&workspace.id()].snapshot.epoch(),
                MembershipEpoch::new(2)
            );

            if boundary == RedemptionBoundary::BeforeCommit {
                assert_eq!(count_redemption_rows(&connection), (0, 0, 2));
                let accepted = redeem_invitation(
                    &mut connection,
                    &mut projections,
                    &manager,
                    claim.clone(),
                    claim.invitee,
                    1_100,
                )
                .unwrap();
                assert!(accepted.applied.accepted);
                // The invitee's group create and its admission: two more epochs.
                assert_eq!(count_redemption_rows(&connection), (1, 1, 4));
            } else {
                assert_eq!(
                    result.unwrap_err(),
                    InvitationError::Storage(StorageError::UncertainOutcome)
                );
                assert_eq!(count_redemption_rows(&connection), (1, 1, 4));
                let committed_id: Vec<u8> = connection
                    .query_row(
                        "SELECT admission_operation_id FROM used_invitation_nonces",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                drop(connection);

                let reopened = crate::storage::open(&directory, 0).unwrap();
                connection = reopened.connection;
                projections = recover(&mut connection, 1_100).unwrap();
                assert_eq!(
                    projections[&workspace.id()].snapshot.epoch(),
                    MembershipEpoch::new(4)
                );
                assert_eq!(
                    redeem_invitation(
                        &mut connection,
                        &mut projections,
                        &manager,
                        claim.clone(),
                        claim.invitee,
                        1_100,
                    )
                    .unwrap()
                    .applied
                    .operation_id,
                    MembershipOperationId::from_hash(Hash::from_bytes(
                        fixed(&committed_id).unwrap()
                    ))
                );
                let recovered_id: Vec<u8> = connection
                    .query_row(
                        "SELECT admission_operation_id FROM used_invitation_nonces",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(recovered_id, committed_id);
                assert_eq!(count_redemption_rows(&connection), (1, 1, 4));
            }

            drop(connection);
            fs::remove_dir_all(directory).unwrap();
        }
    }
}
