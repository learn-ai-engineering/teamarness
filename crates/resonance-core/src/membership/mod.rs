mod adapter;
mod envelope;
mod id;
mod validation;

use serde::{Deserialize, Serialize};

use crate::{PublicIdentity, WorkspaceId};

pub(crate) use adapter::OperationAdapter;
pub(crate) use envelope::{MembershipAction, MembershipEnvelope};
pub use id::MembershipOperationId;
pub(crate) use validation::{authenticate, validate_context};

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
