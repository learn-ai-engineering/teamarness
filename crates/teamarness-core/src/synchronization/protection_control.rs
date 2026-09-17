use std::collections::BTreeMap;

use curve25519_dalek::edwards::CompressedEdwardsY;
use hpke_rs::{Hpke, HpkePrivateKey, HpkePublicKey, Mode};
use hpke_rs_crypto::types::{AeadAlgorithm, KdfAlgorithm, KemAlgorithm};
use hpke_rs_rust_crypto::HpkeRustCrypto;
use iroh_base::{SecretKey, Signature};
use p2panda_core::Hash;
use rusqlite::Transaction;
use sha2::{Digest, Sha512};
use zeroize::{Zeroize, Zeroizing};

use crate::error::StorageError;
use crate::{PublicIdentity, WorkspaceId};

/// Version-1 format magic. Retained only so stored or received version-1 bytes
/// are recognised and refused; no version-1 control is ever opened.
pub(crate) const DOMAIN_V1: &[u8] = b"teamarness.protection-control/v1";
/// Version-2 format magic.
pub(crate) const DOMAIN_V2: &[u8] = b"teamarness.protection-control/v2";

const VERSION: u8 = 2;
pub(crate) const KIND_CREATE: u8 = 1;
pub(crate) const KIND_GRANT: u8 = 3;

/// Domain tag for the control signature, passed through the domain-separated
/// signing interface (`TEAM.SYS.ID-R04`).
const SIGNING_CONTEXT: &str = "teamarness.protection-control/v2";
/// RFC 9180 `info` string for the content-key encapsulation.
const HPKE_INFO: &[u8] = b"teamarness.protection-grant/v2";

/// End of the header, which is the associated data of the encryption.
pub(crate) const AAD_END: usize = 210;
/// End of the signed prefix: the header plus the encapsulated key and ciphertext.
const SIGNED_END: usize = 290;
/// Every version-2 control is exactly this many bytes.
pub(crate) const CONTROL_LEN: usize = 354;

const ENC_LEN: usize = 32;
const CIPHERTEXT_LEN: usize = 48;

/// Opaque protection welcome/grant metadata. Raw content keys never occur in
/// this projection; the production protection adapter owns grant bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GrantSlot {
    pub(crate) recipient: PublicIdentity,
    pub(crate) admission: [u8; 32],
    pub(crate) content_key_id: [u8; 32],
    pub(crate) grant_bytes: Vec<u8>,
}

#[derive(Debug, Default)]
pub(crate) struct GrantProjection {
    slots: BTreeMap<(PublicIdentity, [u8; 32]), GrantSlot>,
    content_key_id: Option<[u8; 32]>,
}

impl GrantProjection {
    pub(crate) fn apply(&mut self, slot: GrantSlot) -> Result<bool, ()> {
        if let Some(existing) = self.content_key_id
            && existing != slot.content_key_id
        {
            return Err(());
        }
        if self.content_key_id.is_none() {
            self.content_key_id = Some(slot.content_key_id);
        }
        let key = (slot.recipient, slot.admission);
        if self.slots.contains_key(&key) {
            return Ok(false);
        }
        self.slots.insert(key, slot);
        Ok(true)
    }

    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }
}

/// A decoded, signature-verified protection control. Decoding never decrypts:
/// only [`open_control`], with the recipient's private key, recovers the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProtectionControl {
    pub(crate) kind: u8,
    pub(crate) scope: [u8; 32],
    pub(crate) workspace: WorkspaceId,
    pub(crate) author: PublicIdentity,
    pub(crate) recipient: PublicIdentity,
    pub(crate) admission: [u8; 32],
    pub(crate) content_key_id: [u8; 32],
    pub(crate) enc: [u8; ENC_LEN],
    pub(crate) ciphertext: [u8; CIPHERTEXT_LEN],
    pub(crate) signature: [u8; 64],
}

fn suite() -> Hpke<HpkeRustCrypto> {
    Hpke::<HpkeRustCrypto>::new(
        Mode::Base,
        KemAlgorithm::DhKem25519,
        KdfAlgorithm::HkdfSha256,
        AeadAlgorithm::ChaCha20Poly1305,
    )
}

/// Converts an Installation's Ed25519 identity to the X25519 public key that
/// seals controls for it. Invalid and small-order points are refused.
pub(crate) fn recipient_x25519(recipient: PublicIdentity) -> Result<[u8; 32], StorageError> {
    let point = CompressedEdwardsY(*recipient.as_bytes())
        .decompress()
        .ok_or(StorageError::CorruptWorkspace)?;
    if point.is_small_order() {
        return Err(StorageError::CorruptWorkspace);
    }
    Ok(point.to_montgomery().to_bytes())
}

/// Derives the matching X25519 secret from this Installation's Ed25519 seed.
/// X25519 clamps the scalar itself, so the first half of SHA-512 is the scalar
/// that `ed25519-dalek` would derive for the same key.
fn local_x25519(secret: &SecretKey) -> Zeroizing<[u8; 32]> {
    let seed = Zeroizing::new(secret.to_bytes());
    let mut hash = Sha512::digest(seed.as_slice());
    let mut scalar = Zeroizing::new([0u8; 32]);
    scalar.copy_from_slice(&hash.as_slice()[..32]);
    hash.as_mut_slice().zeroize();
    scalar
}

#[allow(clippy::too_many_arguments)]
fn header_bytes(
    kind: u8,
    scope: [u8; 32],
    workspace: WorkspaceId,
    author: PublicIdentity,
    recipient: PublicIdentity,
    admission: [u8; 32],
    content_key_id: [u8; 32],
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(AAD_END);
    bytes.extend_from_slice(DOMAIN_V2);
    bytes.extend_from_slice(&[VERSION, kind]);
    bytes.extend_from_slice(&scope);
    bytes.extend_from_slice(workspace.as_bytes());
    bytes.extend_from_slice(author.as_bytes());
    bytes.extend_from_slice(recipient.as_bytes());
    bytes.extend_from_slice(&admission);
    bytes.extend_from_slice(&content_key_id);
    debug_assert_eq!(bytes.len(), AAD_END);
    bytes
}

/// Seals `content_key` to `recipient` with HPKE, binding the whole header as
/// associated data, and signs the result through the domain-separated signing
/// interface.
pub(crate) fn seal_control(
    secret: &SecretKey,
    kind: u8,
    scope: [u8; 32],
    workspace: WorkspaceId,
    recipient: PublicIdentity,
    admission: [u8; 32],
    content_key: &[u8; 32],
) -> Result<Vec<u8>, StorageError> {
    let author = PublicIdentity::from_bytes(*secret.public().as_bytes())
        .map_err(|_| StorageError::CorruptWorkspace)?;
    let content_key_id = *Hash::digest(content_key).as_bytes();
    let header = header_bytes(
        kind,
        scope,
        workspace,
        author,
        recipient,
        admission,
        content_key_id,
    );
    let public = HpkePublicKey::new(recipient_x25519(recipient)?.to_vec());
    let (enc, ciphertext) = suite()
        .seal(
            &public,
            HPKE_INFO,
            &header,
            content_key.as_slice(),
            None,
            None,
            None,
        )
        .map_err(|_| StorageError::CorruptWorkspace)?;
    if enc.len() != ENC_LEN || ciphertext.len() != CIPHERTEXT_LEN {
        return Err(StorageError::CorruptWorkspace);
    }
    let mut bytes = header;
    bytes.extend_from_slice(&enc);
    bytes.extend_from_slice(&ciphertext);
    debug_assert_eq!(bytes.len(), SIGNED_END);
    let signature = crate::identity::sign(secret, SIGNING_CONTEXT, &bytes[..SIGNED_END])
        .map_err(|_| StorageError::CorruptWorkspace)?;
    bytes.extend_from_slice(&signature.to_bytes());
    debug_assert_eq!(bytes.len(), CONTROL_LEN);
    Ok(bytes)
}

/// The Workspace creator's origin for a scope's first content key.
pub(crate) fn create(
    secret: &SecretKey,
    scope: [u8; 32],
    workspace: WorkspaceId,
    recipient: PublicIdentity,
    admission: [u8; 32],
    content_key: [u8; 32],
) -> Result<Vec<u8>, StorageError> {
    seal_control(
        secret,
        KIND_CREATE,
        scope,
        workspace,
        recipient,
        admission,
        &content_key,
    )
}

/// A grant of an existing content key to one current Member.
pub(crate) fn grant(
    secret: &SecretKey,
    scope: [u8; 32],
    workspace: WorkspaceId,
    recipient: PublicIdentity,
    admission: [u8; 32],
    content_key: [u8; 32],
) -> Result<Vec<u8>, StorageError> {
    seal_control(
        secret,
        KIND_GRANT,
        scope,
        workspace,
        recipient,
        admission,
        &content_key,
    )
}

/// Parses and verifies a control without any secret. Version-1 bytes are
/// refused as [`StorageError::LegacyProtectionState`]: there is no read path
/// for them.
pub(crate) fn decode(bytes: &[u8]) -> Result<ProtectionControl, StorageError> {
    if bytes.starts_with(DOMAIN_V1) {
        return Err(StorageError::LegacyProtectionState);
    }
    if bytes.len() != CONTROL_LEN
        || !bytes.starts_with(DOMAIN_V2)
        || bytes[DOMAIN_V2.len()] != VERSION
    {
        return Err(StorageError::CorruptWorkspace);
    }
    let kind = bytes[DOMAIN_V2.len() + 1];
    if kind != KIND_CREATE && kind != KIND_GRANT {
        return Err(StorageError::CorruptWorkspace);
    }
    let mut at = DOMAIN_V2.len() + 2;
    let scope = take_array::<32>(bytes, &mut at)?;
    let workspace = WorkspaceId::from_bytes(take_array::<16>(bytes, &mut at)?);
    let author = take_identity(bytes, &mut at)?;
    let recipient = take_identity(bytes, &mut at)?;
    let admission = take_array::<32>(bytes, &mut at)?;
    let content_key_id = take_array::<32>(bytes, &mut at)?;
    debug_assert_eq!(at, AAD_END);
    let enc = take_array::<ENC_LEN>(bytes, &mut at)?;
    let ciphertext = take_array::<CIPHERTEXT_LEN>(bytes, &mut at)?;
    debug_assert_eq!(at, SIGNED_END);
    let signature = take_array::<64>(bytes, &mut at)?;
    if at != bytes.len() {
        return Err(StorageError::CorruptWorkspace);
    }
    if kind == KIND_CREATE && (recipient != author || admission != [0; 32]) {
        return Err(StorageError::CorruptWorkspace);
    }
    crate::identity::verify(
        author.as_public_key(),
        SIGNING_CONTEXT,
        &bytes[..SIGNED_END],
        &Signature::from_bytes(&signature),
    )
    .map_err(|_| StorageError::CorruptWorkspace)?;
    Ok(ProtectionControl {
        kind,
        scope,
        workspace,
        author,
        recipient,
        admission,
        content_key_id,
        enc,
        ciphertext,
        signature,
    })
}

/// Recovers the content key from a control addressed to this Installation.
///
/// `header` must be the control's own first [`AAD_END`] bytes, which are the
/// associated data the author bound at seal time; a re-signed control with a
/// different header therefore never opens.
pub(crate) fn open_control(
    control: &ProtectionControl,
    header: &[u8],
    local: &SecretKey,
) -> Result<Zeroizing<[u8; 32]>, StorageError> {
    let local_identity = PublicIdentity::from_bytes(*local.public().as_bytes())
        .map_err(|_| StorageError::CorruptWorkspace)?;
    if control.recipient != local_identity || header.len() != AAD_END {
        return Err(StorageError::CorruptWorkspace);
    }
    let private = HpkePrivateKey::new(local_x25519(local).to_vec());
    let plaintext = Zeroizing::new(
        suite()
            .open(
                &control.enc,
                &private,
                HPKE_INFO,
                header,
                &control.ciphertext,
                None,
                None,
                None,
            )
            .map_err(|_| StorageError::CorruptWorkspace)?,
    );
    let mut key = Zeroizing::new([0u8; 32]);
    if plaintext.len() != 32 {
        return Err(StorageError::CorruptWorkspace);
    }
    key.copy_from_slice(plaintext.as_slice());
    if *Hash::digest(key.as_slice()).as_bytes() != control.content_key_id {
        return Err(StorageError::CorruptWorkspace);
    }
    Ok(key)
}

fn take_array<const N: usize>(bytes: &[u8], at: &mut usize) -> Result<[u8; N], StorageError> {
    let end = at.checked_add(N).ok_or(StorageError::CorruptWorkspace)?;
    let value = bytes.get(*at..end).ok_or(StorageError::CorruptWorkspace)?;
    *at = end;
    value.try_into().map_err(|_| StorageError::CorruptWorkspace)
}

fn take_identity(bytes: &[u8], at: &mut usize) -> Result<PublicIdentity, StorageError> {
    PublicIdentity::from_bytes(take_array::<32>(bytes, at)?)
        .map_err(|_| StorageError::CorruptWorkspace)
}

pub(crate) fn insert(
    tx: &Transaction<'_>,
    scope: [u8; 32],
    control_id: [u8; 32],
    kind: &str,
    control: &ProtectionControl,
    bytes: &[u8],
) -> Result<(), StorageError> {
    tx.execute(
        "INSERT OR IGNORE INTO protection_control_operations
         (control_id,scope_id,operation_kind,author_public_identity,recipient_public_identity,
          admission_operation_id,content_key_id,control_bytes,signature)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        rusqlite::params![
            control_id.as_slice(),
            scope.as_slice(),
            kind,
            control.author.as_bytes().as_slice(),
            control.recipient.as_bytes().as_slice(),
            control.admission.as_slice(),
            control.content_key_id.as_slice(),
            bytes,
            control.signature.as_slice()
        ],
    )
    .map(|_| ())
    .map_err(|_| StorageError::Database)
}

#[cfg(test)]
mod tests {
    use super::*;
    use curve25519_dalek::montgomery::MontgomeryPoint;
    use iroh_base::SecretKey;

    fn secret(seed: u8) -> SecretKey {
        SecretKey::from_bytes(&[seed; 32])
    }

    fn identity(seed: u8) -> PublicIdentity {
        PublicIdentity::from_bytes(*secret(seed).public().as_bytes()).unwrap()
    }

    fn slot(key_id: [u8; 32], admission: u8) -> GrantSlot {
        GrantSlot {
            recipient: identity(9),
            admission: [admission; 32],
            content_key_id: key_id,
            grant_bytes: vec![1, 2, 3],
        }
    }

    fn sample_grant(author: &SecretKey, recipient: PublicIdentity, key: [u8; 32]) -> Vec<u8> {
        grant(
            author,
            [3; 32],
            WorkspaceId::from_bytes([4; 16]),
            recipient,
            [5; 32],
            key,
        )
        .unwrap()
    }

    // Verifies: TEAM.SYS.ID-R08
    #[test]
    fn x25519_derivation_matches_montgomery_conversion() {
        for seed in 1..=8u8 {
            let key = secret(seed);
            let derived = local_x25519(&key);
            assert_eq!(
                MontgomeryPoint::mul_base_clamped(*derived).to_bytes(),
                recipient_x25519(identity(seed)).unwrap()
            );
        }
    }

    // Verifies: TEAM.SYS.ID-R08
    #[test]
    fn grant_round_trip_opens_only_with_recipient_secret() {
        let author = secret(7);
        let recipient = secret(9);
        let bytes = sample_grant(&author, identity(9), [8; 32]);

        let control = decode(&bytes).unwrap();

        assert_eq!(control.kind, KIND_GRANT);
        assert_eq!(control.recipient, identity(9));
        assert_eq!(
            *open_control(&control, &bytes[..AAD_END], &recipient).unwrap(),
            [8; 32]
        );
        assert!(open_control(&control, &bytes[..AAD_END], &author).is_err());
        assert!(open_control(&control, &bytes[..AAD_END], &secret(11)).is_err());
    }

    // Verifies: TEAM.SYS.ID-R08
    #[test]
    fn grant_bytes_do_not_contain_the_content_key_or_a_public_wrap() {
        let key = [8; 32];
        let bytes = sample_grant(&secret(7), identity(9), key);

        assert!(!bytes.windows(32).any(|window| window == key));

        let mut wrap = Vec::with_capacity(DOMAIN_V1.len() + 64);
        wrap.extend_from_slice(DOMAIN_V1);
        wrap.extend_from_slice(&[3; 32]);
        wrap.extend_from_slice(identity(9).as_bytes());
        let wrapping_key = *Hash::digest(wrap).as_bytes();
        assert!(
            p2panda_encryption::crypto::xchacha20::x_aead_decrypt(
                &wrapping_key,
                &bytes[AAD_END + ENC_LEN..SIGNED_END],
                [0; 24],
                Some(&[3u8; 32]),
            )
            .is_err()
        );
    }

    // Verifies: TEAM.SYS.ID-R08
    #[test]
    fn header_tamper_breaks_signature() {
        let bytes = sample_grant(&secret(7), identity(9), [8; 32]);

        for offset in 0..SIGNED_END {
            let mut tampered = bytes.clone();
            tampered[offset] ^= 1;
            assert!(
                decode(&tampered).is_err(),
                "flipping byte {offset} was accepted"
            );
        }
    }

    // Verifies: TEAM.SYS.ID-R08
    #[test]
    fn resigned_control_with_other_author_does_not_open() {
        let recipient = secret(9);
        let bytes = sample_grant(&secret(7), identity(9), [8; 32]);
        let attacker = secret(12);

        let mut forged = bytes.clone();
        forged[82..114].copy_from_slice(identity(12).as_bytes());
        let signature =
            crate::identity::sign(&attacker, SIGNING_CONTEXT, &forged[..SIGNED_END]).unwrap();
        forged[SIGNED_END..].copy_from_slice(&signature.to_bytes());

        let control = decode(&forged).unwrap();
        assert_eq!(control.author, identity(12));
        assert!(open_control(&control, &forged[..AAD_END], &recipient).is_err());
    }

    #[test]
    fn grant_to_small_order_recipient_is_rejected() {
        // The Edwards identity point is the canonical small-order element.
        let identity_point = PublicIdentity::from_bytes([
            1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0,
        ])
        .unwrap();

        assert_eq!(
            recipient_x25519(identity_point),
            Err(StorageError::CorruptWorkspace)
        );
        assert_eq!(
            grant(
                &secret(7),
                [3; 32],
                WorkspaceId::from_bytes([4; 16]),
                identity_point,
                [5; 32],
                [8; 32],
            ),
            Err(StorageError::CorruptWorkspace)
        );
    }

    #[test]
    fn create_control_requires_self_recipient_and_zero_admission() {
        let author = secret(7);
        let bytes = create(
            &author,
            [3; 32],
            WorkspaceId::from_bytes([4; 16]),
            identity(7),
            [0; 32],
            [8; 32],
        )
        .unwrap();
        assert_eq!(decode(&bytes).unwrap().kind, KIND_CREATE);

        let wrong_recipient = seal_control(
            &author,
            KIND_CREATE,
            [3; 32],
            WorkspaceId::from_bytes([4; 16]),
            identity(9),
            [0; 32],
            &[8; 32],
        )
        .unwrap();
        assert_eq!(
            decode(&wrong_recipient),
            Err(StorageError::CorruptWorkspace)
        );

        let wrong_admission = seal_control(
            &author,
            KIND_CREATE,
            [3; 32],
            WorkspaceId::from_bytes([4; 16]),
            identity(7),
            [5; 32],
            &[8; 32],
        )
        .unwrap();
        assert_eq!(
            decode(&wrong_admission),
            Err(StorageError::CorruptWorkspace)
        );
    }

    #[test]
    fn decode_rejects_version_1_controls_as_legacy() {
        let mut bytes = sample_grant(&secret(7), identity(9), [8; 32]);
        bytes[..DOMAIN_V1.len()].copy_from_slice(DOMAIN_V1);

        assert_eq!(decode(&bytes), Err(StorageError::LegacyProtectionState));
    }

    // Verifies: TEAM.SYS.ID-R08
    #[test]
    fn control_signature_uses_domain_separated_context() {
        let bytes = sample_grant(&secret(7), identity(9), [8; 32]);
        let signature = Signature::from_bytes(&bytes[SIGNED_END..].try_into().unwrap());

        assert!(
            identity(7)
                .as_public_key()
                .verify(&bytes[..SIGNED_END], &signature)
                .is_err()
        );
        assert!(
            crate::identity::verify(
                identity(7).as_public_key(),
                SIGNING_CONTEXT,
                &bytes[..SIGNED_END],
                &signature,
            )
            .is_ok()
        );
    }

    #[test]
    fn concurrent_grants_are_idempotent_and_reject_key_split() {
        let mut projection = GrantProjection::default();
        assert!(projection.apply(slot([4; 32], 1)).unwrap());
        assert!(!projection.apply(slot([4; 32], 1)).unwrap());
        assert!(projection.apply(slot([4; 32], 2)).unwrap());
        assert_eq!(projection.len(), 2);
        assert!(projection.apply(slot([5; 32], 3)).is_err());
    }
}
