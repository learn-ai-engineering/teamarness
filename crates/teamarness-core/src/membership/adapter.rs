use p2panda_auth::Access;
use p2panda_auth::group::{GroupAction, GroupMember};
use p2panda_auth::traits::Operation;
use p2panda_core::Hash;

use super::{AuthId, MembershipAction, MembershipEnvelope};

/// Maps one authenticated envelope onto the p2panda group operation it is.
///
/// Workspace-group actions name the Workspace as their group and add or remove
/// a Person's key group. Person-group actions name the Person group, which is
/// the identifier of the `create person` operation that made it.
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
        match &self.0.action {
            MembershipAction::CreatePerson { .. } => AuthId::person(self.0.id),
            MembershipAction::AddAgentKey { group, .. }
            | MembershipAction::RemoveAgentKey { group, .. } => AuthId::person(*group),
            MembershipAction::Create
            | MembershipAction::Admit { .. }
            | MembershipAction::AttachPerson { .. }
            | MembershipAction::Revoke { .. } => AuthId::workspace(self.0.workspace_id),
        }
    }

    fn action(&self) -> GroupAction<AuthId, ()> {
        match &self.0.action {
            MembershipAction::Create => GroupAction::Create {
                initial_members: vec![(
                    GroupMember::Individual(AuthId::installation(self.0.author)),
                    Access::manage(),
                )],
            },
            MembershipAction::CreatePerson { creator } => GroupAction::Create {
                initial_members: std::iter::once(self.0.author)
                    .chain((*creator != self.0.author).then_some(*creator))
                    .map(|key| {
                        (
                            GroupMember::Individual(AuthId::installation(key)),
                            Access::manage(),
                        )
                    })
                    .collect(),
            },
            MembershipAction::Admit { proof, .. } => GroupAction::Add {
                member: GroupMember::Group(AuthId::person(proof.claim.create_person.id)),
                access: Access::write(),
            },
            MembershipAction::AttachPerson { group } => GroupAction::Add {
                member: GroupMember::Group(AuthId::person(*group)),
                access: Access::write(),
            },
            MembershipAction::Revoke { group, .. } => GroupAction::Remove {
                member: GroupMember::Group(AuthId::person(*group)),
            },
            MembershipAction::AddAgentKey { agent, .. } => GroupAction::Add {
                member: GroupMember::Individual(AuthId::installation(*agent)),
                access: Access::write(),
            },
            MembershipAction::RemoveAgentKey { agent, .. } => GroupAction::Remove {
                member: GroupMember::Individual(AuthId::installation(*agent)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use iroh_base::SecretKey;
    use p2panda_auth::group::resolver::StrongRemove;
    use p2panda_auth::group::{GroupAction, GroupCrdt, GroupMember};
    use p2panda_auth::traits::Operation;

    use super::*;
    use crate::invitation::{sign_claim, sign_token};
    use crate::membership::{AdmissionProof, AgentKeyProof};
    use crate::{MembershipOperationId, PublicIdentity, WorkspaceId};

    fn secret(byte: u8) -> SecretKey {
        SecretKey::from_bytes(&[byte; 32])
    }

    fn identity(byte: u8) -> PublicIdentity {
        PublicIdentity::from_bytes(*secret(byte).public().as_bytes()).unwrap()
    }

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

    #[test]
    fn creator_person_group_lists_its_manager_once() {
        let workspace = WorkspaceId::from_bytes([5; 16]);
        let genesis =
            MembershipEnvelope::sign(&secret(1), workspace, 10, MembershipAction::Create, vec![])
                .unwrap();
        let create = MembershipEnvelope::sign(
            &secret(1),
            workspace,
            20,
            MembershipAction::CreatePerson {
                creator: identity(1),
            },
            vec![genesis.id],
        )
        .unwrap();
        assert!(
            matches!(OperationAdapter(create).action(), GroupAction::Create { initial_members } if initial_members.len() == 1)
        );
    }

    /// A probe operation: the real adapter, or a deliberately wrong mapping
    /// used to confirm a library refusal.
    #[derive(Clone)]
    enum Probe {
        Real(OperationAdapter),
        ManagerGroupAdd(MembershipEnvelope, MembershipOperationId),
        FormerSoleManagerCreate(OperationAdapter),
    }

    impl Operation<AuthId, Hash, ()> for Probe {
        fn id(&self) -> Hash {
            match self {
                Self::Real(adapter) => adapter.id(),
                Self::ManagerGroupAdd(envelope, _) => envelope.id.as_hash(),
                Self::FormerSoleManagerCreate(adapter) => adapter.id(),
            }
        }
        fn author(&self) -> AuthId {
            match self {
                Self::Real(adapter) => adapter.author(),
                Self::ManagerGroupAdd(envelope, _) => AuthId::installation(envelope.author),
                Self::FormerSoleManagerCreate(adapter) => adapter.author(),
            }
        }
        fn dependencies(&self) -> Vec<Hash> {
            match self {
                Self::Real(adapter) => adapter.dependencies(),
                Self::ManagerGroupAdd(envelope, _) => {
                    envelope.dependencies.iter().map(|d| d.as_hash()).collect()
                }
                Self::FormerSoleManagerCreate(adapter) => adapter.dependencies(),
            }
        }
        fn group_id(&self) -> AuthId {
            match self {
                Self::Real(adapter) => adapter.group_id(),
                Self::ManagerGroupAdd(envelope, _) => AuthId::workspace(envelope.workspace_id),
                Self::FormerSoleManagerCreate(adapter) => adapter.group_id(),
            }
        }
        fn action(&self) -> GroupAction<AuthId, ()> {
            match self {
                Self::Real(adapter) => adapter.action(),
                Self::ManagerGroupAdd(_, group) => GroupAction::Add {
                    member: GroupMember::Group(AuthId::person(*group)),
                    access: Access::manage(),
                },
                Self::FormerSoleManagerCreate(adapter) => GroupAction::Create {
                    initial_members: vec![(
                        GroupMember::Individual(adapter.author()),
                        Access::manage(),
                    )],
                },
            }
        }
    }

    type ProbeCrdt = GroupCrdt<AuthId, Hash, Probe, (), StrongRemove<AuthId, Hash, Probe, ()>>;
    type ProbeState = p2panda_auth::group::GroupCrdtState<AuthId, Hash, Probe, ()>;

    /// Replays `operations` from an empty state, so a step that must fail can
    /// be tried without needing the library state to be clonable.
    fn build(operations: &[&MembershipEnvelope]) -> ProbeState {
        operations.iter().fold(
            ProbeCrdt::init(),
            |state, envelope| match ProbeCrdt::process(
                state,
                &Probe::Real(OperationAdapter((*envelope).clone())),
            ) {
                Ok(state) => state,
                Err(_) => panic!("the probe prefix is accepted by the library"),
            },
        )
    }

    fn members_of(state: &ProbeState, workspace: WorkspaceId) -> BTreeSet<(AuthId, bool)> {
        state
            .members(AuthId::workspace(workspace))
            .into_iter()
            .map(|(id, access)| (id, access.is_manage()))
            .collect()
    }

    #[test]
    fn p2panda_probe_two_individual_managers_remove_one_agent() {
        let workspace = WorkspaceId::from_bytes([5; 16]);
        let creator = secret(1);
        let steward = secret(2);
        let agent = secret(3);
        let sibling = secret(4);
        let genesis =
            MembershipEnvelope::sign(&creator, workspace, 10, MembershipAction::Create, vec![])
                .unwrap();
        let create = MembershipEnvelope::sign(
            &steward,
            workspace,
            20,
            MembershipAction::CreatePerson {
                creator: identity(1),
            },
            vec![genesis.id],
        )
        .unwrap();
        let group = create.id;
        let old_state = ProbeCrdt::process(
            build(&[&genesis]),
            &Probe::FormerSoleManagerCreate(OperationAdapter(create.clone())),
        )
        .unwrap_or_else(|_| panic!("former create must succeed"));
        let old_admit = MembershipEnvelope::sign(
            &creator,
            workspace,
            30,
            MembershipAction::AttachPerson { group },
            vec![group],
        )
        .unwrap();
        let old_state =
            ProbeCrdt::process(old_state, &Probe::Real(OperationAdapter(old_admit.clone())))
                .unwrap_or_else(|_| panic!("former attach must succeed"));
        let old_add = MembershipEnvelope::sign(
            &steward,
            workspace,
            40,
            MembershipAction::AddAgentKey {
                group,
                agent: identity(3),
                proof: AgentKeyProof::sign(&agent, workspace, group, identity(2)).unwrap(),
            },
            vec![old_admit.id],
        )
        .unwrap();
        let old_state =
            ProbeCrdt::process(old_state, &Probe::Real(OperationAdapter(old_add.clone())))
                .unwrap_or_else(|_| panic!("former add must succeed"));
        let old_remove = MembershipEnvelope::sign(
            &creator,
            workspace,
            50,
            MembershipAction::RemoveAgentKey {
                group,
                agent: identity(3),
            },
            vec![old_add.id],
        )
        .unwrap();
        assert!(ProbeCrdt::process(old_state, &Probe::Real(OperationAdapter(old_remove))).is_err());

        let state = ProbeCrdt::process(build(&[&genesis]), &Probe::Real(OperationAdapter(create)))
            .unwrap_or_else(|_| panic!("two-manager probe step must succeed"));
        assert_eq!(state.members(AuthId::person(group)).len(), 2);
        assert_eq!(
            members_of(&state, workspace),
            BTreeSet::from([(AuthId::installation(identity(1)), true)])
        );
        let admit = MembershipEnvelope::sign(
            &creator,
            workspace,
            30,
            MembershipAction::AttachPerson { group },
            vec![group],
        )
        .unwrap();
        let state = ProbeCrdt::process(state, &Probe::Real(OperationAdapter(admit.clone())))
            .unwrap_or_else(|_| panic!("two-manager probe step must succeed"));
        let add = MembershipEnvelope::sign(
            &steward,
            workspace,
            40,
            MembershipAction::AddAgentKey {
                group,
                agent: identity(3),
                proof: AgentKeyProof::sign(&agent, workspace, group, identity(2)).unwrap(),
            },
            vec![admit.id],
        )
        .unwrap();
        let state = ProbeCrdt::process(state, &Probe::Real(OperationAdapter(add.clone())))
            .unwrap_or_else(|_| panic!("two-manager probe step must succeed"));
        let add_sibling = MembershipEnvelope::sign(
            &steward,
            workspace,
            41,
            MembershipAction::AddAgentKey {
                group,
                agent: identity(4),
                proof: AgentKeyProof::sign(&sibling, workspace, group, identity(2)).unwrap(),
            },
            vec![add.id],
        )
        .unwrap();
        let state = ProbeCrdt::process(state, &Probe::Real(OperationAdapter(add_sibling.clone())))
            .unwrap_or_else(|_| panic!("two-manager probe step must succeed"));
        let remove = MembershipEnvelope::sign(
            &creator,
            workspace,
            50,
            MembershipAction::RemoveAgentKey {
                group,
                agent: identity(3),
            },
            vec![add_sibling.id],
        )
        .unwrap();
        let state = ProbeCrdt::process(state, &Probe::Real(OperationAdapter(remove)))
            .unwrap_or_else(|_| panic!("two-manager probe step must succeed"));
        assert_eq!(
            members_of(&state, workspace),
            BTreeSet::from([
                (AuthId::installation(identity(1)), true),
                (AuthId::installation(identity(2)), false),
                (AuthId::installation(identity(4)), false)
            ])
        );
    }

    #[test]
    fn p2panda_probe_concurrent_signed_cross_group_agent_claims() {
        let workspace = WorkspaceId::from_bytes([5; 16]);
        let genesis =
            MembershipEnvelope::sign(&secret(1), workspace, 10, MembershipAction::Create, vec![])
                .unwrap();
        let mut prefix = vec![genesis.clone()];
        let mut attachments = Vec::new();
        for steward in [2, 3] {
            let create = MembershipEnvelope::sign(
                &secret(steward),
                workspace,
                20,
                MembershipAction::CreatePerson {
                    creator: identity(1),
                },
                vec![genesis.id],
            )
            .unwrap();
            let attach = MembershipEnvelope::sign(
                &secret(1),
                workspace,
                30,
                MembershipAction::AttachPerson { group: create.id },
                vec![create.id],
            )
            .unwrap();
            prefix.push(create);
            prefix.push(attach.clone());
            attachments.push(attach);
        }
        let claims = [2, 3].map(|steward| {
            let group = match attachments[usize::from(steward - 2)].action {
                MembershipAction::AttachPerson { group } => group,
                _ => unreachable!(),
            };
            MembershipEnvelope::sign(
                &secret(steward),
                workspace,
                40,
                MembershipAction::AddAgentKey {
                    group,
                    agent: identity(4),
                    proof: AgentKeyProof::sign(&secret(4), workspace, group, identity(steward))
                        .unwrap(),
                },
                vec![attachments[usize::from(steward - 2)].id],
            )
            .unwrap()
        });
        for order in [[0, 1], [1, 0]] {
            let mut state = build(&prefix.iter().collect::<Vec<_>>());
            for index in order {
                state = ProbeCrdt::process(
                    state,
                    &Probe::Real(OperationAdapter(claims[index].clone())),
                )
                .unwrap_or_else(|_| {
                    panic!("both independently valid signed claims must remain processable")
                });
            }
            assert!(
                members_of(&state, workspace).contains(&(AuthId::installation(identity(4)), false))
            );
        }
        let group = match attachments[0].action {
            MembershipAction::AttachPerson { group } => group,
            _ => unreachable!(),
        };
        let independent = MembershipEnvelope::sign(
            &secret(2),
            workspace,
            41,
            MembershipAction::AddAgentKey {
                group,
                agent: identity(4),
                proof: AgentKeyProof::sign(&secret(4), workspace, group, identity(2)).unwrap(),
            },
            vec![attachments[0].id],
        )
        .unwrap();
        let state = ProbeCrdt::process(
            build(&prefix.iter().collect::<Vec<_>>()),
            &Probe::Real(OperationAdapter(claims[0].clone())),
        )
        .unwrap_or_else(|_| panic!("first same-group claim is valid"));
        ProbeCrdt::process(state, &Probe::Real(OperationAdapter(independent)))
            .unwrap_or_else(|_| panic!("independent same-group add is processable"));
    }

    /// Drives `GroupCrdt` directly with the adapter's output, so the nested
    /// group semantics the design relies on are confirmed against the library
    /// rather than inferred from Teamarness replay.
    #[test]
    fn p2panda_probe_nested_person_groups_flatten_add_remove_and_revoke() {
        let workspace = WorkspaceId::from_bytes([5; 16]);
        let creator = secret(1);
        let joiner = secret(2);
        let agent = secret(3);
        let creator_id = AuthId::installation(identity(1));
        let joiner_id = AuthId::installation(identity(2));
        let agent_id = AuthId::installation(identity(3));

        // Creator genesis: the creator is the Workspace group's sole manager.
        let genesis =
            MembershipEnvelope::sign(&creator, workspace, 10, MembershipAction::Create, vec![])
                .unwrap();
        assert_eq!(
            members_of(&build(&[&genesis]), workspace),
            BTreeSet::from([(creator_id, true)])
        );

        // Group create by the joiner: a new nested group with the joiner as
        // its manager, not yet visible in the Workspace.
        let token = sign_token(&creator, workspace, 20, 1_000, [7; 32]).unwrap();
        let claim = sign_claim(&joiner, &token, genesis.id).unwrap();
        let create_person = claim.create_person.clone();
        let group = create_person.id;
        let state = build(&[&genesis, &create_person]);
        assert_eq!(
            members_of(&state, workspace),
            BTreeSet::from([(creator_id, true)])
        );
        assert_eq!(
            state
                .members(AuthId::person(group))
                .into_iter()
                .map(|(id, access)| (id, access.is_manage()))
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([(joiner_id, true), (creator_id, true)])
        );

        // Group admit by the creator: the joiner appears transitively as a
        // writer, capped by the group's write access.
        let admit = MembershipEnvelope::sign(
            &creator,
            workspace,
            30,
            MembershipAction::Admit {
                target: identity(2),
                proof: Box::new(AdmissionProof { claim }),
            },
            vec![genesis.id, group],
        )
        .unwrap();
        assert_eq!(
            members_of(&build(&[&genesis, &create_person, &admit]), workspace),
            BTreeSet::from([(creator_id, true), (joiner_id, false)])
        );

        // Agent add inside the Person group by its manager: the agent is a
        // transitive Workspace member with write access.
        let add_agent = MembershipEnvelope::sign(
            &joiner,
            workspace,
            40,
            MembershipAction::AddAgentKey {
                group,
                agent: identity(3),
                proof: AgentKeyProof::sign(&agent, workspace, group, identity(2)).unwrap(),
            },
            vec![admit.id],
        )
        .unwrap();
        let prefix = [&genesis, &create_person, &admit, &add_agent];
        assert_eq!(
            members_of(&build(&prefix), workspace),
            BTreeSet::from([(creator_id, true), (joiner_id, false), (agent_id, false)])
        );

        // The Workspace creator co-manages the Person group and can remove
        // one Agent directly without detaching the Person.
        let manager_remove = MembershipEnvelope::sign(
            &creator,
            workspace,
            45,
            MembershipAction::RemoveAgentKey {
                group,
                agent: identity(3),
            },
            vec![add_agent.id],
        )
        .unwrap();
        let manager_removed = ProbeCrdt::process(
            build(&prefix),
            &Probe::Real(OperationAdapter(manager_remove)),
        )
        .unwrap_or_else(|_| panic!("co-manager removal must succeed"));
        assert_eq!(
            members_of(&manager_removed, workspace),
            BTreeSet::from([(creator_id, true), (joiner_id, false)])
        );

        // Agent remove by the Person: the agent leaves the Workspace.
        let remove_agent = MembershipEnvelope::sign(
            &joiner,
            workspace,
            50,
            MembershipAction::RemoveAgentKey {
                group,
                agent: identity(3),
            },
            vec![add_agent.id],
        )
        .unwrap();
        let prefix = [&genesis, &create_person, &admit, &add_agent, &remove_agent];
        assert_eq!(
            members_of(&build(&prefix), workspace),
            BTreeSet::from([(creator_id, true), (joiner_id, false)])
        );

        // Re-add, then group revoke by the creator: the Person and its agent
        // are both gone from the Workspace although the Person group itself
        // still lists them.
        let add_again = MembershipEnvelope::sign(
            &joiner,
            workspace,
            60,
            MembershipAction::AddAgentKey {
                group,
                agent: identity(3),
                proof: AgentKeyProof::sign(&agent, workspace, group, identity(2)).unwrap(),
            },
            vec![remove_agent.id],
        )
        .unwrap();
        let revoke = MembershipEnvelope::sign(
            &creator,
            workspace,
            70,
            MembershipAction::Revoke {
                target: identity(2),
                group,
            },
            vec![add_again.id],
        )
        .unwrap();
        let state = build(&[
            &genesis,
            &create_person,
            &admit,
            &add_agent,
            &remove_agent,
            &add_again,
            &revoke,
        ]);
        assert_eq!(
            members_of(&state, workspace),
            BTreeSet::from([(creator_id, true)])
        );
        assert_eq!(state.members(AuthId::person(group)).len(), 3);

        // A group holding manage access is refused by the library, which is
        // why the creator stays an individual manager beside its own group.
        let manager_group = MembershipEnvelope::sign(
            &creator,
            workspace,
            80,
            MembershipAction::AttachPerson { group },
            vec![revoke.id],
        )
        .unwrap();
        let state = build(&[
            &genesis,
            &create_person,
            &admit,
            &add_agent,
            &remove_agent,
            &add_again,
            &revoke,
        ]);
        assert!(ProbeCrdt::process(state, &Probe::ManagerGroupAdd(manager_group, group)).is_err());
    }
}
