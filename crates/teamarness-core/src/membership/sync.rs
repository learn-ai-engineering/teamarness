use std::collections::BTreeSet;

use rusqlite::Connection;

use crate::error::MembershipError;
use crate::{PublicIdentity, WorkspaceId};

use super::envelope::MembershipEnvelope;
use super::projection::{AppliedMembership, WorkspaceProjection, apply_canonical};

pub(crate) const MAX_PROOF_OPERATIONS: usize = 1_024;
pub(crate) const MAX_PROOF_BYTES: usize = 8 * 1024 * 1024;

/// Authenticated canonical membership evidence exchanged before package data.
/// The proof is ordered from the workspace genesis toward its current heads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MembershipProof {
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) operations: Vec<Vec<u8>>,
}

impl MembershipProof {
    pub(crate) fn validate(&self) -> Result<(), MembershipError> {
        if self.operations.is_empty() || self.operations.len() > MAX_PROOF_OPERATIONS {
            return Err(MembershipError::DecodeLimit);
        }
        let bytes = self
            .operations
            .iter()
            .try_fold(0usize, |total, operation| {
                total
                    .checked_add(operation.len())
                    .ok_or(MembershipError::DecodeLimit)
            })?;
        if bytes > MAX_PROOF_BYTES {
            return Err(MembershipError::DecodeLimit);
        }
        Ok(())
    }
}

pub(crate) fn export(projection: &WorkspaceProjection) -> Result<MembershipProof, MembershipError> {
    let mut remaining = projection.operations.clone();
    let mut emitted = BTreeSet::new();
    let mut operations = Vec::with_capacity(remaining.len());
    while !remaining.is_empty() {
        let mut ready = remaining
            .iter()
            .filter(|(_, operation)| {
                operation
                    .dependencies
                    .iter()
                    .all(|dependency| emitted.contains(dependency))
            })
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        ready.sort_unstable();
        if ready.is_empty() {
            return Err(MembershipError::MissingDependency);
        }
        for id in ready {
            let operation = remaining
                .remove(&id)
                .ok_or(MembershipError::MissingDependency)?;
            operations.push(operation.canonical_bytes().to_vec());
            emitted.insert(id);
        }
    }
    let proof = MembershipProof {
        workspace_id: projection.workspace_id,
        operations,
    };
    proof.validate()?;
    Ok(proof)
}

pub(crate) fn apply(
    connection: &mut Connection,
    projections: &mut std::collections::BTreeMap<WorkspaceId, WorkspaceProjection>,
    proof: &MembershipProof,
    now_unix_ms: i64,
) -> Result<Vec<AppliedMembership>, MembershipError> {
    proof.validate()?;
    let mut applied = Vec::new();
    for operation in &proof.operations {
        let envelope = MembershipEnvelope::decode(operation)?;
        if envelope.workspace_id != proof.workspace_id {
            return Err(MembershipError::WorkspaceMismatch);
        }
        let result = apply_canonical(connection, projections, operation, now_unix_ms)?;
        if result.accepted {
            applied.push(result);
        }
    }
    Ok(applied)
}

pub(crate) fn admits_peer(
    proof: &MembershipProof,
    peer: PublicIdentity,
) -> Result<bool, MembershipError> {
    proof.validate()?;
    let mut admitted = false;
    for operation in &proof.operations {
        let envelope = MembershipEnvelope::decode(operation)?;
        if let super::MembershipAction::Admit { target, .. } = &envelope.action
            && *target == peer
        {
            admitted = true;
        }
    }
    Ok(admitted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proof_rejects_unbounded_operation_batches() {
        let proof = MembershipProof {
            workspace_id: WorkspaceId::from_bytes([1; 16]),
            operations: vec![vec![0; MAX_PROOF_BYTES + 1]],
        };
        assert_eq!(proof.validate(), Err(MembershipError::DecodeLimit));
    }
}
