use std::collections::BTreeMap;

use crate::error::MembershipError;
use crate::{PublicIdentity, WorkspaceId};

use super::projection::{WorkspaceProjection, replay};

/// Resolves authenticated author-declared history. This result is informational
/// only: late operations can revise it, so it must never authorize a write.
pub(crate) fn was_member(
    projection: &WorkspaceProjection,
    workspace_id: WorkspaceId,
    identity: PublicIdentity,
    at_unix_ms: i64,
) -> Result<bool, MembershipError> {
    if projection.workspace_id != workspace_id {
        return Err(MembershipError::WorkspaceMismatch);
    }
    let operations = projection
        .operations
        .iter()
        .filter(|(_, operation)| operation.authored_at_unix_ms <= at_unix_ms)
        .map(|(id, operation)| (*id, operation.clone()))
        .collect::<BTreeMap<_, _>>();
    if !operations.contains_key(&projection.genesis_id) {
        return Ok(false);
    }

    let historical = replay(
        projection.workspace_id,
        projection.creator,
        projection.genesis_id,
        operations,
    )?;
    Ok(historical.contains_member(identity))
}
