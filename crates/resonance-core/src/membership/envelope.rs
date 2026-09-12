use iroh_base::{SecretKey, Signature};
use p2panda_core::Hash;

use crate::error::MembershipError;
use crate::invitation::{VerifiedInvitationClaim, decode_claim};
use crate::membership::{INSTALLATION_TAG, WORKSPACE_TAG};
use crate::{MembershipOperationId, PublicIdentity, WorkspaceId};

const VERSION: u8 = 1;
const CREATE_TAG: u8 = 0;
const ADMIT_TAG: u8 = 1;
const REVOKE_TAG: u8 = 2;
const MEMBER_ACCESS: u8 = 1;
const DOMAIN: &str = "resonance.membership-operation/v1";
const MAX_DEPENDENCIES: usize = 64;
const MAX_ADMISSION_PROOF_BYTES: usize = 1_024;
const MAX_UNSIGNED_BYTES: usize = 64 * 1024;
const MAX_ENVELOPE_BYTES: usize = 4 + MAX_UNSIGNED_BYTES + 32 + 64;

#[derive(Clone)]
pub(crate) struct AdmissionProof {
    pub(crate) claim: VerifiedInvitationClaim,
}

#[derive(Clone)]
pub(crate) enum MembershipAction {
    Create,
    Admit {
        target: PublicIdentity,
        proof: Box<AdmissionProof>,
    },
    Revoke {
        target: PublicIdentity,
    },
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
        if let MembershipAction::Admit { target, proof } = &action {
            if proof.claim.invitee != *target {
                return Err(MembershipError::AdmissionTargetMismatch);
            }
            if proof.claim.token.workspace_id != workspace_id {
                return Err(MembershipError::MalformedAction);
            }
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
        MembershipAction::Revoke { target } => {
            bytes.push(REVOKE_TAG);
            bytes.push(INSTALLATION_TAG);
            bytes.extend_from_slice(target.as_bytes());
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
            if reader.byte()? != INSTALLATION_TAG {
                return Err(MembershipError::InvalidRole);
            }
            let target = PublicIdentity::from_bytes(reader.array()?)
                .map_err(|_| MembershipError::MalformedAction)?;
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
            if reader.byte()? != INSTALLATION_TAG {
                return Err(MembershipError::InvalidRole);
            }
            let target = PublicIdentity::from_bytes(reader.array()?)
                .map_err(|_| MembershipError::MalformedAction)?;
            MembershipAction::Revoke { target }
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

struct Reader<'a> {
    remaining: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { remaining: bytes }
    }

    fn byte(&mut self) -> Result<u8, MembershipError> {
        Ok(self.take(1)?[0])
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], MembershipError> {
        Ok(copy_array(self.take(N)?))
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], MembershipError> {
        if length > self.remaining.len() {
            return Err(MembershipError::MalformedAction);
        }
        let (value, remaining) = self.remaining.split_at(length);
        self.remaining = remaining;
        Ok(value)
    }

    fn is_empty(&self) -> bool {
        self.remaining.is_empty()
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

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

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
            "0000007e01020505050505050505050505050505050501ca93ac1705187071d67b83c7ff0efe8108e8ec4530575d7726879333dbdabe7c000000000000002a00000210e5cf3d3c8a4f9f3468c8cc58eea84892a22fdadbc1acb22410190044c1d55317762fddd969a453925d65717ac3eea21320b66b54342fde15128d6caf21215f3aa8a2aad82029a0ed923a103cfe2512a694d3fb2596bfdd0b2653dca120bc4e432649c3355847f8ef16b1373fbfa0e03681f1fa6ab3569afa86491009a7518cf5a45231c051ad75ad3a547a69b073bec29b2256e4da560fb4774e152b138a06"
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
        let token = sign_token(
            &secret(7),
            WorkspaceId::from_bytes([3; 16]),
            1_000,
            2_000,
            [9; 32],
        )
        .unwrap();
        let claim = sign_claim(&secret(8), &token).unwrap();
        let target = claim.invitee;
        let proof = Box::new(AdmissionProof { claim });
        let envelope = MembershipEnvelope::sign(
            &secret(7),
            WorkspaceId::from_bytes([3; 16]),
            1_100,
            MembershipAction::Admit { target, proof },
            vec![MembershipOperationId::digest(b"genesis")],
        )
        .unwrap();
        assert!(matches!(
            MembershipEnvelope::decode(envelope.canonical_bytes()).unwrap().action,
            MembershipAction::Admit { target: decoded, .. } if decoded == target
        ));
        let other = PublicIdentity::from_bytes(*secret(9).public().as_bytes()).unwrap();
        let proof = Box::new(AdmissionProof {
            claim: sign_claim(&secret(8), &token).unwrap(),
        });
        assert!(matches!(
            MembershipEnvelope::sign(
                &secret(7),
                WorkspaceId::from_bytes([3; 16]),
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
    fn bounded_generated_envelopes_round_trip_and_reject_malformed_bytes() {
        for seed in 1_u8..=24 {
            let dependencies = (0..usize::from(seed % 5))
                .map(|index| MembershipOperationId::digest([seed, index as u8]))
                .collect();
            let workspace_id = WorkspaceId::from_bytes([seed; 16]);
            let authored_at_unix_ms = i64::from(seed) * 1_000;
            let action = match seed % 3 {
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
                    let claim = sign_claim(&secret(seed + 32), &token).unwrap();
                    MembershipAction::Admit {
                        target: claim.invitee,
                        proof: Box::new(AdmissionProof { claim }),
                    }
                }
                _ => {
                    let target =
                        PublicIdentity::from_bytes(*secret(seed + 32).public().as_bytes()).unwrap();
                    MembershipAction::Revoke { target }
                }
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
