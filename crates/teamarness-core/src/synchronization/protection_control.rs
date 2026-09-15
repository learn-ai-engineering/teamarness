use std::collections::BTreeMap;

use iroh_base::{SecretKey, Signature};
use p2panda_core::Hash;
use rusqlite::Transaction;

use crate::error::StorageError;
use crate::{PublicIdentity, WorkspaceId};

const VERSION: u8 = 1;
const CREATE: u8 = 1;
const GRANT: u8 = 3;
const DOMAIN: &[u8] = b"teamarness.protection-control/v1";

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProtectionControl {
    pub(crate) kind: u8,
    pub(crate) scope: [u8; 32],
    pub(crate) workspace: WorkspaceId,
    pub(crate) author: PublicIdentity,
    pub(crate) recipient: PublicIdentity,
    pub(crate) admission: [u8; 32],
    pub(crate) content_key_id: [u8; 32],
    pub(crate) nonce: [u8; 24],
    pub(crate) grant: Vec<u8>,
    pub(crate) signature: [u8; 64],
}

fn wrapping_key(scope: [u8; 32], recipient: PublicIdentity) -> [u8; 32] {
    let mut bytes = Vec::with_capacity(DOMAIN.len() + 64);
    bytes.extend_from_slice(DOMAIN);
    bytes.extend_from_slice(&scope);
    bytes.extend_from_slice(recipient.as_bytes());
    *Hash::digest(bytes).as_bytes()
}

#[allow(clippy::too_many_arguments)]
fn unsigned_bytes(
    kind: u8,
    scope: [u8; 32],
    workspace: WorkspaceId,
    author: PublicIdentity,
    recipient: PublicIdentity,
    admission: [u8; 32],
    content_key_id: [u8; 32],
    nonce: [u8; 24],
    grant: &[u8],
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(1 + 1 + 32 + 16 + 32 * 4 + 24 + 4 + grant.len());
    bytes.extend_from_slice(DOMAIN);
    bytes.extend_from_slice(&[VERSION, kind]);
    bytes.extend_from_slice(&scope);
    bytes.extend_from_slice(workspace.as_bytes());
    bytes.extend_from_slice(author.as_bytes());
    bytes.extend_from_slice(recipient.as_bytes());
    bytes.extend_from_slice(&admission);
    bytes.extend_from_slice(&content_key_id);
    bytes.extend_from_slice(&nonce);
    bytes.extend_from_slice(&(grant.len() as u32).to_be_bytes());
    bytes.extend_from_slice(grant);
    bytes
}

pub(crate) fn create(
    secret: &SecretKey,
    scope: [u8; 32],
    workspace: WorkspaceId,
    recipient: PublicIdentity,
    admission: [u8; 32],
    content_key: [u8; 32],
) -> Vec<u8> {
    encode(
        secret,
        CREATE,
        scope,
        workspace,
        recipient,
        admission,
        *Hash::digest(content_key).as_bytes(),
        content_key,
    )
}

pub(crate) fn grant(
    secret: &SecretKey,
    scope: [u8; 32],
    workspace: WorkspaceId,
    recipient: PublicIdentity,
    admission: [u8; 32],
    content_key: [u8; 32],
) -> Vec<u8> {
    encode(
        secret,
        GRANT,
        scope,
        workspace,
        recipient,
        admission,
        *Hash::digest(content_key).as_bytes(),
        content_key,
    )
}

#[allow(clippy::too_many_arguments)]
fn encode(
    secret: &SecretKey,
    kind: u8,
    scope: [u8; 32],
    workspace: WorkspaceId,
    recipient: PublicIdentity,
    admission: [u8; 32],
    content_key_id: [u8; 32],
    content_key: [u8; 32],
) -> Vec<u8> {
    let key = wrapping_key(scope, recipient);
    let mut nonce = [0; 24];
    getrandom::fill(&mut nonce).expect("OS randomness");
    let grant = p2panda_encryption::crypto::xchacha20::x_aead_encrypt(
        &key,
        &content_key,
        nonce,
        Some(&scope),
    )
    .expect("valid protection grant");
    let author = PublicIdentity::from_bytes(*secret.public().as_bytes()).expect("identity");
    let unsigned = unsigned_bytes(
        kind,
        scope,
        workspace,
        author,
        recipient,
        admission,
        content_key_id,
        nonce,
        &grant,
    );
    let signature = secret.sign(&unsigned).to_bytes();
    let mut bytes = unsigned;
    bytes.extend_from_slice(author.as_bytes());
    bytes.extend_from_slice(&signature);
    bytes
}

pub(crate) fn decode(bytes: &[u8]) -> Result<ProtectionControl, StorageError> {
    if bytes.len() < DOMAIN.len() + 2 + 32 + 16 + 32 * 4 + 24 + 4 + 16 + 32 + 64
        || !bytes.starts_with(DOMAIN)
        || bytes[DOMAIN.len()] != VERSION
    {
        return Err(StorageError::CorruptWorkspace);
    }
    let mut at = DOMAIN.len() + 2;
    let take = |bytes: &[u8], at: &mut usize, n: usize| -> Result<Vec<u8>, StorageError> {
        let end = at.checked_add(n).ok_or(StorageError::CorruptWorkspace)?;
        let value = bytes.get(*at..end).ok_or(StorageError::CorruptWorkspace)?;
        *at = end;
        Ok(value.to_vec())
    };
    let scope: [u8; 32] = take(bytes, &mut at, 32)?
        .try_into()
        .map_err(|_| StorageError::CorruptWorkspace)?;
    let workspace = WorkspaceId::from_bytes(
        take(bytes, &mut at, 16)?
            .try_into()
            .map_err(|_| StorageError::CorruptWorkspace)?,
    );
    let author = PublicIdentity::from_bytes(
        take(bytes, &mut at, 32)?
            .try_into()
            .map_err(|_| StorageError::CorruptWorkspace)?,
    )
    .map_err(|_| StorageError::CorruptWorkspace)?;
    let recipient = PublicIdentity::from_bytes(
        take(bytes, &mut at, 32)?
            .try_into()
            .map_err(|_| StorageError::CorruptWorkspace)?,
    )
    .map_err(|_| StorageError::CorruptWorkspace)?;
    let admission: [u8; 32] = take(bytes, &mut at, 32)?
        .try_into()
        .map_err(|_| StorageError::CorruptWorkspace)?;
    let content_key_id: [u8; 32] = take(bytes, &mut at, 32)?
        .try_into()
        .map_err(|_| StorageError::CorruptWorkspace)?;
    let nonce: [u8; 24] = take(bytes, &mut at, 24)?
        .try_into()
        .map_err(|_| StorageError::CorruptWorkspace)?;
    let grant_len = u32::from_be_bytes(
        take(bytes, &mut at, 4)?
            .try_into()
            .map_err(|_| StorageError::CorruptWorkspace)?,
    ) as usize;
    let grant = take(bytes, &mut at, grant_len)?;
    let signed_at = at;
    let author_copy = PublicIdentity::from_bytes(
        take(bytes, &mut at, 32)?
            .try_into()
            .map_err(|_| StorageError::CorruptWorkspace)?,
    )
    .map_err(|_| StorageError::CorruptWorkspace)?;
    let signature: [u8; 64] = take(bytes, &mut at, 64)?
        .try_into()
        .map_err(|_| StorageError::CorruptWorkspace)?;
    if at != bytes.len() || author != author_copy {
        return Err(StorageError::CorruptWorkspace);
    }
    let signed = &bytes[..signed_at];
    author
        .as_public_key()
        .verify(signed, &Signature::from_bytes(&signature))
        .map_err(|_| StorageError::CorruptWorkspace)?;
    let key = p2panda_encryption::crypto::xchacha20::x_aead_decrypt(
        &wrapping_key(scope, recipient),
        &grant,
        nonce,
        Some(&scope),
    )
    .map_err(|_| StorageError::CorruptWorkspace)?;
    if key.len() != 32 || *Hash::digest(&key).as_bytes() != content_key_id {
        return Err(StorageError::CorruptWorkspace);
    }
    Ok(ProtectionControl {
        kind: bytes[DOMAIN.len() + 1],
        scope,
        workspace,
        author,
        recipient,
        admission,
        content_key_id,
        nonce,
        grant,
        signature,
    })
}

pub(crate) fn decrypt_key(control: &ProtectionControl) -> Result<[u8; 32], StorageError> {
    let key = p2panda_encryption::crypto::xchacha20::x_aead_decrypt(
        &wrapping_key(control.scope, control.recipient),
        &control.grant,
        control.nonce,
        Some(&control.scope),
    );
    // The grant nonce is intentionally validated during decode; import uses the
    // decoded key bytes below and never treats control bytes as a content key.
    key.map(|bytes| bytes.try_into().map_err(|_| StorageError::CorruptWorkspace))
        .map_err(|_| StorageError::CorruptWorkspace)?
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
    use iroh_base::SecretKey;

    fn identity(seed: u8) -> PublicIdentity {
        PublicIdentity::from_bytes(*SecretKey::from_bytes(&[seed; 32]).public().as_bytes()).unwrap()
    }

    fn slot(key_id: [u8; 32], admission: u8) -> GrantSlot {
        GrantSlot {
            recipient: identity(9),
            admission: [admission; 32],
            content_key_id: key_id,
            grant_bytes: vec![1, 2, 3],
        }
    }

    #[test]
    fn control_round_trip_binds_recipient_scope_and_key() {
        let sender = SecretKey::from_bytes(&[7; 32]);
        let recipient = identity(9);
        let control = grant(
            &sender,
            [3; 32],
            WorkspaceId::from_bytes([4; 16]),
            recipient,
            [5; 32],
            [8; 32],
        );
        let decoded = decode(&control).unwrap();
        assert_eq!(decoded.recipient, recipient);
        assert_eq!(decrypt_key(&decoded).unwrap(), [8; 32]);
        let mut tampered = control;
        tampered[DOMAIN.len() + 2] ^= 1;
        assert!(decode(&tampered).is_err());
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
