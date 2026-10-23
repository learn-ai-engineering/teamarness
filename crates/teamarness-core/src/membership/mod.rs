mod adapter;
mod authority;
pub(crate) mod envelope;
mod history;
mod id;
pub(crate) mod projection;
pub(crate) mod replication;
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub mod test_authoring;
mod validation;

use serde::{Deserialize, Serialize};

use crate::{PublicIdentity, WorkspaceId};

pub(crate) use adapter::OperationAdapter;
pub(crate) use authority::Ledger;
pub(crate) use envelope::{AdmissionProof, AgentKeyProof, MembershipAction, MembershipEnvelope};
pub(crate) use history::was_member;
pub use id::MembershipOperationId;
pub(crate) use projection::WorkspaceProjection;
pub(crate) use validation::{authenticate, authenticate_canonical_copy, validate_context};

pub(crate) const INSTALLATION_TAG: u8 = 1;
pub(crate) const WORKSPACE_TAG: u8 = 2;
pub(crate) const PERSON_GROUP_TAG: u8 = 3;

/// Tagged local identifier required because p2panda uses one author type for
/// individual actors and groups.
///
/// A `Person` is a Workspace Person's key group, identified by the operation
/// that created it. It nests inside the Workspace group and holds the Person's
/// Installation key as its manager and its Agent keys as writers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) enum AuthId {
    Installation([u8; 32]),
    Workspace([u8; 16]),
    Person([u8; 32]),
}

impl AuthId {
    pub(crate) fn installation(identity: PublicIdentity) -> Self {
        Self::Installation(*identity.as_bytes())
    }

    pub(crate) fn workspace(workspace_id: WorkspaceId) -> Self {
        Self::Workspace(*workspace_id.as_bytes())
    }

    pub(crate) fn person(group: MembershipOperationId) -> Self {
        Self::Person(*group.as_bytes())
    }
}

impl p2panda_core::traits::Author for AuthId {}
