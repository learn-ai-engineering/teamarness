use iroh_base::{SecretKey, Signature};
use p2panda_core::Hash;

use crate::encoding::ByteReader;
use crate::error::MembershipError;
use crate::invitation::{VerifiedInvitationClaim, decode_claim};
use crate::membership::{INSTALLATION_TAG, PERSON_GROUP_TAG, WORKSPACE_TAG};
use crate::{MembershipOperationId, PublicIdentity, WorkspaceId};

const VERSION: u8 = 1;
const CREATE_TAG: u8 = 0;
const ADMIT_TAG: u8 = 1;
const REVOKE_TAG: u8 = 2;
const CREATE_PERSON_TAG: u8 = 3;
const ATTACH_PERSON_TAG: u8 = 4;
const ADD_AGENT_KEY_TAG: u8 = 5;
const REMOVE_AGENT_KEY_TAG: u8 = 6;
const MEMBER_ACCESS: u8 = 1;
const DOMAIN: &str = "teamarness.membership-operation/v1";
/// The Agent key signs this domain over `workspace || person_group || steward`
/// to prove possession and bind itself to one Person's group.
const AGENT_KEY_PROOF_DOMAIN: &str = "teamarness.agent-key-proof/v1";
const MAX_DEPENDENCIES: usize = 64;
const MAX_ADMISSION_PROOF_BYTES: usize = 1_024;
const MAX_UNSIGNED_BYTES: usize = 64 * 1024;
pub(crate) const MAX_ENVELOPE_BYTES: usize = 4 + MAX_UNSIGNED_BYTES + 32 + 64;
/// A `create person` operation with one dependency has this exact canonical
/// width: header (1 + 1 + 16 + 1 + 32 + 8), action tag, dependency count and
/// one dependency, then the identifier and signature.
pub(crate) const CREATE_PERSON_ENVELOPE_BYTES: usize = 4 + (59 + 1 + 2 + 32) + 32 + 64;

#[derive(Clone)]
pub(crate) struct AdmissionProof {
    pub(crate) claim: VerifiedInvitationClaim,
}

/// Proof that the Agent key is held by whoever adds it: the Agent key signs the
/// Workspace, the Person group it joins, and the Steward key that adds it.
#[derive(Clone)]
pub(crate) struct AgentKeyProof {
    signature: Signature,
}

impl AgentKeyProof {
    pub(crate) fn sign(
        agent_secret: &SecretKey,
        workspace_id: WorkspaceId,
        group: MembershipOperationId,
        steward: PublicIdentity,
    ) -> Result<Self, MembershipError> {
        let payload = Self::payload(workspace_id, group, steward);
        let signature = crate::identity::sign(agent_secret, AGENT_KEY_PROOF_DOMAIN, &payload)
            .map_err(|_| MembershipError::InvalidAdmissionProof)?;
        Ok(Self { signature })
    }

    pub(crate) fn from_bytes(bytes: [u8; 64]) -> Self {
        Self {
            signature: Signature::from_bytes(&bytes),
        }
    }

    pub(crate) fn to_bytes(&self) -> [u8; 64] {
        self.signature.to_bytes()
    }

    pub(crate) fn verify(
        &self,
        agent: PublicIdentity,
        workspace_id: WorkspaceId,
        group: MembershipOperationId,
        steward: PublicIdentity,
    ) -> Result<(), MembershipError> {
        let payload = Self::payload(workspace_id, group, steward);
        crate::identity::verify(
            agent.as_public_key(),
            AGENT_KEY_PROOF_DOMAIN,
            &payload,
            &self.signature,
        )
        .map_err(|_| MembershipError::InvalidAdmissionProof)
    }

    fn payload(
        workspace_id: WorkspaceId,
        group: MembershipOperationId,
        steward: PublicIdentity,
    ) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(16 + 32 + 32);
        bytes.extend_from_slice(workspace_id.as_bytes());
        bytes.extend_from_slice(group.as_bytes());
        bytes.extend_from_slice(steward.as_bytes());
        bytes
    }
}

/// One membership action. Workspace-group actions (`Create`, `Admit`,
/// `AttachPerson`, `Revoke`) need manage authority on the Workspace group;
/// Person-group actions (`CreatePerson`, `AddAgentKey`, `RemoveAgentKey`) are
/// authored by the Person's own Installation key inside its key group.
#[derive(Clone)]
pub(crate) enum MembershipAction {
    Create,
    /// Adds the invitee's Person group, carried in the claim, to the Workspace.
    Admit {
        target: PublicIdentity,
        proof: Box<AdmissionProof>,
    },
    /// Removes the Person group `group`, whose Installation key is `target`.
    Revoke {
        target: PublicIdentity,
        group: MembershipOperationId,
    },
    /// Creates the author's own key group; the group is this operation's ID.
    CreatePerson,
    /// Adds the creator's own Person group to the Workspace beside genesis.
    AttachPerson {
        group: MembershipOperationId,
    },
    /// Adds an Agent key to the author's Person group with its proof of possession.
    AddAgentKey {
        group: MembershipOperationId,
        agent: PublicIdentity,
        proof: AgentKeyProof,
    },
    /// Removes an Agent key from the author's Person group.
    RemoveAgentKey {
        group: MembershipOperationId,
        agent: PublicIdentity,
    },
}

impl MembershipAction {
    /// The key whose membership this action changes, if any.
    pub(crate) fn subject(&self) -> Option<PublicIdentity> {
        match self {
            Self::Create | Self::CreatePerson | Self::AttachPerson { .. } => None,
            Self::Admit { target, .. } | Self::Revoke { target, .. } => Some(*target),
            Self::AddAgentKey { agent, .. } | Self::RemoveAgentKey { agent, .. } => Some(*agent),
        }
    }

    /// Whether this action changes who is a current Member.
    pub(crate) fn changes_roster(&self) -> bool {
        matches!(
            self,
            Self::Admit { .. }
                | Self::Revoke { .. }
                | Self::AddAgentKey { .. }
                | Self::RemoveAgentKey { .. }
        )
    }

    /// Field-level equality, used to confirm a parsed copy against its bytes.
    pub(crate) fn same_as(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Create, Self::Create) | (Self::CreatePerson, Self::CreatePerson) => true,
            (
                Self::Admit {
                    target: left_target,
                    proof: left_proof,
                },
                Self::Admit {
                    target: right_target,
                    proof: right_proof,
                },
            ) => left_target == right_target && left_proof.claim.bytes == right_proof.claim.bytes,
            (
                Self::Revoke {
                    target: left_target,
                    group: left_group,
                },
                Self::Revoke {
                    target: right_target,
                    group: right_group,
                },
            ) => left_target == right_target && left_group == right_group,
            (Self::AttachPerson { group: left }, Self::AttachPerson { group: right }) => {
                left == right
            }
            (
                Self::AddAgentKey {
                    group: left_group,
                    agent: left_agent,
                    proof: left_proof,
                },
                Self::AddAgentKey {
                    group: right_group,
                    agent: right_agent,
                    proof: right_proof,
                },
            ) => {
                left_group == right_group
                    && left_agent == right_agent
                    && left_proof.to_bytes() == right_proof.to_bytes()
            }
            (
                Self::RemoveAgentKey {
                    group: left_group,
                    agent: left_agent,
                },
                Self::RemoveAgentKey {
                    group: right_group,
                    agent: right_agent,
                },
            ) => left_group == right_group && left_agent == right_agent,
            _ => false,
        }
    }
}

#[derive(Clone)]
pub(crate) struct MembershipEnvelope {
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) author: PublicIdentity,
    pub(crate) authored_at_unix_ms: i64,
    pub(crate) action: MembershipAction,
    pub(crate) dependencies: Vec<MembershipOperationId>,
    pub(crate) id: MembershipOperationId,
    pub(crate) signature: Signature,
    /// The canonical unsigned encoding, which is the input to the operation
    /// identifier. Only the envelope tests read it back.
    #[cfg_attr(not(test), allow(dead_code))]
    unsigned_bytes: Vec<u8>,
    bytes: Vec<u8>,
}

impl MembershipEnvelope {
    pub(crate) fn sign(
        secret_key: &SecretKey,
        workspace_id: WorkspaceId,
        authored_at_unix_ms: i64,
        action: MembershipAction,
        mut dependencies: Vec<MembershipOperationId>,
    ) -> Result<Self, MembershipError> {
        dependencies.sort_unstable();
        dependencies.dedup();
        if dependencies.len() > MAX_DEPENDENCIES {
            return Err(MembershipError::DecodeLimit);
        }
        let author = PublicIdentity::from_bytes(*secret_key.public().as_bytes())
            .map_err(|_| MembershipError::InvalidRole)?;
        let unsigned_bytes = encode_unsigned(
            workspace_id,
            author,
            authored_at_unix_ms,
            &action,
            &dependencies,
        )?;
        let id = MembershipOperationId::digest(&unsigned_bytes);
        let signature = crate::identity::sign(secret_key, DOMAIN, &unsigned_bytes)
            .map_err(|_| MembershipError::InvalidSignature)?;
        let bytes = encode_envelope(&unsigned_bytes, id, &signature);
        MembershipEnvelope::decode(&bytes)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, MembershipError> {
        if bytes.len() > MAX_ENVELOPE_BYTES {
            return Err(MembershipError::DecodeLimit);
        }
        if bytes.len() < 4 + 1 + 17 + 33 + 8 + 1 + 2 + 32 + 64 {
            return Err(MembershipError::MalformedAction);
        }
        let unsigned_len = u32::from_be_bytes(copy_array(&bytes[..4])) as usize;
        if unsigned_len > MAX_UNSIGNED_BYTES {
            return Err(MembershipError::DecodeLimit);
        }
        let expected_len = 4usize
            .checked_add(unsigned_len)
            .and_then(|length| length.checked_add(32 + 64))
            .ok_or(MembershipError::DecodeLimit)?;
        if bytes.len() != expected_len {
            return Err(MembershipError::MalformedAction);
        }
        let unsigned_bytes = bytes[4..4 + unsigned_len].to_vec();
        let encoded_id = MembershipOperationId::from_hash(Hash::from_bytes(copy_array(
            &bytes[4 + unsigned_len..4 + unsigned_len + 32],
        )));
        let signature = Signature::from_bytes(&copy_array(&bytes[4 + unsigned_len + 32..]));
        let (workspace_id, author, authored_at_unix_ms, action, dependencies) =
            decode_unsigned(&unsigned_bytes)?;
        let id = MembershipOperationId::digest(&unsigned_bytes);
        if encoded_id != id {
            return Err(MembershipError::ContentHashMismatch);
        }
        crate::identity::verify(author.as_public_key(), DOMAIN, &unsigned_bytes, &signature)
            .map_err(|_| MembershipError::InvalidSignature)?;
        match &action {
            MembershipAction::Admit { target, proof } => {
                if proof.claim.invitee != *target {
                    return Err(MembershipError::AdmissionTargetMismatch);
                }
                if proof.claim.token.workspace_id != workspace_id
                    || proof.claim.create_person.workspace_id != workspace_id
                {
                    return Err(MembershipError::MalformedAction);
                }
            }
            MembershipAction::AddAgentKey {
                group,
                agent,
                proof,
            } => {
                // The proof binds the Agent key to this Workspace, this group
                // and this Steward before any authority is considered.
                proof.verify(*agent, workspace_id, *group, author)?;
            }
            _ => {}
        }
        Ok(Self {
            workspace_id,
            author,
            authored_at_unix_ms,
            action,
            dependencies,
            id,
            signature,
            unsigned_bytes,
            bytes: bytes.to_vec(),
        })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn canonical_unsigned_bytes(&self) -> &[u8] {
        &self.unsigned_bytes
    }

    pub(crate) fn canonical_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

fn encode_unsigned(
    workspace_id: WorkspaceId,
    author: PublicIdentity,
    authored_at_unix_ms: i64,
    action: &MembershipAction,
    dependencies: &[MembershipOperationId],
) -> Result<Vec<u8>, MembershipError> {
    let mut bytes = Vec::new();
    bytes.push(VERSION);
    bytes.push(WORKSPACE_TAG);
    bytes.extend_from_slice(workspace_id.as_bytes());
    bytes.push(INSTALLATION_TAG);
    bytes.extend_from_slice(author.as_bytes());
    bytes.extend_from_slice(&authored_at_unix_ms.to_be_bytes());
    match action {
        MembershipAction::Create => bytes.push(CREATE_TAG),
        MembershipAction::Admit { target, proof } => {
            if proof.claim.bytes.len() > MAX_ADMISSION_PROOF_BYTES {
                return Err(MembershipError::DecodeLimit);
            }
            bytes.push(ADMIT_TAG);
            bytes.push(INSTALLATION_TAG);
            bytes.extend_from_slice(target.as_bytes());
            bytes.push(MEMBER_ACCESS);
            bytes.extend_from_slice(&(proof.claim.bytes.len() as u16).to_be_bytes());
            bytes.extend_from_slice(&proof.claim.bytes);
        }
        MembershipAction::Revoke { target, group } => {
            bytes.push(REVOKE_TAG);
            bytes.push(INSTALLATION_TAG);
            bytes.extend_from_slice(target.as_bytes());
            bytes.push(PERSON_GROUP_TAG);
            bytes.extend_from_slice(group.as_bytes());
        }
        MembershipAction::CreatePerson => bytes.push(CREATE_PERSON_TAG),
        MembershipAction::AttachPerson { group } => {
            bytes.push(ATTACH_PERSON_TAG);
            bytes.push(PERSON_GROUP_TAG);
            bytes.extend_from_slice(group.as_bytes());
            bytes.push(MEMBER_ACCESS);
        }
        MembershipAction::AddAgentKey {
            group,
            agent,
            proof,
        } => {
            bytes.push(ADD_AGENT_KEY_TAG);
            bytes.push(PERSON_GROUP_TAG);
            bytes.extend_from_slice(group.as_bytes());
            bytes.push(INSTALLATION_TAG);
            bytes.extend_from_slice(agent.as_bytes());
            bytes.push(MEMBER_ACCESS);
            bytes.extend_from_slice(&proof.to_bytes());
        }
        MembershipAction::RemoveAgentKey { group, agent } => {
            bytes.push(REMOVE_AGENT_KEY_TAG);
            bytes.push(PERSON_GROUP_TAG);
            bytes.extend_from_slice(group.as_bytes());
            bytes.push(INSTALLATION_TAG);
            bytes.extend_from_slice(agent.as_bytes());
        }
    }
    bytes.extend_from_slice(&(dependencies.len() as u16).to_be_bytes());
    for dependency in dependencies {
        bytes.extend_from_slice(dependency.as_bytes());
    }
    if bytes.len() > MAX_UNSIGNED_BYTES {
        return Err(MembershipError::DecodeLimit);
    }
    Ok(bytes)
}

fn decode_unsigned(
    bytes: &[u8],
) -> Result<
    (
        WorkspaceId,
        PublicIdentity,
        i64,
        MembershipAction,
        Vec<MembershipOperationId>,
    ),
    MembershipError,
> {
    if bytes.len() > MAX_UNSIGNED_BYTES {
        return Err(MembershipError::DecodeLimit);
    }
    let mut reader = Reader::new(bytes);
    if reader.byte()? != VERSION {
        return Err(MembershipError::UnsupportedVersion);
    }
    if reader.byte()? != WORKSPACE_TAG {
        return Err(MembershipError::InvalidRole);
    }
    let workspace_id = WorkspaceId::from_bytes(reader.array()?);
    if reader.byte()? != INSTALLATION_TAG {
        return Err(MembershipError::InvalidRole);
    }
    let author = PublicIdentity::from_bytes(reader.array()?)
        .map_err(|_| MembershipError::MalformedAction)?;
    let authored_at_unix_ms = i64::from_be_bytes(reader.array()?);
    let action = match reader.byte()? {
        CREATE_TAG => MembershipAction::Create,
        ADMIT_TAG => {
            let target = reader.installation()?;
            if reader.byte()? != MEMBER_ACCESS {
                return Err(MembershipError::MalformedAction);
            }
            let proof_len = u16::from_be_bytes(reader.array()?) as usize;
            if proof_len > MAX_ADMISSION_PROOF_BYTES {
                return Err(MembershipError::DecodeLimit);
            }
            let claim = decode_claim(reader.take(proof_len)?)
                .map_err(|_| MembershipError::MalformedAction)?;
            MembershipAction::Admit {
                target,
                proof: Box::new(AdmissionProof { claim }),
            }
        }
        REVOKE_TAG => {
            let target = reader.installation()?;
            let group = reader.person_group()?;
            MembershipAction::Revoke { target, group }
        }
        CREATE_PERSON_TAG => MembershipAction::CreatePerson,
        ATTACH_PERSON_TAG => {
            let group = reader.person_group()?;
            if reader.byte()? != MEMBER_ACCESS {
                return Err(MembershipError::MalformedAction);
            }
            MembershipAction::AttachPerson { group }
        }
        ADD_AGENT_KEY_TAG => {
            let group = reader.person_group()?;
            let agent = reader.installation()?;
            if reader.byte()? != MEMBER_ACCESS {
                return Err(MembershipError::MalformedAction);
            }
            let proof = AgentKeyProof::from_bytes(reader.array()?);
            MembershipAction::AddAgentKey {
                group,
                agent,
                proof,
            }
        }
        REMOVE_AGENT_KEY_TAG => {
            let group = reader.person_group()?;
            let agent = reader.installation()?;
            MembershipAction::RemoveAgentKey { group, agent }
        }
        _ => return Err(MembershipError::MalformedAction),
    };
    let dependency_count = u16::from_be_bytes(reader.array()?) as usize;
    if dependency_count > MAX_DEPENDENCIES {
        return Err(MembershipError::DecodeLimit);
    }
    let mut dependencies = Vec::with_capacity(dependency_count);
    for _ in 0..dependency_count {
        dependencies.push(MembershipOperationId::from_hash(Hash::from_bytes(
            reader.array()?,
        )));
    }
    if !dependencies.windows(2).all(|pair| pair[0] < pair[1]) {
        return Err(MembershipError::DependenciesNotCanonical);
    }
    if !reader.is_empty() {
        return Err(MembershipError::MalformedAction);
    }
    Ok((
        workspace_id,
        author,
        authored_at_unix_ms,
        action,
        dependencies,
    ))
}

fn encode_envelope(
    unsigned_bytes: &[u8],
    id: MembershipOperationId,
    signature: &Signature,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(4 + unsigned_bytes.len() + 32 + 64);
    bytes.extend_from_slice(&(unsigned_bytes.len() as u32).to_be_bytes());
    bytes.extend_from_slice(unsigned_bytes);
    bytes.extend_from_slice(id.as_bytes());
    bytes.extend_from_slice(&signature.to_bytes());
    bytes
}

/// The shared byte reader, with this codec's one error value.
struct Reader<'a>(ByteReader<'a>);

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self(ByteReader::new(bytes))
    }

    fn byte(&mut self) -> Result<u8, MembershipError> {
        self.0.take_u8().ok_or(MembershipError::MalformedAction)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], MembershipError> {
        self.0
            .take_array::<N>()
            .ok_or(MembershipError::MalformedAction)
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], MembershipError> {
        self.0.take(length).ok_or(MembershipError::MalformedAction)
    }

    /// A tagged Installation key.
    fn installation(&mut self) -> Result<PublicIdentity, MembershipError> {
        if self.byte()? != INSTALLATION_TAG {
            return Err(MembershipError::InvalidRole);
        }
        PublicIdentity::from_bytes(self.array()?).map_err(|_| MembershipError::MalformedAction)
    }

    /// A tagged Person group identifier.
    fn person_group(&mut self) -> Result<MembershipOperationId, MembershipError> {
        if self.byte()? != PERSON_GROUP_TAG {
            return Err(MembershipError::InvalidRole);
        }
        Ok(MembershipOperationId::from_hash(Hash::from_bytes(
            self.array()?,
        )))
    }

    const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn copy_array<const N: usize>(bytes: &[u8]) -> [u8; N] {
    bytes.try_into().expect("validated canonical slice width")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::invitation::{sign_claim, sign_token};

    fn secret(byte: u8) -> SecretKey {
        SecretKey::from_bytes(&[byte; 32])
    }

    fn identity(byte: u8) -> PublicIdentity {
        PublicIdentity::from_bytes(*secret(byte).public().as_bytes()).unwrap()
    }

    use crate::encoding::hex_encode as hex;

    #[test]
    fn canonical_create_is_stable_and_dependencies_are_sorted_unique() {
        let dependency_a = MembershipOperationId::digest(b"a");
        let dependency_b = MembershipOperationId::digest(b"b");
        let envelope = MembershipEnvelope::sign(
            &secret(4),
            WorkspaceId::from_bytes([5; 16]),
            42,
            MembershipAction::Create,
            vec![dependency_b, dependency_a, dependency_b],
        )
        .unwrap();

        let mut expected_dependencies = vec![dependency_a, dependency_b];
        expected_dependencies.sort_unstable();
        assert_eq!(envelope.dependencies, expected_dependencies);
        assert_eq!(
            MembershipEnvelope::decode(envelope.canonical_bytes())
                .unwrap()
                .id,
            envelope.id
        );
        assert_eq!(
            hex(envelope.canonical_unsigned_bytes()),
            "01020505050505050505050505050505050501ca93ac1705187071d67b83c7ff0efe8108e8ec4530575d7726879333dbdabe7c000000000000002a00000210e5cf3d3c8a4f9f3468c8cc58eea84892a22fdadbc1acb22410190044c1d55317762fddd969a453925d65717ac3eea21320b66b54342fde15128d6caf21215f"
        );
        assert_eq!(
            envelope.id.to_string(),
            "3aa8a2aad82029a0ed923a103cfe2512a694d3fb2596bfdd0b2653dca120bc4e"
        );
        assert_eq!(
            hex(envelope.canonical_bytes()),
            "0000007e01020505050505050505050505050505050501ca93ac1705187071d67b83c7ff0efe8108e8ec4530575d7726879333dbdabe7c000000000000002a00000210e5cf3d3c8a4f9f3468c8cc58eea84892a22fdadbc1acb22410190044c1d55317762fddd969a453925d65717ac3eea21320b66b54342fde15128d6caf21215f3aa8a2aad82029a0ed923a103cfe2512a694d3fb2596bfdd0b2653dca120bc4eb427746d74a3f2543710173899733a358082ba89c2366a4ccb6d52382782a0e4e521c20399c77505f95b7b5df70614f4ea1e3c6a7b87ef3a357c5398e79f0b00"
        );
    }

    #[test]
    fn create_person_with_one_dependency_has_the_declared_fixed_width() {
        let envelope = MembershipEnvelope::sign(
            &secret(4),
            WorkspaceId::from_bytes([5; 16]),
            42,
            MembershipAction::CreatePerson,
            vec![MembershipOperationId::digest(b"genesis")],
        )
        .unwrap();
        assert_eq!(
            envelope.canonical_bytes().len(),
            CREATE_PERSON_ENVELOPE_BYTES
        );
    }

    #[test]
    fn malformed_hash_signature_role_action_dependencies_and_bounds_are_rejected() {
        let dependency_a = MembershipOperationId::digest(b"a");
        let dependency_b = MembershipOperationId::digest(b"b");
        let envelope = MembershipEnvelope::sign(
            &secret(4),
            WorkspaceId::from_bytes([5; 16]),
            42,
            MembershipAction::Create,
            vec![dependency_a, dependency_b],
        )
        .unwrap();

        let mut bytes = envelope.bytes.clone();
        bytes[4] = 2;
        assert!(matches!(
            MembershipEnvelope::decode(&bytes),
            Err(MembershipError::UnsupportedVersion)
        ));
        let mut bytes = envelope.bytes.clone();
        bytes[5] = INSTALLATION_TAG;
        assert!(matches!(
            MembershipEnvelope::decode(&bytes),
            Err(MembershipError::InvalidRole)
        ));
        let mut bytes = envelope.bytes.clone();
        bytes[63] = 99;
        assert!(matches!(
            MembershipEnvelope::decode(&bytes),
            Err(MembershipError::MalformedAction)
        ));
        let mut bytes = envelope.bytes.clone();
        let first_dependency = 4 + 62;
        let (left, right) = bytes[first_dependency..first_dependency + 64].split_at_mut(32);
        left.swap_with_slice(right);
        assert!(matches!(
            MembershipEnvelope::decode(&bytes),
            Err(MembershipError::DependenciesNotCanonical)
        ));
        let mut bytes = envelope.bytes.clone();
        let first_dependency = 4 + 62;
        let first = bytes[first_dependency..first_dependency + 32].to_vec();
        bytes[first_dependency + 32..first_dependency + 64].copy_from_slice(&first);
        assert!(matches!(
            MembershipEnvelope::decode(&bytes),
            Err(MembershipError::DependenciesNotCanonical)
        ));
        let mut bytes = envelope.bytes.clone();
        let id_offset = 4 + envelope.unsigned_bytes.len();
        bytes[id_offset] ^= 1;
        assert!(matches!(
            MembershipEnvelope::decode(&bytes),
            Err(MembershipError::ContentHashMismatch)
        ));
        let mut bytes = envelope.bytes.clone();
        *bytes.last_mut().unwrap() ^= 1;
        assert!(matches!(
            MembershipEnvelope::decode(&bytes),
            Err(MembershipError::InvalidSignature)
        ));
        assert!(matches!(
            MembershipEnvelope::decode(&vec![0; MAX_ENVELOPE_BYTES + 1]),
            Err(MembershipError::DecodeLimit)
        ));
    }

    #[test]
    fn admit_requires_a_complete_authenticated_target_bound_proof() {
        let workspace = WorkspaceId::from_bytes([3; 16]);
        let genesis = MembershipOperationId::digest(b"genesis");
        let token = sign_token(&secret(7), workspace, 1_000, 2_000, [9; 32]).unwrap();
        let claim = sign_claim(&secret(8), &token, genesis).unwrap();
        let target = claim.invitee;
        let proof = Box::new(AdmissionProof { claim });
        let envelope = MembershipEnvelope::sign(
            &secret(7),
            workspace,
            1_100,
            MembershipAction::Admit { target, proof },
            vec![genesis],
        )
        .unwrap();
        assert!(matches!(
            MembershipEnvelope::decode(envelope.canonical_bytes()).unwrap().action,
            MembershipAction::Admit { target: decoded, .. } if decoded == target
        ));
        let other = identity(9);
        let proof = Box::new(AdmissionProof {
            claim: sign_claim(&secret(8), &token, genesis).unwrap(),
        });
        assert!(matches!(
            MembershipEnvelope::sign(
                &secret(7),
                workspace,
                1_100,
                MembershipAction::Admit {
                    target: other,
                    proof
                },
                vec![],
            ),
            Err(MembershipError::AdmissionTargetMismatch)
        ));
    }

    #[test]
    fn add_agent_key_verifies_the_agent_proof_against_workspace_group_and_steward() {
        let workspace = WorkspaceId::from_bytes([3; 16]);
        let group = MembershipOperationId::digest(b"group");
        let steward = secret(7);
        let agent = secret(21);
        let proof = AgentKeyProof::sign(&agent, workspace, group, identity(7)).unwrap();
        let envelope = MembershipEnvelope::sign(
            &steward,
            workspace,
            1_100,
            MembershipAction::AddAgentKey {
                group,
                agent: identity(21),
                proof: proof.clone(),
            },
            vec![group],
        )
        .unwrap();
        let decoded = MembershipEnvelope::decode(envelope.canonical_bytes()).unwrap();
        assert!(decoded.action.same_as(&envelope.action));
        assert_eq!(decoded.action.subject(), Some(identity(21)));

        // The same proof under another steward, workspace, group or agent key fails.
        let other_steward = secret(8);
        assert!(matches!(
            MembershipEnvelope::sign(
                &other_steward,
                workspace,
                1_100,
                MembershipAction::AddAgentKey {
                    group,
                    agent: identity(21),
                    proof: proof.clone(),
                },
                vec![group],
            ),
            Err(MembershipError::InvalidAdmissionProof)
        ));
        assert!(matches!(
            MembershipEnvelope::sign(
                &steward,
                WorkspaceId::from_bytes([4; 16]),
                1_100,
                MembershipAction::AddAgentKey {
                    group,
                    agent: identity(21),
                    proof: proof.clone(),
                },
                vec![group],
            ),
            Err(MembershipError::InvalidAdmissionProof)
        ));
        assert!(matches!(
            MembershipEnvelope::sign(
                &steward,
                workspace,
                1_100,
                MembershipAction::AddAgentKey {
                    group: MembershipOperationId::digest(b"other group"),
                    agent: identity(21),
                    proof: proof.clone(),
                },
                vec![group],
            ),
            Err(MembershipError::InvalidAdmissionProof)
        ));
        assert!(matches!(
            MembershipEnvelope::sign(
                &steward,
                workspace,
                1_100,
                MembershipAction::AddAgentKey {
                    group,
                    agent: identity(22),
                    proof,
                },
                vec![group],
            ),
            Err(MembershipError::InvalidAdmissionProof)
        ));
    }

    #[test]
    fn bounded_generated_envelopes_round_trip_and_reject_malformed_bytes() {
        for seed in 1_u8..=24 {
            let dependencies: Vec<_> = (0..usize::from(seed % 5))
                .map(|index| MembershipOperationId::digest([seed, index as u8]))
                .collect();
            let workspace_id = WorkspaceId::from_bytes([seed; 16]);
            let authored_at_unix_ms = i64::from(seed) * 1_000;
            let group = MembershipOperationId::digest([seed, 99]);
            let action = match seed % 7 {
                0 => MembershipAction::Create,
                1 => {
                    let token = sign_token(
                        &secret(seed),
                        workspace_id,
                        authored_at_unix_ms - 100,
                        authored_at_unix_ms + 100,
                        [seed; 32],
                    )
                    .unwrap();
                    let claim = sign_claim(
                        &secret(seed + 32),
                        &token,
                        MembershipOperationId::digest([seed, 98]),
                    )
                    .unwrap();
                    MembershipAction::Admit {
                        target: claim.invitee,
                        proof: Box::new(AdmissionProof { claim }),
                    }
                }
                2 => MembershipAction::Revoke {
                    target: identity(seed + 32),
                    group,
                },
                3 => MembershipAction::CreatePerson,
                4 => MembershipAction::AttachPerson { group },
                5 => MembershipAction::AddAgentKey {
                    group,
                    agent: identity(seed + 64),
                    proof: AgentKeyProof::sign(
                        &secret(seed + 64),
                        workspace_id,
                        group,
                        identity(seed),
                    )
                    .unwrap(),
                },
                _ => MembershipAction::RemoveAgentKey {
                    group,
                    agent: identity(seed + 64),
                },
            };
            let envelope = MembershipEnvelope::sign(
                &secret(seed),
                workspace_id,
                authored_at_unix_ms,
                action,
                dependencies,
            )
            .unwrap();
            let decoded = MembershipEnvelope::decode(envelope.canonical_bytes()).unwrap();
            assert_eq!(decoded.canonical_bytes(), envelope.canonical_bytes());
            assert_eq!(
                decoded.canonical_unsigned_bytes(),
                envelope.canonical_unsigned_bytes()
            );
            assert_eq!(decoded.id, envelope.id);
            assert!(decoded.action.same_as(&envelope.action));

            let canonical = envelope.canonical_bytes();
            for offset in (0..canonical.len()).step_by((canonical.len() / 13).max(1)) {
                let mut malformed = canonical.to_vec();
                malformed[offset] ^= 0x80;
                assert!(MembershipEnvelope::decode(&malformed).is_err());
            }
            for length in [0, 1, 4, canonical.len() / 2, canonical.len() - 1] {
                assert!(MembershipEnvelope::decode(&canonical[..length]).is_err());
            }
            let mut oversized = canonical.to_vec();
            oversized.push(seed);
            assert!(MembershipEnvelope::decode(&oversized).is_err());
        }
    }
}
