use std::str::FromStr;

use iroh_base::{SecretKey, Signature};
use p2panda_core::Hash;

use crate::error::{InvitationError, ValueParseError};
use crate::membership::{INSTALLATION_TAG, WORKSPACE_TAG};
use crate::{PublicIdentity, WorkspaceId};

const VERSION: u8 = 1;
const MEMBER_ACCESS: u8 = 1;
const TOKEN_DOMAIN: &str = "teamarness.invitation/v1";
const CLAIM_DOMAIN: &str = "teamarness.invitation-claim/v1";
const TOKEN_UNSIGNED_LEN: usize = 100;
const TOKEN_LEN: usize = TOKEN_UNSIGNED_LEN + 32 + 64;
const CLAIM_LEN: usize = 1 + 2 + TOKEN_LEN + 1 + 32 + 64;

/// An opaque, transferable signed invitation token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InvitationToken(String);

/// An opaque invitation token plus proof of possession by the invitee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InvitationClaim(String);

#[derive(Clone)]
pub(crate) struct VerifiedInvitationToken {
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) inviter: PublicIdentity,
    pub(crate) issued_at_unix_ms: i64,
    pub(crate) expires_at_unix_ms: i64,
    pub(crate) nonce: [u8; 32],
    pub(crate) id: Hash,
    pub(crate) bytes: Vec<u8>,
}

#[derive(Clone)]
pub(crate) struct VerifiedInvitationClaim {
    pub(crate) token: VerifiedInvitationToken,
    pub(crate) invitee: PublicIdentity,
    pub(crate) bytes: Vec<u8>,
}

impl InvitationToken {
    /// Returns the token's transport-safe lowercase hexadecimal form.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn encoded(&self) -> &str {
        &self.0
    }

    pub(crate) fn verified(&self) -> Result<VerifiedInvitationToken, InvitationError> {
        decode_token(&hex_decode(&self.0).ok_or(InvitationError::Malformed)?)
    }

    pub(crate) fn from_verified(token: &VerifiedInvitationToken) -> Self {
        Self(hex_encode(&token.bytes))
    }
}

impl FromStr for InvitationToken {
    type Err = ValueParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let bytes = hex_decode(value).ok_or_else(|| ValueParseError::new("invitation token"))?;
        decode_token(&bytes).map_err(|_| ValueParseError::new("invitation token"))?;
        Ok(Self(value.to_owned()))
    }
}

impl InvitationClaim {
    /// Returns the claim's transport-safe lowercase hexadecimal form.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn encoded(&self) -> &str {
        &self.0
    }

    pub(crate) fn verified(&self) -> Result<VerifiedInvitationClaim, InvitationError> {
        decode_claim(&hex_decode(&self.0).ok_or(InvitationError::Malformed)?)
    }

    pub(crate) fn from_verified(claim: &VerifiedInvitationClaim) -> Self {
        Self(hex_encode(&claim.bytes))
    }
}

impl FromStr for InvitationClaim {
    type Err = ValueParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let bytes = hex_decode(value).ok_or_else(|| ValueParseError::new("invitation claim"))?;
        decode_claim(&bytes).map_err(|_| ValueParseError::new("invitation claim"))?;
        Ok(Self(value.to_owned()))
    }
}

pub(crate) fn sign_token(
    secret_key: &SecretKey,
    workspace_id: WorkspaceId,
    issued_at_unix_ms: i64,
    expires_at_unix_ms: i64,
    nonce: [u8; 32],
) -> Result<VerifiedInvitationToken, InvitationError> {
    let inviter = PublicIdentity::from_bytes(*secret_key.public().as_bytes())
        .map_err(|_| InvitationError::InvalidRole)?;
    let unsigned = token_unsigned_bytes(
        workspace_id,
        inviter,
        issued_at_unix_ms,
        expires_at_unix_ms,
        nonce,
    );
    let id = Hash::digest(&unsigned);
    let signature = crate::identity::sign(secret_key, TOKEN_DOMAIN, &unsigned)
        .map_err(|_| InvitationError::InvalidTokenSignature)?;
    let mut bytes = unsigned;
    bytes.extend_from_slice(id.as_bytes());
    bytes.extend_from_slice(&signature.to_bytes());
    decode_token(&bytes)
}

pub(crate) fn sign_claim(
    secret_key: &SecretKey,
    token: &VerifiedInvitationToken,
) -> Result<VerifiedInvitationClaim, InvitationError> {
    let invitee = PublicIdentity::from_bytes(*secret_key.public().as_bytes())
        .map_err(|_| InvitationError::InvalidRole)?;
    let payload = claim_signature_payload(token.id, invitee);
    let signature = crate::identity::sign(secret_key, CLAIM_DOMAIN, &payload)
        .map_err(|_| InvitationError::InvalidClaimSignature)?;
    let mut bytes = Vec::with_capacity(CLAIM_LEN);
    bytes.push(VERSION);
    bytes.extend_from_slice(&(TOKEN_LEN as u16).to_be_bytes());
    bytes.extend_from_slice(&token.bytes);
    bytes.push(INSTALLATION_TAG);
    bytes.extend_from_slice(invitee.as_bytes());
    bytes.extend_from_slice(&signature.to_bytes());
    decode_claim(&bytes)
}

fn token_unsigned_bytes(
    workspace_id: WorkspaceId,
    inviter: PublicIdentity,
    issued_at_unix_ms: i64,
    expires_at_unix_ms: i64,
    nonce: [u8; 32],
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(TOKEN_UNSIGNED_LEN);
    bytes.push(VERSION);
    bytes.push(WORKSPACE_TAG);
    bytes.extend_from_slice(workspace_id.as_bytes());
    bytes.push(INSTALLATION_TAG);
    bytes.extend_from_slice(inviter.as_bytes());
    bytes.extend_from_slice(&issued_at_unix_ms.to_be_bytes());
    bytes.extend_from_slice(&expires_at_unix_ms.to_be_bytes());
    bytes.extend_from_slice(&nonce);
    bytes.push(MEMBER_ACCESS);
    bytes
}

fn decode_token(bytes: &[u8]) -> Result<VerifiedInvitationToken, InvitationError> {
    if bytes.len() > TOKEN_LEN {
        return Err(InvitationError::DecodeLimit);
    }
    if bytes.len() != TOKEN_LEN {
        return Err(InvitationError::Malformed);
    }
    if bytes[0] != VERSION {
        return Err(InvitationError::UnsupportedVersion);
    }
    if bytes[1] != WORKSPACE_TAG || bytes[18] != INSTALLATION_TAG {
        return Err(InvitationError::InvalidRole);
    }
    if bytes[99] != MEMBER_ACCESS {
        return Err(InvitationError::Malformed);
    }

    let workspace_id = WorkspaceId::from_bytes(copy_array(&bytes[2..18]));
    let inviter = PublicIdentity::from_bytes(copy_array(&bytes[19..51]))
        .map_err(|_| InvitationError::Malformed)?;
    let issued_at_unix_ms = i64::from_be_bytes(copy_array(&bytes[51..59]));
    let expires_at_unix_ms = i64::from_be_bytes(copy_array(&bytes[59..67]));
    if expires_at_unix_ms <= issued_at_unix_ms {
        return Err(InvitationError::Malformed);
    }
    let nonce = copy_array(&bytes[67..99]);
    let unsigned = &bytes[..TOKEN_UNSIGNED_LEN];
    let id = Hash::from_bytes(copy_array(
        &bytes[TOKEN_UNSIGNED_LEN..TOKEN_UNSIGNED_LEN + 32],
    ));
    if Hash::digest(unsigned) != id {
        return Err(InvitationError::ContentHashMismatch);
    }
    let signature = Signature::from_bytes(&copy_array(&bytes[TOKEN_UNSIGNED_LEN + 32..]));
    crate::identity::verify(inviter.as_public_key(), TOKEN_DOMAIN, unsigned, &signature)
        .map_err(|_| InvitationError::InvalidTokenSignature)?;

    Ok(VerifiedInvitationToken {
        workspace_id,
        inviter,
        issued_at_unix_ms,
        expires_at_unix_ms,
        nonce,
        id,
        bytes: bytes.to_vec(),
    })
}

pub(crate) fn decode_claim(bytes: &[u8]) -> Result<VerifiedInvitationClaim, InvitationError> {
    if bytes.len() > CLAIM_LEN {
        return Err(InvitationError::DecodeLimit);
    }
    if bytes.len() != CLAIM_LEN {
        return Err(InvitationError::Malformed);
    }
    if bytes[0] != VERSION {
        return Err(InvitationError::UnsupportedVersion);
    }
    let token_length = u16::from_be_bytes(copy_array(&bytes[1..3])) as usize;
    if token_length != TOKEN_LEN {
        return Err(InvitationError::DecodeLimit);
    }
    let token_end = 3 + token_length;
    let token = decode_token(&bytes[3..token_end])?;
    if bytes[token_end] != INSTALLATION_TAG {
        return Err(InvitationError::InvalidRole);
    }
    let invitee_end = token_end + 1 + 32;
    let invitee = PublicIdentity::from_bytes(copy_array(&bytes[token_end + 1..invitee_end]))
        .map_err(|_| InvitationError::Malformed)?;
    let signature = Signature::from_bytes(&copy_array(&bytes[invitee_end..]));
    let payload = claim_signature_payload(token.id, invitee);
    crate::identity::verify(invitee.as_public_key(), CLAIM_DOMAIN, &payload, &signature)
        .map_err(|_| InvitationError::InvalidClaimSignature)?;

    Ok(VerifiedInvitationClaim {
        token,
        invitee,
        bytes: bytes.to_vec(),
    })
}

fn claim_signature_payload(token_id: Hash, invitee: PublicIdentity) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(65);
    bytes.extend_from_slice(token_id.as_bytes());
    bytes.push(INSTALLATION_TAG);
    bytes.extend_from_slice(invitee.as_bytes());
    bytes
}

use crate::encoding::{hex_decode, hex_encode};

fn copy_array<const N: usize>(bytes: &[u8]) -> [u8; N] {
    bytes.try_into().expect("validated canonical slice width")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret(byte: u8) -> SecretKey {
        SecretKey::from_bytes(&[byte; 32])
    }

    #[test]
    fn token_and_claim_round_trip_and_reject_tampering() {
        let token = sign_token(
            &secret(7),
            WorkspaceId::from_bytes([3; 16]),
            1_000,
            2_000,
            [9; 32],
        )
        .unwrap();
        let claim = sign_claim(&secret(8), &token).unwrap();

        let encoded_token = InvitationToken::from_verified(&token);
        let encoded_claim = InvitationClaim::from_verified(&claim);
        assert_eq!(encoded_token.verified().unwrap().id, token.id);
        assert_eq!(encoded_claim.verified().unwrap().invitee, claim.invitee);

        let mut tampered = token.bytes.clone();
        tampered[70] ^= 1;
        assert!(matches!(
            decode_token(&tampered),
            Err(InvitationError::ContentHashMismatch)
        ));
        let mut tampered = token.bytes.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(matches!(
            decode_token(&tampered),
            Err(InvitationError::InvalidTokenSignature)
        ));
        let mut tampered = claim.bytes.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(matches!(
            decode_claim(&tampered),
            Err(InvitationError::InvalidClaimSignature)
        ));
    }

    #[test]
    fn role_version_shape_and_bounds_are_checked() {
        let token = sign_token(
            &secret(7),
            WorkspaceId::from_bytes([3; 16]),
            1_000,
            2_000,
            [9; 32],
        )
        .unwrap();
        let mut bytes = token.bytes.clone();
        bytes[0] = 2;
        assert!(matches!(
            decode_token(&bytes),
            Err(InvitationError::UnsupportedVersion)
        ));
        let mut bytes = token.bytes.clone();
        bytes[1] = INSTALLATION_TAG;
        assert!(matches!(
            decode_token(&bytes),
            Err(InvitationError::InvalidRole)
        ));
        let mut bytes = token.bytes.clone();
        bytes[99] = 2;
        assert!(matches!(
            decode_token(&bytes),
            Err(InvitationError::Malformed)
        ));
        assert!(matches!(
            decode_token(&[0; TOKEN_LEN + 1]),
            Err(InvitationError::DecodeLimit)
        ));

        let claim = sign_claim(&secret(8), &token).unwrap();
        let mut bytes = claim.bytes.clone();
        bytes[3 + TOKEN_LEN] = WORKSPACE_TAG;
        assert!(matches!(
            decode_claim(&bytes),
            Err(InvitationError::InvalidRole)
        ));
        assert!(matches!(
            decode_claim(&[0; CLAIM_LEN + 1]),
            Err(InvitationError::DecodeLimit)
        ));
    }

    #[test]
    fn canonical_token_and_claim_golden_vectors_are_stable() {
        let token = sign_token(
            &secret(7),
            WorkspaceId::from_bytes([3; 16]),
            1_000,
            2_000,
            [9; 32],
        )
        .unwrap();
        let claim = sign_claim(&secret(8), &token).unwrap();
        assert_eq!(
            hex_encode(&token.bytes),
            "01020303030303030303030303030303030301ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c00000000000003e800000000000007d0090909090909090909090909090909090909090909090909090909090909090901390172f8e526626476553d760aff5765f0982aad65c66ce82004fca24383d4173a2a0c0b6820a6ed6d42a020e06b98bb0552457070a469b02ae1479f24913dbbf3f4417accb9e9276549657229fb434f0c06ffab49a80b059d9ed74913ee6d03"
        );
        assert_eq!(
            token.id.to_string(),
            "390172f8e526626476553d760aff5765f0982aad65c66ce82004fca24383d417"
        );
        assert_eq!(
            hex_encode(&claim.bytes),
            "0100c401020303030303030303030303030303030301ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c00000000000003e800000000000007d0090909090909090909090909090909090909090909090909090909090909090901390172f8e526626476553d760aff5765f0982aad65c66ce82004fca24383d4173a2a0c0b6820a6ed6d42a020e06b98bb0552457070a469b02ae1479f24913dbbf3f4417accb9e9276549657229fb434f0c06ffab49a80b059d9ed74913ee6d03011398f62c6d1a457c51ba6a4b5f3dbd2f69fca93216218dc8997e416bd17d93ca511c5aeb0f1f776e59dea0e74c30ee2d1eae9a0eb75c2167d6f7e37683103e2635e528ef81502b64ad857773766eef6de1282266792da0f519f26320f6461c06"
        );
    }

    #[test]
    fn bounded_generated_invitation_values_round_trip_and_reject_malformed_bytes() {
        for seed in 1_u8..=24 {
            let issued_at = i64::from(seed) * 1_000;
            let token = sign_token(
                &secret(seed),
                WorkspaceId::from_bytes([seed; 16]),
                issued_at,
                issued_at + i64::from(seed) + 1,
                [seed.wrapping_mul(17); 32],
            )
            .unwrap();
            let claim = sign_claim(&secret(seed + 32), &token).unwrap();

            let token_text = InvitationToken::from_verified(&token);
            let parsed_token = InvitationToken::from_str(token_text.encoded()).unwrap();
            assert_eq!(parsed_token, token_text);
            assert_eq!(parsed_token.verified().unwrap().bytes, token.bytes);

            let claim_text = InvitationClaim::from_verified(&claim);
            let parsed_claim = InvitationClaim::from_str(claim_text.encoded()).unwrap();
            assert_eq!(parsed_claim, claim_text);
            assert_eq!(parsed_claim.verified().unwrap().bytes, claim.bytes);

            let canonical = token.bytes.as_slice();
            for offset in (0..canonical.len()).step_by((canonical.len() / 13).max(1)) {
                let mut malformed = canonical.to_vec();
                malformed[offset] ^= 0x80;
                assert!(decode_token(&malformed).is_err());
            }
            for length in [0, 1, canonical.len() / 2, canonical.len() - 1] {
                assert!(decode_token(&canonical[..length]).is_err());
            }
            let mut oversized = canonical.to_vec();
            oversized.push(seed);
            assert!(decode_token(&oversized).is_err());

            for offset in (0..claim.bytes.len()).step_by((claim.bytes.len() / 13).max(1)) {
                let mut malformed = claim.bytes.clone();
                malformed[offset] ^= 0x80;
                assert!(decode_claim(&malformed).is_err());
            }
            for length in [0, 1, claim.bytes.len() / 2, claim.bytes.len() - 1] {
                assert!(decode_claim(&claim.bytes[..length]).is_err());
            }
            let mut oversized = claim.bytes.clone();
            oversized.push(seed);
            assert!(decode_claim(&oversized).is_err());
        }
    }
}
