mod adapter;
mod envelope;
mod history;
mod id;
pub(crate) mod projection;
pub(crate) mod sync;
mod validation;

use serde::{Deserialize, Serialize};

use crate::{PublicIdentity, WorkspaceId};

pub(crate) use adapter::OperationAdapter;
pub(crate) use envelope::{AdmissionProof, MembershipAction, MembershipEnvelope};
pub(crate) use history::was_member;
pub use id::MembershipOperationId;
pub(crate) use projection::WorkspaceProjection;
pub(crate) use validation::{authenticate, authenticate_canonical_copy, validate_context};

pub(crate) const INSTALLATION_TAG: u8 = 1;
pub(crate) const WORKSPACE_TAG: u8 = 2;

/// Tagged local identifier required because p2panda uses one author type for
/// individual actors and groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) enum AuthId {
    Installation([u8; 32]),
    Workspace([u8; 16]),
}

impl AuthId {
    pub(crate) fn installation(identity: PublicIdentity) -> Self {
        Self::Installation(*identity.as_bytes())
    }

    pub(crate) fn workspace(workspace_id: WorkspaceId) -> Self {
        Self::Workspace(*workspace_id.as_bytes())
    }
}

impl p2panda_core::traits::Author for AuthId {}
