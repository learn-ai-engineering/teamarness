use p2panda_auth::Access;
use p2panda_auth::group::{GroupAction, GroupMember};
use p2panda_auth::traits::Operation;
use p2panda_core::Hash;

use super::{AuthId, MembershipAction, MembershipEnvelope};

#[derive(Clone)]
pub(crate) struct OperationAdapter(pub(crate) MembershipEnvelope);

impl Operation<AuthId, Hash, ()> for OperationAdapter {
    fn id(&self) -> Hash {
        self.0.id.as_hash()
    }

    fn author(&self) -> AuthId {
        AuthId::installation(self.0.author)
    }

    fn dependencies(&self) -> Vec<Hash> {
        self.0
            .dependencies
            .iter()
            .map(|dependency| dependency.as_hash())
            .collect()
    }

    fn group_id(&self) -> AuthId {
        AuthId::workspace(self.0.workspace_id)
    }

    fn action(&self) -> GroupAction<AuthId, ()> {
        match &self.0.action {
            MembershipAction::Create => GroupAction::Create {
                initial_members: vec![(
                    GroupMember::Individual(AuthId::installation(self.0.author)),
                    Access::manage(),
                )],
            },
            MembershipAction::Admit { target, .. } => GroupAction::Add {
                member: GroupMember::Individual(AuthId::installation(*target)),
                access: Access::write(),
            },
            MembershipAction::Revoke { target } => GroupAction::Remove {
                member: GroupMember::Individual(AuthId::installation(*target)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use iroh_base::SecretKey;
    use p2panda_auth::group::{GroupAction, GroupMember};
    use p2panda_auth::traits::Operation;

    use super::*;
    use crate::WorkspaceId;

    #[test]
    fn adapter_returns_owned_p2panda_values_with_distinct_roles() {
        let secret = SecretKey::from_bytes(&[4; 32]);
        let envelope = MembershipEnvelope::sign(
            &secret,
            WorkspaceId::from_bytes([5; 16]),
            42,
            MembershipAction::Create,
            vec![],
        )
        .unwrap();
        let adapter = OperationAdapter(envelope.clone());

        assert_eq!(adapter.id(), envelope.id.as_hash());
        assert_eq!(
            adapter.author(),
            AuthId::Installation(*secret.public().as_bytes())
        );
        assert_eq!(adapter.group_id(), AuthId::Workspace([5; 16]));
        assert!(adapter.dependencies().is_empty());
        assert!(matches!(
            adapter.action(),
            GroupAction::Create { initial_members }
                if initial_members == vec![(
                    GroupMember::Individual(AuthId::Installation(*secret.public().as_bytes())),
                    Access::manage(),
                )]
        ));
    }
}
