use crate::error::MembershipError;
use crate::{MembershipOperationId, WorkspaceId};

use super::{MembershipAction, MembershipEnvelope};

/// Performs only version-1 structural and cryptographic checks. Workspace
/// anchoring, dependency lookup, timestamps, authorization, and CRDT processing
/// intentionally belong to later phases.
pub(crate) fn authenticate(bytes: &[u8]) -> Result<MembershipEnvelope, MembershipError> {
    MembershipEnvelope::decode(bytes)
}

/// Re-authenticates an already decoded wire envelope and rejects any mismatch
/// between its parsed fields and canonical signed bytes.
pub(crate) fn authenticate_canonical_copy(
    envelope: &MembershipEnvelope,
) -> Result<(), MembershipError> {
    let authenticated = authenticate(envelope.canonical_bytes())?;
    if authenticated.id != envelope.id
        || authenticated.workspace_id != envelope.workspace_id
        || authenticated.author != envelope.author
        || authenticated.authored_at_unix_ms != envelope.authored_at_unix_ms
        || authenticated.dependencies != envelope.dependencies
        || authenticated.signature.to_bytes() != envelope.signature.to_bytes()
        || !authenticated.action.same_as(&envelope.action)
    {
        return Err(MembershipError::ContentHashMismatch);
    }
    Ok(())
}

/// Validates an authenticated envelope against one immutable workspace anchor
/// and its available causal dependency records.
pub(crate) fn validate_context(
    envelope: &MembershipEnvelope,
    workspace_id: WorkspaceId,
    genesis_id: MembershipOperationId,
    dependencies: &[(MembershipOperationId, WorkspaceId, i64, bool)],
    now_unix_ms: i64,
) -> Result<(), MembershipError> {
    if envelope.workspace_id != workspace_id {
        return Err(MembershipError::WorkspaceMismatch);
    }
    if matches!(envelope.action, MembershipAction::Create) {
        return Err(MembershipError::ConflictingGenesis);
    }
    if envelope.dependencies.is_empty() {
        return Err(MembershipError::NonDescendantDependency);
    }
    for dependency_id in &envelope.dependencies {
        let Some((_, dependency_workspace, timestamp, descends_from_genesis)) = dependencies
            .iter()
            .find(|(id, _, _, _)| id == dependency_id)
        else {
            return Err(MembershipError::MissingDependency);
        };
        if *dependency_workspace != workspace_id {
            return Err(MembershipError::CrossWorkspaceDependency);
        }
        if !*descends_from_genesis && *dependency_id != genesis_id {
            return Err(MembershipError::NonDescendantDependency);
        }
        if envelope.authored_at_unix_ms < *timestamp {
            return Err(MembershipError::NonMonotonicTimestamp);
        }
    }
    if envelope.authored_at_unix_ms > now_unix_ms.saturating_add(5 * 60 * 1_000) {
        return Err(MembershipError::FutureTimestamp);
    }
    Ok(())
}
