//! Private implementation seams for the immutable-record synchronization capability.
//!
//! The public API deliberately exposes only semantic values. P2Panda operation,
//! reconciliation, encryption, storage, and transport types stay in this module.

use std::fmt;
use std::str::FromStr;

use iroh_base::{SecretKey, Signature};
use p2panda_core::Hash;
use serde::{Deserialize, Serialize};

use crate::error::ValueParseError;
use crate::{PublicIdentity, WorkspaceId};

const VERSION: u8 = 1;
const MAX_PURPOSE: usize = 64;
const MAX_CANONICAL: usize = 262_144;
const MAX_OPEN_ITEMS: usize = 32;
const MAX_OPEN_BYTES: usize = 512 * 1024;
const MAX_PARENTS: usize = 64;
const RECORD_ID_DOMAIN: &[u8] = b"teamarness.immutable-record-id/v1";
const SCOPE_DOMAIN: &[u8] = b"teamarness.immutable-record-scope/v1";
const SIGNING_DOMAIN: &str = "teamarness.immutable-record/v1";

/// Stable equality identity for one authenticated immutable record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecordId([u8; 32]);

impl Serialize for RecordId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
impl<'de> Deserialize<'de> for RecordId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl RecordId {
    fn digest(bytes: &[u8]) -> Self {
        let mut input = Vec::with_capacity(RECORD_ID_DOMAIN.len() + bytes.len());
        input.extend_from_slice(RECORD_ID_DOMAIN);
        input.extend_from_slice(bytes);
        Self(*Hash::digest(input).as_bytes())
    }

    pub(crate) const fn as_bytes(self) -> [u8; 32] {
        self.0
    }
}

impl fmt::Display for RecordId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_hex(&self.0, f)
    }
}

impl FromStr for RecordId {
    type Err = ValueParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_hex(value, "record ID").map(Self)
    }
}

/// Immutable bytewise-sortable causal order. It is not a delivery cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OrderKey([u8; 41]);

impl Serialize for OrderKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
impl<'de> Deserialize<'de> for OrderKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl OrderKey {
    fn from_height_and_id(height: u64, record_id: RecordId) -> Self {
        let mut bytes = [0; 41];
        bytes[0] = VERSION;
        bytes[1..9].copy_from_slice(&height.to_be_bytes());
        bytes[9..].copy_from_slice(&record_id.0);
        Self(bytes)
    }

    pub(crate) const fn as_bytes(self) -> [u8; 41] {
        self.0
    }

    pub(crate) const fn height(self) -> u64 {
        u64::from_be_bytes([
            self.0[1], self.0[2], self.0[3], self.0[4], self.0[5], self.0[6], self.0[7], self.0[8],
        ])
    }
}

impl fmt::Display for OrderKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_hex(&self.0, f)
    }
}

impl FromStr for OrderKey {
    type Err = ValueParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_hex(value, "order key").and_then(|bytes| {
            if bytes.len() != 41 || bytes[0] != VERSION {
                return Err(ValueParseError::new("order key"));
            }
            Ok(Self(bytes))
        })
    }
}

/// Installation-local durable delivery checkpoint, never a distributed order position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cursor([u8; 25]);

impl Serialize for Cursor {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
impl<'de> Deserialize<'de> for Cursor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl Cursor {
    pub(crate) fn new(journal_id: [u8; 16], sequence: u64) -> Self {
        let mut bytes = [0; 25];
        bytes[0] = VERSION;
        bytes[1..17].copy_from_slice(&journal_id);
        bytes[17..].copy_from_slice(&sequence.to_be_bytes());
        Self(bytes)
    }

    pub(crate) const fn as_bytes(self) -> [u8; 25] {
        self.0
    }
}

impl fmt::Display for Cursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_hex(&self.0, f)
    }
}

impl FromStr for Cursor {
    type Err = ValueParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_hex(value, "cursor").and_then(|bytes| {
            if bytes.len() != 25 || bytes[0] != VERSION {
                return Err(ValueParseError::new("cursor"));
            }
            Ok(Self(bytes))
        })
    }
}

#[derive(Clone)]
struct SignedRecordV1 {
    package_identity: String,
    workspace_id: WorkspaceId,
    purpose: String,
    canonical: Vec<u8>,
    author: PublicIdentity,
    signature: Signature,
}

impl SignedRecordV1 {
    fn sign(
        secret_key: &SecretKey,
        package_identity: &str,
        workspace_id: WorkspaceId,
        purpose: &str,
        canonical: &[u8],
    ) -> Result<Self, ValueParseError> {
        validate_fields(package_identity, purpose, canonical)?;
        let author = PublicIdentity::from_bytes(*secret_key.public().as_bytes())
            .map_err(|_| ValueParseError::new("record author"))?;
        let unsigned = encode_unsigned(package_identity, workspace_id, purpose, canonical, author)?;
        let signature = crate::identity::sign(secret_key, SIGNING_DOMAIN, &unsigned)
            .map_err(|_| ValueParseError::new("record signature"))?;
        Ok(Self {
            package_identity: package_identity.to_owned(),
            workspace_id,
            purpose: purpose.to_owned(),
            canonical: canonical.to_vec(),
            author,
            signature,
        })
    }

    fn unsigned_bytes(&self) -> Vec<u8> {
        encode_unsigned(
            &self.package_identity,
            self.workspace_id,
            &self.purpose,
            &self.canonical,
            self.author,
        )
        .expect("validated signed record fields")
    }

    fn canonical_bytes(&self) -> Vec<u8> {
        let unsigned = self.unsigned_bytes();
        let mut bytes = Vec::with_capacity(unsigned.len() + 64);
        bytes.extend_from_slice(&unsigned);
        bytes.extend_from_slice(&self.signature.to_bytes());
        bytes
    }

    fn verify(&self) -> Result<(), ValueParseError> {
        let unsigned = self.unsigned_bytes();
        crate::identity::verify(
            self.author.as_public_key(),
            SIGNING_DOMAIN,
            &unsigned,
            &self.signature,
        )
        .map_err(|_| ValueParseError::new("record signature"))
    }
}

fn encode_unsigned(
    package_identity: &str,
    workspace_id: WorkspaceId,
    purpose: &str,
    canonical: &[u8],
    author: PublicIdentity,
) -> Result<Vec<u8>, ValueParseError> {
    validate_fields(package_identity, purpose, canonical)?;
    let package = package_identity.as_bytes();
    let purpose = purpose.as_bytes();
    let mut bytes = Vec::with_capacity(
        1 + 2 + package.len() + 16 + 2 + purpose.len() + 4 + canonical.len() + 32,
    );
    bytes.push(VERSION);
    bytes.extend_from_slice(&(package.len() as u16).to_be_bytes());
    bytes.extend_from_slice(package);
    bytes.extend_from_slice(workspace_id.as_bytes());
    bytes.extend_from_slice(&(purpose.len() as u16).to_be_bytes());
    bytes.extend_from_slice(purpose);
    bytes.extend_from_slice(&(canonical.len() as u32).to_be_bytes());
    bytes.extend_from_slice(canonical);
    bytes.extend_from_slice(author.as_bytes());
    Ok(bytes)
}

fn validate_open_batch(items: &[Vec<u8>]) -> Result<(), ValueParseError> {
    if items.is_empty() || items.len() > MAX_OPEN_ITEMS {
        return Err(ValueParseError::new("open batch item count"));
    }
    let total = items.iter().try_fold(0usize, |total, item| {
        total
            .checked_add(item.len())
            .ok_or(ValueParseError::new("open batch size"))
    })?;
    if total > MAX_OPEN_BYTES {
        return Err(ValueParseError::new("open batch size"));
    }
    Ok(())
}

fn validate_frontier(parent_count: usize) -> Result<(), ValueParseError> {
    if parent_count > MAX_PARENTS {
        Err(ValueParseError::new("causal frontier"))
    } else {
        Ok(())
    }
}

fn validate_fields(package: &str, purpose: &str, canonical: &[u8]) -> Result<(), ValueParseError> {
    if package.is_empty() || package.len() > u16::MAX as usize || !package.is_ascii() {
        return Err(ValueParseError::new("package identity"));
    }
    if purpose.is_empty()
        || purpose.len() > MAX_PURPOSE
        || !purpose.is_ascii()
        || !purpose.bytes().all(|b| b.is_ascii_graphic() && b != b' ')
    {
        return Err(ValueParseError::new("record purpose"));
    }
    if canonical.is_empty() || canonical.len() > MAX_CANONICAL {
        return Err(ValueParseError::new("canonical record"));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SealedRecordV1 {
    scope_digest: [u8; 32],
    record_id: RecordId,
    order_key: OrderKey,
    content_key_id: [u8; 32],
    nonce: [u8; 24],
    ciphertext: Vec<u8>,
}

impl SealedRecordV1 {
    fn prefix(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(1 + 1 + 32 + 32 + 41 + 32 + 24 + 4);
        bytes.extend_from_slice(&[VERSION, 1]);
        bytes.extend_from_slice(&self.scope_digest);
        bytes.extend_from_slice(&self.record_id.0);
        bytes.extend_from_slice(&self.order_key.0);
        bytes.extend_from_slice(&self.content_key_id);
        bytes.extend_from_slice(&self.nonce);
        bytes.extend_from_slice(&(self.ciphertext.len() as u32).to_be_bytes());
        bytes
    }

    fn encode(&self) -> Vec<u8> {
        let mut bytes = self.prefix();
        bytes.extend_from_slice(&self.ciphertext);
        bytes
    }

    fn decode(bytes: &[u8]) -> Result<Self, ValueParseError> {
        const PREFIX: usize = 1 + 1 + 32 + 32 + 41 + 32 + 24 + 4;
        if bytes.len() < PREFIX || bytes[0] != VERSION || bytes[1] != 1 {
            return Err(ValueParseError::new("sealed record"));
        }
        let ciphertext_len =
            u32::from_be_bytes(bytes[PREFIX - 4..PREFIX].try_into().expect("width")) as usize;
        if ciphertext_len == 0 || ciphertext_len != bytes.len() - PREFIX {
            return Err(ValueParseError::new("sealed record"));
        }
        let mut at = 2;
        let scope_digest = take_array::<32>(bytes, &mut at)?;
        let record_id = RecordId(take_array::<32>(bytes, &mut at)?);
        let order_key = OrderKey(take_array::<41>(bytes, &mut at)?);
        let content_key_id = take_array::<32>(bytes, &mut at)?;
        let nonce = take_array::<24>(bytes, &mut at)?;
        Ok(Self {
            scope_digest,
            record_id,
            order_key,
            content_key_id,
            nonce,
            ciphertext: bytes[PREFIX..].to_vec(),
        })
    }
}

/// Internal protection adapter. Raw key custody is deliberately not part of the public API.
trait RecordProtection {
    fn seal(
        &self,
        record: &SignedRecordV1,
        order_key: OrderKey,
    ) -> Result<SealedRecordV1, ValueParseError>;
    fn open(&self, sealed: &SealedRecordV1) -> Result<SignedRecordV1, ValueParseError>;
}

/// Production adapter uses P2Panda Encryption's XChaCha20-Poly1305 data primitive.
#[derive(Clone)]
struct P2pandaRecordProtection {
    package_identity: String,
    workspace_id: WorkspaceId,
    content_key: [u8; 32],
}

impl P2pandaRecordProtection {
    fn scope_digest(&self) -> [u8; 32] {
        let mut value = Vec::new();
        value.extend_from_slice(SCOPE_DOMAIN);
        value.extend_from_slice(self.package_identity.as_bytes());
        value.extend_from_slice(self.workspace_id.as_bytes());
        *Hash::digest(value).as_bytes()
    }

    fn key_id(&self) -> [u8; 32] {
        *Hash::digest(self.content_key).as_bytes()
    }
}

impl RecordProtection for P2pandaRecordProtection {
    fn seal(
        &self,
        record: &SignedRecordV1,
        order_key: OrderKey,
    ) -> Result<SealedRecordV1, ValueParseError> {
        let record_id = RecordId::digest(&record.canonical_bytes());
        let mut nonce = [0; 24];
        getrandom::fill(&mut nonce).map_err(|_| ValueParseError::new("record nonce"))?;
        let canonical = record.canonical_bytes();
        let plaintext_len = 4 + canonical.len() + 32;
        // XChaCha20-Poly1305 adds a 16-byte authentication tag. Including the
        // final ciphertext length in the bound prevents truncation/extension.
        let preliminary = SealedRecordV1 {
            scope_digest: self.scope_digest(),
            record_id,
            order_key,
            content_key_id: self.key_id(),
            nonce,
            ciphertext: vec![0; plaintext_len + 16],
        };
        let mut plaintext = (canonical.len() as u32).to_be_bytes().to_vec();
        plaintext.extend_from_slice(&canonical);
        plaintext.extend_from_slice(Hash::digest(preliminary.prefix()).as_bytes());
        let ciphertext = p2panda_encryption::crypto::xchacha20::x_aead_encrypt(
            &self.content_key,
            &plaintext,
            nonce,
            None,
        )
        .map_err(|_| ValueParseError::new("record encryption"))?;
        let sealed = SealedRecordV1 {
            ciphertext,
            ..preliminary
        };
        Ok(sealed)
    }

    fn open(&self, sealed: &SealedRecordV1) -> Result<SignedRecordV1, ValueParseError> {
        if sealed.scope_digest != self.scope_digest() || sealed.content_key_id != self.key_id() {
            return Err(ValueParseError::new("sealed record scope"));
        }
        let plaintext = p2panda_encryption::crypto::xchacha20::x_aead_decrypt(
            &self.content_key,
            &sealed.ciphertext,
            sealed.nonce,
            None,
        )
        .map_err(|_| ValueParseError::new("record decryption"))?;
        if plaintext.len() < 4 + 32 {
            return Err(ValueParseError::new("record plaintext"));
        }
        let canonical_len = u32::from_be_bytes(plaintext[..4].try_into().expect("width")) as usize;
        if canonical_len + 4 + 32 != plaintext.len() {
            return Err(ValueParseError::new("record plaintext"));
        }
        if plaintext[4 + canonical_len..] != *Hash::digest(sealed.prefix()).as_bytes() {
            return Err(ValueParseError::new("sealed record binding"));
        }
        let canonical = &plaintext[4..4 + canonical_len];
        let signature_offset = canonical
            .len()
            .checked_sub(64)
            .ok_or(ValueParseError::new("record plaintext"))?;
        let (unsigned, signature_bytes) = canonical.split_at(signature_offset);
        let parsed = decode_signed(unsigned, signature_bytes)?;
        if RecordId::digest(canonical) != sealed.record_id || parsed.verify().is_err() {
            return Err(ValueParseError::new("record authentication"));
        }
        Ok(parsed)
    }
}

fn decode_signed(unsigned: &[u8], signature: &[u8]) -> Result<SignedRecordV1, ValueParseError> {
    if signature.len() != 64 || unsigned.len() < 1 + 2 + 16 + 2 + 4 + 32 {
        return Err(ValueParseError::new("signed record"));
    }
    let mut at = 0;
    if take_byte(unsigned, &mut at)? != VERSION {
        return Err(ValueParseError::new("signed record version"));
    }
    let package_len = u16::from_be_bytes(take_array::<2>(unsigned, &mut at)?) as usize;
    let package = String::from_utf8(take_vec(unsigned, &mut at, package_len)?)
        .map_err(|_| ValueParseError::new("package identity"))?;
    let workspace_id = WorkspaceId::from_bytes(take_array::<16>(unsigned, &mut at)?);
    let purpose_len = u16::from_be_bytes(take_array::<2>(unsigned, &mut at)?) as usize;
    let purpose = String::from_utf8(take_vec(unsigned, &mut at, purpose_len)?)
        .map_err(|_| ValueParseError::new("record purpose"))?;
    let canonical_len = u32::from_be_bytes(take_array::<4>(unsigned, &mut at)?) as usize;
    let canonical = take_vec(unsigned, &mut at, canonical_len)?;
    let author = PublicIdentity::from_bytes(take_array::<32>(unsigned, &mut at)?)
        .map_err(|_| ValueParseError::new("record author"))?;
    if at != unsigned.len() {
        return Err(ValueParseError::new("signed record"));
    }
    let signature = Signature::from_bytes(signature.try_into().expect("checked signature width"));
    let record = SignedRecordV1 {
        package_identity: package,
        workspace_id,
        purpose,
        canonical,
        author,
        signature,
    };
    validate_fields(&record.package_identity, &record.purpose, &record.canonical)?;
    Ok(record)
}

fn take_byte(bytes: &[u8], at: &mut usize) -> Result<u8, ValueParseError> {
    if *at >= bytes.len() {
        return Err(ValueParseError::new("canonical record"));
    }
    let value = bytes[*at];
    *at += 1;
    Ok(value)
}
fn take_array<const N: usize>(bytes: &[u8], at: &mut usize) -> Result<[u8; N], ValueParseError> {
    if bytes.len().saturating_sub(*at) < N {
        return Err(ValueParseError::new("canonical record"));
    }
    let value = bytes[*at..*at + N].try_into().expect("checked width");
    *at += N;
    Ok(value)
}
fn take_vec(bytes: &[u8], at: &mut usize, len: usize) -> Result<Vec<u8>, ValueParseError> {
    if bytes.len().saturating_sub(*at) < len {
        return Err(ValueParseError::new("canonical record"));
    }
    let value = bytes[*at..*at + len].to_vec();
    *at += len;
    Ok(value)
}
fn parse_hex<const N: usize>(value: &str, label: &'static str) -> Result<[u8; N], ValueParseError> {
    if value.len() != N * 2 {
        return Err(ValueParseError::new(label));
    }
    let mut out = [0; N];
    for (index, pair) in value.as_bytes().chunks(2).enumerate() {
        out[index] = (hex_digit(pair[0]).ok_or(ValueParseError::new(label))? << 4)
            | hex_digit(pair[1]).ok_or(ValueParseError::new(label))?;
    }
    Ok(out)
}
fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}
fn write_hex(bytes: &[u8], f: &mut fmt::Formatter<'_>) -> fmt::Result {
    for byte in bytes {
        write!(f, "{byte:02x}")?;
    }
    Ok(())
}

// These private seams are intentionally named and kept independent so production and
// deterministic adapters can be tested through identical semantic behavior.
trait ReplicatedRecordStore {
    fn put(&mut self, record: SealedRecordV1) -> Result<(), ValueParseError>;
}
trait ReplicaEngine {
    fn record_id(&self, operation: &[u8]) -> RecordId;
}
trait PeerSessionTransport {
    fn bounds(&self) -> (usize, usize);
}
struct DeterministicStore(Vec<SealedRecordV1>);
impl ReplicatedRecordStore for DeterministicStore {
    fn put(&mut self, record: SealedRecordV1) -> Result<(), ValueParseError> {
        if !self.0.iter().any(|old| old.record_id == record.record_id) {
            self.0.push(record);
        }
        Ok(())
    }
}
struct DeterministicReplica;
impl ReplicaEngine for DeterministicReplica {
    fn record_id(&self, operation: &[u8]) -> RecordId {
        RecordId::digest(operation)
    }
}
struct DeterministicTransport;
impl PeerSessionTransport for DeterministicTransport {
    fn bounds(&self) -> (usize, usize) {
        (4_096, 64 * 1024 * 1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret(seed: u8) -> SecretKey {
        SecretKey::from_bytes(&[seed; 32])
    }
    fn signed() -> SignedRecordV1 {
        SignedRecordV1::sign(
            &secret(7),
            "teamarness.conversations",
            WorkspaceId::from_bytes([3; 16]),
            "message.post",
            b"canonical",
        )
        .unwrap()
    }

    pub(crate) mod encoding {
        use super::*;
        #[test]
        fn identity_order_and_cursor_are_distinct_and_canonical() {
            let record = signed();
            let id = RecordId::digest(&record.canonical_bytes());
            let key = OrderKey::from_height_and_id(9, id);
            let cursor = Cursor::new([4; 16], 11);
            assert_eq!(key.height(), 9);
            assert!(key > OrderKey::from_height_and_id(8, id));
            assert_ne!(id.to_string(), key.to_string());
            assert_eq!(cursor.to_string().len(), 50);
            assert_eq!(id, id.to_string().parse().unwrap());
            assert_eq!(key, key.to_string().parse().unwrap());
            assert_eq!(cursor, cursor.to_string().parse().unwrap());
        }

        #[test]
        fn malformed_contract_values_are_rejected() {
            assert!(RecordId::from_str("00").is_err());
            assert!(OrderKey::from_str(&"00".repeat(41)).is_err());
            assert!(Cursor::from_str(&"00".repeat(25)).is_err());
            assert!(
                SignedRecordV1::sign(
                    &secret(1),
                    "pkg",
                    WorkspaceId::from_bytes([0; 16]),
                    "",
                    b"x"
                )
                .is_err()
            );
            assert!(
                SignedRecordV1::sign(
                    &secret(1),
                    "pkg",
                    WorkspaceId::from_bytes([0; 16]),
                    "purpose",
                    &vec![0; MAX_CANONICAL + 1]
                )
                .is_err()
            );
            assert!(validate_open_batch(&[]).is_err());
            assert!(validate_open_batch(&[vec![0; MAX_OPEN_BYTES + 1]]).is_err());
            assert!(validate_frontier(MAX_PARENTS + 1).is_err());
        }
    }

    pub(crate) mod replica_compatibility {
        use super::*;
        #[test]
        fn deterministic_replica_and_store_deduplicate_operations() {
            let replica = DeterministicReplica;
            let operation = b"operation";
            assert_eq!(replica.record_id(operation), replica.record_id(operation));
            let record = signed();
            let id = RecordId::digest(&record.canonical_bytes());
            let sealed = SealedRecordV1 {
                scope_digest: [1; 32],
                record_id: id,
                order_key: OrderKey::from_height_and_id(0, id),
                content_key_id: [2; 32],
                nonce: [3; 24],
                ciphertext: vec![4],
            };
            let mut store = DeterministicStore(Vec::new());
            store.put(sealed.clone()).unwrap();
            store.put(sealed).unwrap();
            assert_eq!(store.0.len(), 1);
            assert_eq!(DeterministicTransport.bounds(), (4_096, 64 * 1024 * 1024));
        }
    }

    pub(crate) mod protection_compatibility {
        use super::*;
        #[test]
        fn p2panda_encryption_round_trip_rejects_tampering_and_wrong_scope() {
            let record = signed();
            let protector = P2pandaRecordProtection {
                package_identity: record.package_identity.clone(),
                workspace_id: record.workspace_id,
                content_key: [9; 32],
            };
            let id = RecordId::digest(&record.canonical_bytes());
            let sealed = protector
                .seal(&record, OrderKey::from_height_and_id(2, id))
                .unwrap();
            assert_eq!(SealedRecordV1::decode(&sealed.encode()).unwrap(), sealed);
            assert_eq!(protector.open(&sealed).unwrap().canonical, record.canonical);
            let mut altered = sealed.clone();
            altered.ciphertext[0] ^= 1;
            assert!(protector.open(&altered).is_err());
            let wrong = P2pandaRecordProtection {
                package_identity: "other.package".into(),
                ..protector
            };
            assert!(wrong.open(&sealed).is_err());
        }

        #[test]
        fn serialized_secret_and_concurrent_holders_keep_one_key_id() {
            let first = P2pandaRecordProtection {
                package_identity: "pkg".into(),
                workspace_id: WorkspaceId::from_bytes([1; 16]),
                content_key: [8; 32],
            };
            let second = P2pandaRecordProtection {
                content_key: first.content_key,
                ..first.clone()
            };
            assert_eq!(first.key_id(), second.key_id());
            let serialized = serde_json::to_vec(&first.content_key).unwrap();
            let restored: [u8; 32] = serde_json::from_slice(&serialized).unwrap();
            assert_eq!(restored, first.content_key);
        }
    }
}
