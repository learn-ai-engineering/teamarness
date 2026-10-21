//! Action-specific replay authority.
//!
//! Every rule here is evaluated against the operation's own causal ancestry,
//! never against whatever else happens to be in the accepted set, so two
//! Installations that hold the same operations reach the same verdicts in any
//! application order (MEM-R07). The p2panda group CRDT then enforces group
//! access itself; these rules add what Teamarness requires on top: invitation
//! proofs, one group per key, no readmission, and Agent keys that never author.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::MembershipError;
use crate::{MembershipOperationId, PublicIdentity};

use super::{MembershipAction, MembershipEnvelope};

/// What one operation's causal ancestry says about groups and keys. Every set
/// is "ever happened", which makes merging ancestries a plain union.
#[derive(Clone, Debug, Default)]
pub(crate) struct Ledger {
    /// Person groups created, by the key that manages each.
    groups: BTreeMap<MembershipOperationId, PublicIdentity>,
    /// Groups ever attached to the Workspace through `admit` or `attach person`.
    attached: BTreeSet<MembershipOperationId>,
    /// Groups ever revoked from the Workspace.
    revoked: BTreeSet<MembershipOperationId>,
    /// Keys ever added, with the group each entered through.
    added_keys: BTreeMap<PublicIdentity, MembershipOperationId>,
    /// Agent keys ever removed from their group.
    removed_keys: BTreeSet<PublicIdentity>,
    /// Keys ever added as Agent keys; they may never author.
    agents: BTreeSet<PublicIdentity>,
}

impl Ledger {
    /// The ledger of everything causally before `operation`.
    pub(crate) fn ancestry_of(
        operation: &MembershipEnvelope,
        operations: &BTreeMap<MembershipOperationId, MembershipEnvelope>,
        mut check: impl FnMut() -> Result<(), MembershipError>,
    ) -> Result<Self, MembershipError> {
        let mut ledger = Self::default();
        let mut pending = operation.dependencies.clone();
        let mut visited = BTreeSet::new();
        while let Some(id) = pending.pop() {
            check()?;
            if !visited.insert(id) {
                continue;
            }
            let Some(ancestor) = operations.get(&id) else {
                continue;
            };
            ledger.record(ancestor);
            pending.extend(ancestor.dependencies.iter().copied());
        }
        Ok(ledger)
    }

    /// Records one accepted operation's effect.
    pub(crate) fn record(&mut self, operation: &MembershipEnvelope) {
        match &operation.action {
            MembershipAction::Create => {}
            MembershipAction::CreatePerson { .. } => {
                self.groups.insert(operation.id, operation.author);
            }
            MembershipAction::Admit { target, proof } => {
                let group = proof.claim.create_person.id;
                self.attached.insert(group);
                self.added_keys.insert(*target, group);
            }
            MembershipAction::AttachPerson { group } => {
                self.attached.insert(*group);
                self.added_keys.insert(operation.author, *group);
            }
            MembershipAction::Revoke { group, .. } => {
                self.revoked.insert(*group);
            }
            MembershipAction::AddAgentKey { group, agent, .. } => {
                self.added_keys.insert(*agent, *group);
                self.agents.insert(*agent);
            }
            MembershipAction::RemoveAgentKey { agent, .. } => {
                self.removed_keys.insert(*agent);
            }
        }
    }

    fn group_admitted(&self, group: MembershipOperationId) -> bool {
        self.attached.contains(&group) && !self.revoked.contains(&group)
    }

    fn key_is_member(&self, key: PublicIdentity, creator: PublicIdentity) -> bool {
        if key == creator {
            return true;
        }
        self.added_keys
            .get(&key)
            .is_some_and(|group| self.group_admitted(*group) && !self.removed_keys.contains(&key))
    }

    /// A key may enter a group only once, ever: a current member is a
    /// duplicate, a former one is a readmission.
    fn admit_key(
        &self,
        key: PublicIdentity,
        creator: PublicIdentity,
    ) -> Result<(), MembershipError> {
        if key == creator {
            return Err(MembershipError::DuplicateMemberKey);
        }
        if !self.added_keys.contains_key(&key) {
            return Ok(());
        }
        if self.key_is_member(key, creator) {
            Err(MembershipError::DuplicateMemberKey)
        } else {
            Err(MembershipError::CausalReadmission)
        }
    }

    /// The group must exist in the ancestry and be managed by `manager`.
    fn managed_group(
        &self,
        group: MembershipOperationId,
        manager: PublicIdentity,
    ) -> Result<(), MembershipError> {
        match self.groups.get(&group) {
            None => Err(MembershipError::UnknownPersonGroup),
            Some(author) if *author == manager => Ok(()),
            Some(_) => Err(MembershipError::NotManager),
        }
    }
}

/// Decides whether `operation` holds the authority its action requires at its
/// causal position. The p2panda CRDT still applies its own group rules after.
pub(crate) fn authorize(
    operation: &MembershipEnvelope,
    creator: PublicIdentity,
    ancestry: &Ledger,
) -> Result<(), MembershipError> {
    if ancestry.agents.contains(&operation.author) {
        return Err(MembershipError::AgentKeyCannotAuthor);
    }
    match &operation.action {
        MembershipAction::Create => Err(MembershipError::ConflictingGenesis),
        // A group create grants nothing until a Manager attaches it, so it
        // needs no prior standing; the attachment carries every check.
        MembershipAction::CreatePerson {
            creator: co_manager,
        } => {
            if *co_manager != creator {
                return Err(MembershipError::NotManager);
            }
            Ok(())
        }
        MembershipAction::Admit { target, proof } => {
            if operation.author != creator {
                return Err(MembershipError::NotManager);
            }
            if proof.claim.token.inviter != operation.author
                || proof.claim.token.workspace_id != operation.workspace_id
                || proof.claim.token.issued_at_unix_ms > operation.authored_at_unix_ms
                || proof.claim.token.expires_at_unix_ms < operation.authored_at_unix_ms
                || proof.claim.invitee != *target
            {
                return Err(MembershipError::InvalidAdmissionProof);
            }
            let group = proof.claim.create_person.id;
            if !matches!(proof.claim.create_person.action, MembershipAction::CreatePerson { creator: co_manager } if co_manager == creator)
                || proof.claim.token.inviter != creator
            {
                return Err(MembershipError::InvalidAdmissionProof);
            }
            match ancestry.groups.get(&group) {
                Some(author) if *author == *target => {}
                _ => return Err(MembershipError::UnknownPersonGroup),
            }
            ancestry.admit_key(*target, creator)
        }
        MembershipAction::AttachPerson { group } => {
            if operation.author != creator {
                return Err(MembershipError::NotManager);
            }
            match ancestry.groups.get(group) {
                Some(author) if *author == creator => {}
                _ => return Err(MembershipError::UnknownPersonGroup),
            }
            // The creator is a member from genesis; it attaches one group, once.
            if ancestry.added_keys.contains_key(&creator) {
                return Err(MembershipError::DuplicateMemberKey);
            }
            Ok(())
        }
        MembershipAction::Revoke { target, group } => {
            if operation.author != creator {
                return Err(MembershipError::NotManager);
            }
            if *target == creator {
                return Err(MembershipError::CannotRevokeManager);
            }
            match ancestry.groups.get(group) {
                Some(author) if *author == *target => {}
                _ => return Err(MembershipError::UnknownPersonGroup),
            }
            if !ancestry.group_admitted(*group) {
                return Err(MembershipError::MemberNotFound);
            }
            Ok(())
        }
        MembershipAction::AddAgentKey {
            group,
            agent,
            proof,
        } => {
            ancestry.managed_group(*group, operation.author)?;
            if !ancestry.group_admitted(*group) {
                return Err(MembershipError::PersonNotAdmitted);
            }
            proof.verify(*agent, operation.workspace_id, *group, operation.author)?;
            ancestry.admit_key(*agent, creator)
        }
        MembershipAction::RemoveAgentKey { group, agent } => {
            if operation.author != creator {
                ancestry.managed_group(*group, operation.author)?;
            } else if !ancestry.groups.contains_key(group) {
                return Err(MembershipError::UnknownPersonGroup);
            }
            if !ancestry.group_admitted(*group) {
                return Err(MembershipError::PersonNotAdmitted);
            }
            let in_this_group = ancestry.added_keys.get(agent) == Some(group)
                && ancestry.agents.contains(agent)
                && !ancestry.removed_keys.contains(agent);
            if !in_this_group {
                return Err(MembershipError::MemberNotFound);
            }
            Ok(())
        }
    }
}

/// The Agent keys among `keys`, per the whole accepted set. A key added as an
/// Agent is never an Installation key, so the set is a plain "ever added".
pub(crate) fn agent_keys(
    operations: &BTreeMap<MembershipOperationId, MembershipEnvelope>,
) -> BTreeSet<PublicIdentity> {
    operations
        .values()
        .filter_map(|operation| match &operation.action {
            MembershipAction::AddAgentKey { agent, .. } => Some(*agent),
            _ => None,
        })
        .collect()
}
