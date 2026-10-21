//! Causal export, want computation, and the disclosure policy.
//!
//! Everything here is pure: it reads a [`WorkspaceProjection`] and produces
//! canonical bytes. Nothing in this module names Iroh, a session, or storage,
//! so the replication protocol and its tests can reason about the policy on its
//! own.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::MembershipError;
use crate::{MembershipOperationId, PublicIdentity};

use super::MembershipAction;
use super::projection::WorkspaceProjection;

/// One session carries at most a whole accepted operation set, which every
/// accepted mutation already preflights against the same bound.
pub(crate) const MAX_REPLICATION_OPERATIONS: usize = 1_024;
pub(crate) const MAX_REPLICATION_BYTES: usize = 8 * 1024 * 1024;

/// What one Installation is willing to send the peer that asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Disclosure {
    /// A current Member receives every accepted operation it lacks.
    CurrentMember(Vec<Vec<u8>>),
    /// A former Member receives only the causal closure of the revocations
    /// that target it, so it can learn and replay its own revocation.
    FormerMember(Vec<Vec<u8>>),
    /// A peer with no membership history receives nothing at all.
    None,
}

impl Disclosure {
    pub(crate) fn operations(&self) -> &[Vec<u8>] {
        match self {
            Self::CurrentMember(operations) | Self::FormerMember(operations) => operations,
            Self::None => &[],
        }
    }
}

/// Every accepted operation identifier, in canonical sorted order.
pub(crate) fn operation_ids(projection: &WorkspaceProjection) -> Vec<MembershipOperationId> {
    projection.operations.keys().copied().collect()
}

/// The identifiers the remote advertised that are missing locally.
pub(crate) fn want(
    local: &BTreeSet<MembershipOperationId>,
    remote: &[MembershipOperationId],
) -> Vec<MembershipOperationId> {
    remote
        .iter()
        .filter(|id| !local.contains(*id))
        .copied()
        .collect()
}

/// Emits `subset` in an order where every dependency precedes its dependents.
///
/// The order comes from a Kahn pass over the whole accepted set, filtered to
/// `subset`. The receiver already holds every operation outside `subset`, so
/// each dependency is either already held or earlier in the stream.
pub(crate) fn export_causal(
    projection: &WorkspaceProjection,
    subset: &BTreeSet<MembershipOperationId>,
) -> Result<Vec<Vec<u8>>, MembershipError> {
    export_causal_with_limits(
        projection,
        subset,
        MAX_REPLICATION_OPERATIONS,
        MAX_REPLICATION_BYTES,
    )
}

fn export_causal_with_limits(
    projection: &WorkspaceProjection,
    subset: &BTreeSet<MembershipOperationId>,
    max_operations: usize,
    max_bytes: usize,
) -> Result<Vec<Vec<u8>>, MembershipError> {
    if subset.len() > max_operations {
        return Err(MembershipError::DecodeLimit);
    }
    let mut remaining: BTreeMap<_, _> = projection.operations.clone();
    let mut emitted = BTreeSet::new();
    let mut operations = Vec::new();
    let mut bytes = 0_usize;
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
            if subset.contains(&id) {
                let canonical = operation.canonical_bytes().to_vec();
                bytes = bytes
                    .checked_add(canonical.len())
                    .ok_or(MembershipError::DecodeLimit)?;
                if operations.len() >= max_operations || bytes > max_bytes {
                    return Err(MembershipError::DecodeLimit);
                }
                operations.push(canonical);
            }
            emitted.insert(id);
        }
    }
    Ok(operations)
}

/// Decides what this Installation sends `peer`, given what `peer` already has.
pub(crate) fn disclosure(
    projection: &WorkspaceProjection,
    peer: PublicIdentity,
    peer_has: &BTreeSet<MembershipOperationId>,
) -> Result<Disclosure, MembershipError> {
    if projection.contains_member(peer) {
        let subset = projection
            .operations
            .keys()
            .filter(|id| !peer_has.contains(*id))
            .copied()
            .collect();
        return Ok(Disclosure::CurrentMember(export_causal(
            projection, &subset,
        )?));
    }
    if projection.has_membership_history(peer) {
        let subset = revocation_closure(projection, peer)
            .into_iter()
            .filter(|id| !peer_has.contains(id))
            .collect();
        return Ok(Disclosure::FormerMember(export_causal(
            projection, &subset,
        )?));
    }
    Ok(Disclosure::None)
}

/// Every `Revoke` of `peer`, or removal of `peer` as an Agent key, together
/// with its transitive dependencies.
fn revocation_closure(
    projection: &WorkspaceProjection,
    peer: PublicIdentity,
) -> BTreeSet<MembershipOperationId> {
    let mut closure = BTreeSet::new();
    let mut pending = projection
        .operations
        .iter()
        .filter(|(_, operation)| {
            matches!(&operation.action, MembershipAction::Revoke { target, .. } if *target == peer)
                || matches!(&operation.action, MembershipAction::RemoveAgentKey { agent, .. } if *agent == peer)
        })
        .map(|(id, _)| *id)
        .collect::<Vec<_>>();
    while let Some(id) = pending.pop() {
        if !closure.insert(id) {
            continue;
        }
        if let Some(operation) = projection.operations.get(&id) {
            pending.extend(operation.dependencies.iter().copied());
        }
    }
    closure
}

#[cfg(test)]
mod tests {
    use super::*;

    use iroh_base::SecretKey;

    use crate::WorkspaceId;
    use crate::membership::envelope::MembershipEnvelope;

    fn identity(seed: u8) -> PublicIdentity {
        PublicIdentity::from_bytes(*SecretKey::from_bytes(&[seed; 32]).public().as_bytes()).unwrap()
    }

    fn id(seed: u8) -> MembershipOperationId {
        MembershipOperationId::digest([seed])
    }

    /// Builds a projection directly from envelope stand-ins, so the export
    /// order can be checked against every branch shape without storage.
    fn projection_with(
        members: &[u8],
        operations: Vec<(MembershipOperationId, MembershipEnvelope)>,
    ) -> WorkspaceProjection {
        let workspace_id = WorkspaceId::from_bytes([7; 16]);
        WorkspaceProjection::for_replication_test(
            workspace_id,
            identity(1),
            operations
                .first()
                .map_or_else(|| id(0), |(first, _)| *first),
            operations.into_iter().collect(),
            members.iter().map(|seed| identity(*seed)).collect(),
        )
    }

    fn envelope(
        author: u8,
        action: MembershipAction,
        dependencies: Vec<MembershipOperationId>,
    ) -> MembershipEnvelope {
        MembershipEnvelope::sign(
            &SecretKey::from_bytes(&[author; 32]),
            WorkspaceId::from_bytes([7; 16]),
            1_000,
            action,
            dependencies,
        )
        .expect("a test envelope signs")
    }

    /// An `Admit` needs a real signed invitation claim, which is what gives the
    /// target membership history.
    fn admit(
        author: u8,
        target: u8,
        dependencies: Vec<MembershipOperationId>,
    ) -> MembershipEnvelope {
        let token = crate::invitation::sign_token(
            &SecretKey::from_bytes(&[author; 32]),
            WorkspaceId::from_bytes([7; 16]),
            900,
            100_000,
            [target; 32],
        )
        .expect("a test invitation token signs");
        let claim = crate::invitation::sign_claim(
            &SecretKey::from_bytes(&[target; 32]),
            &token,
            MembershipOperationId::digest(b"genesis"),
        )
        .expect("a test invitation claim signs");
        envelope(
            author,
            MembershipAction::Admit {
                target: identity(target),
                proof: Box::new(super::super::AdmissionProof { claim }),
            },
            dependencies,
        )
    }

    #[test]
    fn want_is_the_remote_ids_missing_locally() {
        let local = BTreeSet::from([id(1), id(2)]);
        let remote = [id(1), id(3), id(4)];
        assert_eq!(want(&local, &remote), vec![id(3), id(4)]);
        assert!(want(&BTreeSet::from([id(1), id(3), id(4)]), &remote).is_empty());
    }

    /// A diamond: `genesis → {left, right} → merge`. Every subset must still
    /// emit dependencies before dependents.
    #[test]
    fn export_causal_emits_dependencies_before_dependents_for_every_subset() {
        let genesis = envelope(1, MembershipAction::Create, Vec::new());
        let left = envelope(
            1,
            MembershipAction::Revoke {
                target: identity(2),
                group: id(2),
            },
            vec![genesis.id],
        );
        let right = envelope(
            1,
            MembershipAction::Revoke {
                target: identity(3),
                group: id(3),
            },
            vec![genesis.id],
        );
        let merge = envelope(
            1,
            MembershipAction::Revoke {
                target: identity(4),
                group: id(4),
            },
            vec![left.id, right.id],
        );
        let all = [
            (genesis.id, genesis.clone()),
            (left.id, left.clone()),
            (right.id, right.clone()),
            (merge.id, merge.clone()),
        ];
        let projection = projection_with(&[1], all.to_vec());
        let ids = [genesis.id, left.id, right.id, merge.id];

        for mask in 0..16_u8 {
            let subset = ids
                .iter()
                .enumerate()
                .filter(|(index, _)| mask & (1 << index) != 0)
                .map(|(_, id)| *id)
                .collect::<BTreeSet<_>>();
            let exported = export_causal(&projection, &subset).unwrap();
            assert_eq!(exported.len(), subset.len());
            let mut seen = BTreeSet::new();
            for bytes in exported {
                let decoded = MembershipEnvelope::decode(&bytes).unwrap();
                for dependency in &decoded.dependencies {
                    assert!(
                        !subset.contains(dependency) || seen.contains(dependency),
                        "a dependency inside the subset was emitted after its dependent"
                    );
                }
                seen.insert(decoded.id);
            }
        }
    }

    #[test]
    fn export_causal_enforces_operation_and_byte_limits() {
        let genesis = envelope(1, MembershipAction::Create, Vec::new());
        let revoke = envelope(
            1,
            MembershipAction::Revoke {
                target: identity(2),
                group: id(2),
            },
            vec![genesis.id],
        );
        let projection = projection_with(
            &[1],
            vec![(genesis.id, genesis.clone()), (revoke.id, revoke.clone())],
        );
        let subset = BTreeSet::from([genesis.id, revoke.id]);

        assert_eq!(
            export_causal_with_limits(&projection, &subset, 1, MAX_REPLICATION_BYTES),
            Err(MembershipError::DecodeLimit)
        );
        assert_eq!(
            export_causal_with_limits(&projection, &subset, MAX_REPLICATION_OPERATIONS, 8),
            Err(MembershipError::DecodeLimit)
        );
        assert_eq!(
            export_causal(&projection, &subset).unwrap().len(),
            2,
            "the shipped limits admit an ordinary workspace"
        );
    }

    #[test]
    fn disclosure_serves_everything_missing_to_a_current_member() {
        let genesis = envelope(1, MembershipAction::Create, Vec::new());
        let revoke = envelope(
            1,
            MembershipAction::Revoke {
                target: identity(9),
                group: id(9),
            },
            vec![genesis.id],
        );
        let projection = projection_with(
            &[1, 2],
            vec![(genesis.id, genesis.clone()), (revoke.id, revoke.clone())],
        );

        let everything = disclosure(&projection, identity(2), &BTreeSet::new()).unwrap();
        assert_eq!(everything.operations().len(), 2);
        assert!(matches!(everything, Disclosure::CurrentMember(_)));

        let partial = disclosure(&projection, identity(2), &BTreeSet::from([genesis.id])).unwrap();
        assert_eq!(partial.operations().len(), 1);
    }

    #[test]
    fn disclosure_serves_only_the_revocation_closure_to_a_former_member() {
        let genesis = envelope(1, MembershipAction::Create, Vec::new());
        let admitted = admit(1, 2, vec![genesis.id]);
        let revoke = envelope(
            1,
            MembershipAction::Revoke {
                target: identity(2),
                group: id(2),
            },
            vec![admitted.id],
        );
        let later = admit(1, 3, vec![revoke.id]);
        let projection = projection_with(
            &[1],
            vec![
                (genesis.id, genesis.clone()),
                (admitted.id, admitted.clone()),
                (revoke.id, revoke.clone()),
                (later.id, later.clone()),
            ],
        );
        // Identity 2 was admitted and then revoked, so it is a former member.
        let disclosed = disclosure(&projection, identity(2), &BTreeSet::new()).unwrap();

        assert!(matches!(disclosed, Disclosure::FormerMember(_)));
        let ids = disclosed
            .operations()
            .iter()
            .map(|bytes| MembershipEnvelope::decode(bytes).unwrap().id)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            ids,
            BTreeSet::from([genesis.id, admitted.id, revoke.id]),
            "the closure is the revocation and everything it causally depends on"
        );
        assert!(
            !ids.contains(&later.id),
            "nothing authored after the revocation is disclosed"
        );
    }

    #[test]
    fn disclosure_serves_nothing_to_a_stranger() {
        let genesis = envelope(1, MembershipAction::Create, Vec::new());
        let projection = projection_with(&[1], vec![(genesis.id, genesis.clone())]);
        let disclosed = disclosure(&projection, identity(8), &BTreeSet::new()).unwrap();
        assert_eq!(disclosed, Disclosure::None);
        assert!(disclosed.operations().is_empty());
    }
}
