//! Private implementation seams for the immutable-record synchronization capability.
//!
//! The public API deliberately exposes only semantic values. P2Panda operation,
//! reconciliation, encryption, storage, and transport types stay in this module.

mod log_sync;
pub(crate) mod network_handler;
mod operation_store;
pub(crate) mod p2panda_adapter;
mod protection_control;
mod scheduler;
mod wakes;
mod worker;

pub(crate) use operation_store::log_id as operation_log_id;
pub(crate) use worker::SyncWorker;

pub(crate) use network_handler::{
    LogSyncHandler, PackageSyncHandler, SyncBatch, SyncReceiver, encode_batch,
};

use std::collections::VecDeque;
use std::fmt;
use std::str::FromStr;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use iroh_base::{SecretKey, Signature};
use p2panda_core::{Hash, Operation};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::error::{StorageError, ValueParseError};
use crate::{PublicIdentity, PublicSignature, WorkspaceId};

const VERSION: u8 = 1;
const MAX_PURPOSE: usize = 64;
const MAX_CANONICAL: usize = 262_144;
const MAX_OPEN_ITEMS: usize = 32;
const MAX_OPEN_BYTES: usize = 512 * 1024;
const MAX_PARENTS: usize = 64;
const RECORD_ID_DOMAIN: &[u8] = b"teamarness.immutable-record-id/v1";
const SCOPE_DOMAIN: &[u8] = b"teamarness.immutable-record-scope/v1";
const SIGNING_DOMAIN: &str = "teamarness.immutable-record/v1";
type ScopeState = ([u8; 16], [u8; 32], [u8; 32]);
type StoredRecordFields = (Vec<u8>, Vec<u8>, Vec<u8>, String, Vec<u8>, Vec<u8>);
type ScopeRecoveryRow = ([u8; 32], String, WorkspaceId, [u8; 32], [u8; 32]);
type RecordFields<'a> = (
    &'a [u8],
    &'a [u8],
    &'a [u8],
    &'a str,
    &'a [u8],
    &'a [u8],
    &'a [u8],
);

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

/// A committed immutable record returned by local append.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImmutableRecord {
    record_id: RecordId,
    order_key: OrderKey,
    author: PublicIdentity,
    purpose: String,
    signature: PublicSignature,
    canonical: Vec<u8>,
    sealed_record: Vec<u8>,
}
impl ImmutableRecord {
    #[must_use]
    pub const fn record_id(&self) -> RecordId {
        self.record_id
    }
    #[must_use]
    pub const fn order_key(&self) -> OrderKey {
        self.order_key
    }
    #[must_use]
    pub const fn author(&self) -> PublicIdentity {
        self.author
    }
    #[must_use]
    pub fn purpose(&self) -> &str {
        &self.purpose
    }
    #[must_use]
    pub const fn signature(&self) -> PublicSignature {
        self.signature
    }
    #[must_use]
    pub fn canonical(&self) -> &[u8] {
        &self.canonical
    }
    #[must_use]
    pub fn sealed_record(&self) -> &[u8] {
        &self.sealed_record
    }
    #[must_use]
    pub fn durability(&self) -> &'static str {
        "committed"
    }
}

/// One item in the durable Core delivery journal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordEvent {
    record: ImmutableRecord,
    cursor: Cursor,
}
impl RecordEvent {
    #[must_use]
    pub const fn record_id(&self) -> RecordId {
        self.record.record_id
    }
    #[must_use]
    pub const fn order_key(&self) -> OrderKey {
        self.record.order_key
    }
    #[must_use]
    pub const fn cursor(&self) -> Cursor {
        self.cursor
    }
    #[must_use]
    pub const fn author(&self) -> PublicIdentity {
        self.record.author
    }
    #[must_use]
    pub fn purpose(&self) -> &str {
        self.record.purpose()
    }
    #[must_use]
    pub fn canonical(&self) -> &[u8] {
        self.record.canonical()
    }
    #[must_use]
    pub fn sealed_record(&self) -> &[u8] {
        self.record.sealed_record()
    }
    #[must_use]
    pub fn record(&self) -> &ImmutableRecord {
        &self.record
    }
}

/// A record recovered from a sealed envelope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenedRecord {
    record_id: RecordId,
    order_key: OrderKey,
    author: PublicIdentity,
    purpose: String,
    signature: PublicSignature,
    canonical: Vec<u8>,
}
impl OpenedRecord {
    #[must_use]
    pub const fn record_id(&self) -> RecordId {
        self.record_id
    }
    #[must_use]
    pub const fn order_key(&self) -> OrderKey {
        self.order_key
    }
    #[must_use]
    pub const fn author(&self) -> PublicIdentity {
        self.author
    }
    #[must_use]
    pub fn purpose(&self) -> &str {
        &self.purpose
    }
    #[must_use]
    pub const fn signature(&self) -> PublicSignature {
        self.signature
    }
    #[must_use]
    pub fn canonical(&self) -> &[u8] {
        &self.canonical
    }
}

/// Errors from the local immutable-record capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SynchronizationError {
    WorkspaceNotFound,
    NotCurrentMember,
    RevokedMember,
    InvalidInput,
    WrongScope,
    CorruptRecord,
    CursorScopeMismatch,
    CursorAhead,
    RuntimeUnavailable,
    Storage(StorageError),
}
impl fmt::Display for SynchronizationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::WorkspaceNotFound => "synchronization workspace was not found",
            Self::NotCurrentMember => "only a current workspace member may append",
            Self::RevokedMember => "a revoked workspace member may not append",
            Self::InvalidInput => "invalid immutable record input",
            Self::WrongScope => "sealed record belongs to another scope",
            Self::CorruptRecord => "immutable record is corrupt",
            Self::CursorScopeMismatch => "cursor belongs to another delivery journal",
            Self::CursorAhead => "cursor is ahead of the delivery journal",
            Self::RuntimeUnavailable => "synchronization runtime is unavailable",
            Self::Storage(_) => "immutable record storage failed",
        })
    }
}
impl std::error::Error for SynchronizationError {}
impl From<StorageError> for SynchronizationError {
    fn from(e: StorageError) -> Self {
        Self::Storage(e)
    }
}

/// A blocking, at-least-once delivery subscription.
pub struct RecordSubscription {
    durable: VecDeque<RecordEvent>,
    live: Receiver<RecordEvent>,
}
impl fmt::Debug for RecordSubscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecordSubscription").finish_non_exhaustive()
    }
}
impl RecordSubscription {
    pub fn next_event(&mut self) -> Result<Option<RecordEvent>, SynchronizationError> {
        Ok(self.durable.pop_front().or_else(|| self.live.recv().ok()))
    }
    pub fn next_event_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<RecordEvent>, SynchronizationError> {
        if let Some(event) = self.durable.pop_front() {
            return Ok(Some(event));
        }
        match self.live.recv_timeout(timeout) {
            Ok(event) => Ok(Some(event)),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => Ok(None),
        }
    }
}

pub(crate) fn scope_digest(package_identity: &str, workspace_id: WorkspaceId) -> [u8; 32] {
    let mut input = Vec::with_capacity(SCOPE_DOMAIN.len() + package_identity.len() + 16);
    input.extend_from_slice(SCOPE_DOMAIN);
    input.extend_from_slice(package_identity.as_bytes());
    input.extend_from_slice(workspace_id.as_bytes());
    *Hash::digest(input).as_bytes()
}
fn key_id(key: &[u8; 32]) -> [u8; 32] {
    *Hash::digest(key).as_bytes()
}
fn storage_error() -> SynchronizationError {
    SynchronizationError::Storage(StorageError::Database)
}
fn fixed<const N: usize>(value: Vec<u8>) -> Result<[u8; N], SynchronizationError> {
    value
        .try_into()
        .map_err(|_| SynchronizationError::CorruptRecord)
}
fn public_signature(value: Vec<u8>) -> Result<PublicSignature, SynchronizationError> {
    Ok(PublicSignature::from_bytes(fixed(value)?))
}

fn ensure_scope(
    conn: &rusqlite::Transaction<'_>,
    package: &str,
    workspace: WorkspaceId,
    creator: PublicIdentity,
) -> Result<ScopeState, SynchronizationError> {
    let digest = scope_digest(package, workspace);
    let found: Option<(Vec<u8>, Vec<u8>)> = conn
        .query_row(
            "SELECT journal_id, content_key FROM sync_scopes WHERE scope_id=?1",
            [digest.as_slice()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|_| storage_error())?;
    if let Some((journal, key)) = found {
        return Ok((fixed(journal)?, fixed(key)?, digest));
    }
    let mut journal = [0_u8; 16];
    let mut key = [0_u8; 32];
    getrandom::fill(&mut journal).map_err(|_| storage_error())?;
    getrandom::fill(&mut key).map_err(|_| storage_error())?;
    conn.execute("INSERT INTO sync_scopes(scope_id,package_identity,workspace_id,journal_id,content_key_id,content_key) VALUES(?1,?2,?3,?4,?5,?6)", rusqlite::params![digest.as_slice(), package, workspace.as_bytes().as_slice(), journal.as_slice(), key_id(&key).as_slice(), key.as_slice()]).map_err(|_| storage_error())?;
    let mut bundle = b"teamarness.secret-bundle/v1".to_vec();
    bundle.extend_from_slice(&key);
    conn.execute(
        "INSERT INTO protection_state(scope_id,state_bytes,content_key_id,creator_public_identity) VALUES(?1,?2,?3,?4)",
        rusqlite::params![digest.as_slice(), bundle, key_id(&key).as_slice(), creator.as_bytes().as_slice()],
    )
    .map_err(|_| storage_error())?;
    conn.execute(
        "INSERT INTO durable_wakes(scope_id,wake_generation) VALUES(?1,0)",
        [digest.as_slice()],
    )
    .map_err(|_| storage_error())?;
    Ok((journal, key, digest))
}

pub(crate) fn export_protection_grant(
    conn: &mut rusqlite::Connection,
    secret: &SecretKey,
    workspace: WorkspaceId,
    package: &str,
    recipient: PublicIdentity,
    admission: [u8; 32],
) -> Result<Vec<u8>, SynchronizationError> {
    let scope = scope_digest(package, workspace);
    let key = conn
        .query_row(
            "SELECT content_key FROM sync_scopes WHERE scope_id=?1",
            [scope.as_slice()],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(|_| storage_error())?
        .ok_or(SynchronizationError::WorkspaceNotFound)
        .and_then(fixed)?;
    let author = secret.public();
    let existing = conn
        .query_row(
            "SELECT control_bytes FROM protection_control_operations
             WHERE scope_id=?1 AND operation_kind='grant'
               AND author_public_identity=?2 AND recipient_public_identity=?3
               AND admission_operation_id=?4
             LIMIT 1",
            rusqlite::params![
                scope.as_slice(),
                author.as_bytes().as_slice(),
                recipient.as_bytes().as_slice(),
                admission.as_slice()
            ],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(|_| storage_error())?;
    if let Some(existing) = existing {
        return Ok(existing);
    }
    let bytes = protection_control::grant(secret, scope, workspace, recipient, admission, key);
    let control =
        protection_control::decode(&bytes).map_err(|_| SynchronizationError::CorruptRecord)?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| storage_error())?;
    protection_control::insert(
        &tx,
        scope,
        RecordId::digest(&bytes).as_bytes(),
        "grant",
        &control,
        &bytes,
    )
    .map_err(|_| storage_error())?;
    tx.commit()
        .map_err(|_| SynchronizationError::Storage(StorageError::UncertainOutcome))?;
    Ok(bytes)
}

pub(crate) fn protection_control_author(
    bytes: &[u8],
) -> Result<PublicIdentity, SynchronizationError> {
    protection_control::decode(bytes)
        .map(|control| control.author)
        .map_err(|_| SynchronizationError::CorruptRecord)
}

pub(crate) fn import_protection_grant(
    conn: &mut rusqlite::Connection,
    local_identity: PublicIdentity,
    workspace: WorkspaceId,
    package: &str,
    bytes: &[u8],
) -> Result<usize, SynchronizationError> {
    let control =
        protection_control::decode(bytes).map_err(|_| SynchronizationError::CorruptRecord)?;
    let scope = scope_digest(package, workspace);
    if control.scope != scope
        || control.workspace != workspace
        || control.recipient != local_identity
        || !matches!(control.kind, 1 | 3)
    {
        return Err(SynchronizationError::WrongScope);
    }
    let key = protection_control::decrypt_key(&control)
        .map_err(|_| SynchronizationError::CorruptRecord)?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| storage_error())?;
    let existing: Option<Vec<u8>> = tx
        .query_row(
            "SELECT content_key_id FROM sync_scopes WHERE scope_id=?1",
            [scope.as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| storage_error())?;
    if let Some(existing) = existing {
        if existing != control.content_key_id {
            return Err(SynchronizationError::CorruptRecord);
        }
    } else {
        let mut journal = [0; 16];
        getrandom::fill(&mut journal).map_err(|_| storage_error())?;
        tx.execute(
            "INSERT INTO sync_scopes(scope_id,package_identity,workspace_id,journal_id,content_key_id,content_key) VALUES(?1,?2,?3,?4,?5,?6)",
            rusqlite::params![scope.as_slice(), package, workspace.as_bytes().as_slice(), journal.as_slice(), control.content_key_id.as_slice(), key.as_slice()],
        ).map_err(|_| storage_error())?;
        let mut bundle = b"teamarness.secret-bundle/v1".to_vec();
        bundle.extend_from_slice(&key);
        tx.execute(
            "INSERT INTO protection_state(scope_id,state_bytes,content_key_id,creator_public_identity) VALUES(?1,?2,?3,?4)",
            rusqlite::params![scope.as_slice(), bundle, control.content_key_id.as_slice(), control.author.as_bytes().as_slice()],
        ).map_err(|_| storage_error())?;
        tx.execute(
            "INSERT INTO durable_wakes(scope_id,wake_generation) VALUES(?1,0)",
            [scope.as_slice()],
        )
        .map_err(|_| storage_error())?;
    }
    protection_control::insert(
        &tx,
        scope,
        RecordId::digest(bytes).as_bytes(),
        if control.kind == 1 { "create" } else { "grant" },
        &control,
        bytes,
    )
    .map_err(|_| storage_error())?;
    let pending: Vec<Vec<u8>> = {
        let mut statement = tx.prepare("SELECT sealed_record FROM pending_protection_records WHERE scope_id=?1 ORDER BY pending_id").map_err(|_| storage_error())?;
        statement
            .query_map([scope.as_slice()], |row| row.get(0))
            .map_err(|_| storage_error())?
            .collect::<Result<_, _>>()
            .map_err(|_| storage_error())?
    };
    tx.commit()
        .map_err(|_| SynchronizationError::Storage(StorageError::UncertainOutcome))?;
    let mut promoted = 0;
    for sealed in pending {
        if insert_remote(conn, workspace, package, &sealed).is_ok() {
            conn.execute(
                "DELETE FROM pending_protection_records WHERE scope_id=?1 AND sealed_record=?2",
                rusqlite::params![scope.as_slice(), sealed],
            )
            .map_err(|_| storage_error())?;
            promoted += 1;
        }
    }
    Ok(promoted)
}

fn record_from_row(row: RecordFields<'_>) -> Result<ImmutableRecord, SynchronizationError> {
    Ok(ImmutableRecord {
        record_id: RecordId(fixed(row.0.to_vec())?),
        order_key: OrderKey(fixed(row.1.to_vec())?),
        author: PublicIdentity::from_bytes(fixed(row.2.to_vec())?)
            .map_err(|_| SynchronizationError::CorruptRecord)?,
        purpose: row.3.to_owned(),
        signature: public_signature(row.4.to_vec())?,
        canonical: row.5.to_vec(),
        sealed_record: row.6.to_vec(),
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn append(
    conn: &mut rusqlite::Connection,
    secret: &SecretKey,
    workspace: WorkspaceId,
    package: &str,
    purpose: &str,
    canonical: &[u8],
    is_member: bool,
    is_creator: bool,
) -> Result<ImmutableRecord, SynchronizationError> {
    if !is_member {
        return Err(SynchronizationError::NotCurrentMember);
    }
    let signed = SignedRecordV1::sign(secret, package, workspace, purpose, canonical)
        .map_err(|_| SynchronizationError::InvalidInput)?;
    let scope_exists = conn
        .query_row(
            "SELECT 1 FROM sync_scopes WHERE scope_id=?1",
            [scope_digest(package, workspace).as_slice()],
            |_| Ok(()),
        )
        .optional()
        .map_err(|_| storage_error())?
        .is_some();
    if !scope_exists && !is_creator {
        return Err(SynchronizationError::NotCurrentMember);
    }
    let canonical_signed = signed.canonical_bytes();
    let record_id = RecordId::digest(&canonical_signed);
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| storage_error())?;
    let (journal, key, scope) = ensure_scope(&tx, package, workspace, signed.author)?;
    let protection = P2pandaRecordProtection {
        package_identity: package.to_owned(),
        workspace_id: workspace,
        content_key: key,
    };
    let existing: Option<StoredRecordFields> = tx.query_row("SELECT o.record_id,o.order_key,o.author_public_identity,r.purpose,r.signature,r.sealed_record FROM immutable_operations o JOIN immutable_records r ON r.record_id=o.record_id JOIN immutable_idempotency i ON i.record_id=o.record_id WHERE i.scope_id=?1 AND i.record_id=?2", rusqlite::params![scope.as_slice(), record_id.as_bytes().as_slice()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional().map_err(|_| storage_error())?;
    if let Some((id, order, author, p, sig, sealed)) = existing {
        let opened = protection
            .open(
                &SealedRecordV1::decode(&sealed)
                    .map_err(|_| SynchronizationError::CorruptRecord)?,
            )
            .map_err(|_| SynchronizationError::CorruptRecord)?;
        return record_from_row((&id, &order, &author, &p, &sig, &opened.canonical, &sealed));
    }
    let max_height: Option<i64> = tx.query_row("SELECT MAX(causal_height) FROM immutable_operations WHERE scope_id=?1 AND status='accepted'", [scope.as_slice()], |r| r.get(0)).map_err(|_| storage_error())?;
    let height = u64::try_from(max_height.unwrap_or(-1).saturating_add(1))
        .map_err(|_| SynchronizationError::CorruptRecord)?;
    let order = OrderKey::from_height_and_id(height, record_id);
    let sealed = protection
        .seal(&signed, order)
        .map_err(|_| SynchronizationError::CorruptRecord)?;
    let sealed_bytes = sealed.encode();
    let operation_id = RecordId::digest(&sealed_bytes);
    let sequence: i64 = tx.query_row("SELECT COALESCE(MAX(author_sequence),-1)+1 FROM immutable_operations WHERE scope_id=?1 AND author_public_identity=?2", rusqlite::params![scope.as_slice(), signed.author.as_bytes().as_slice()], |r| r.get(0)).map_err(|_| storage_error())?;
    tx.execute("INSERT INTO immutable_operations(operation_id,scope_id,record_id,order_key,author_public_identity,author_sequence,operation_bytes,parents,causal_height,status) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'accepted')", rusqlite::params![operation_id.as_bytes().as_slice(),scope.as_slice(),record_id.as_bytes().as_slice(),order.as_bytes().as_slice(),signed.author.as_bytes().as_slice(),sequence,sealed_bytes.as_slice(),Vec::<u8>::new(),height]).map_err(|_| storage_error())?;
    let projection_operation_id = operation_store::insert(
        &tx,
        secret,
        scope,
        operation_id,
        record_id,
        order,
        signed.author,
        u32::try_from(sequence).map_err(|_| SynchronizationError::CorruptRecord)?,
        &sealed_bytes,
        &[],
        height,
        purpose,
        key_id(&key),
    )
    .map_err(|_| storage_error())?;
    if tx
        .query_row::<i64, _, _>(
            "SELECT COUNT(*) FROM protection_control_operations WHERE scope_id=?1",
            [scope.as_slice()],
            |row| row.get(0),
        )
        .map_err(|_| storage_error())?
        == 0
    {
        let control =
            protection_control::create(secret, scope, workspace, signed.author, [0; 32], key);
        let parsed = protection_control::decode(&control).map_err(|_| storage_error())?;
        protection_control::insert(
            &tx,
            scope,
            RecordId::digest(&control).as_bytes(),
            "create",
            &parsed,
            &control,
        )?;
        let _ = projection_operation_id;
    }
    tx.execute("INSERT INTO immutable_records(record_id,scope_id,purpose,canonical_digest,signature,sealed_record) VALUES(?1,?2,?3,?4,?5,?6)", rusqlite::params![record_id.as_bytes().as_slice(),scope.as_slice(),purpose,Hash::digest(canonical).as_bytes().as_slice(),signed.signature.to_bytes().as_slice(),sealed_bytes.as_slice()]).map_err(|_| storage_error())?;
    tx.execute(
        "INSERT INTO sealed_envelopes(record_id,scope_id,envelope) VALUES(?1,?2,?3)",
        rusqlite::params![
            record_id.as_bytes().as_slice(),
            scope.as_slice(),
            sealed_bytes.as_slice()
        ],
    )
    .map_err(|_| storage_error())?;
    tx.execute("INSERT INTO immutable_idempotency(scope_id,author_public_identity,purpose,canonical_digest,record_id) VALUES(?1,?2,?3,?4,?5)", rusqlite::params![scope.as_slice(),signed.author.as_bytes().as_slice(),purpose,Hash::digest(canonical).as_bytes().as_slice(),record_id.as_bytes().as_slice()]).map_err(|_| storage_error())?;
    tx.execute("INSERT INTO immutable_author_heads(scope_id,author_public_identity,operation_id) VALUES(?1,?2,?3) ON CONFLICT(scope_id,author_public_identity) DO UPDATE SET operation_id=excluded.operation_id", rusqlite::params![scope.as_slice(),signed.author.as_bytes().as_slice(),operation_id.as_bytes().as_slice()]).map_err(|_| storage_error())?;
    tx.execute("INSERT INTO record_delivery(scope_id,journal_id,record_id,created_at_unix_ms) VALUES(?1,?2,?3,?4)", rusqlite::params![scope.as_slice(),journal.as_slice(),record_id.as_bytes().as_slice(),0_i64]).map_err(|_| storage_error())?;
    tx.execute(
        "UPDATE durable_wakes SET wake_generation=wake_generation+1 WHERE scope_id=?1",
        [scope.as_slice()],
    )
    .map_err(|_| storage_error())?;
    tx.commit()
        .map_err(|_| SynchronizationError::Storage(StorageError::UncertainOutcome))?;
    record_from_row((
        record_id.as_bytes().as_slice(),
        order.as_bytes().as_slice(),
        signed.author.as_bytes(),
        purpose,
        signed.signature.to_bytes().as_slice(),
        canonical,
        sealed_bytes.as_slice(),
    ))
}

pub(crate) fn open_records(
    conn: &rusqlite::Connection,
    workspace: WorkspaceId,
    package: &str,
    envelopes: &[Vec<u8>],
) -> Result<Vec<OpenedRecord>, SynchronizationError> {
    validate_open_batch(envelopes).map_err(|_| SynchronizationError::InvalidInput)?;
    let scope = scope_digest(package, workspace);
    let key: Vec<u8> = conn
        .query_row(
            "SELECT content_key FROM sync_scopes WHERE scope_id=?1",
            [scope.as_slice()],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| storage_error())?
        .ok_or(SynchronizationError::WorkspaceNotFound)?;
    let key = fixed(key)?;
    let protector = P2pandaRecordProtection {
        package_identity: package.to_owned(),
        workspace_id: workspace,
        content_key: key,
    };
    envelopes
        .iter()
        .map(|bytes| {
            let sealed =
                SealedRecordV1::decode(bytes).map_err(|_| SynchronizationError::CorruptRecord)?;
            if sealed.scope_digest != scope {
                return Err(SynchronizationError::WrongScope);
            }
            let record = protector
                .open(&sealed)
                .map_err(|_| SynchronizationError::CorruptRecord)?;
            Ok(OpenedRecord {
                record_id: sealed.record_id,
                order_key: sealed.order_key,
                author: record.author,
                purpose: record.purpose,
                signature: PublicSignature::from_bytes(record.signature.to_bytes()),
                canonical: record.canonical,
            })
        })
        .collect()
}

pub(crate) fn subscribe(
    conn: &rusqlite::Connection,
    workspace: WorkspaceId,
    package: &str,
    after: Option<Cursor>,
    live: Receiver<RecordEvent>,
) -> Result<RecordSubscription, SynchronizationError> {
    let scope = scope_digest(package, workspace);
    let journal: Vec<u8> = conn
        .query_row(
            "SELECT journal_id FROM sync_scopes WHERE scope_id=?1",
            [scope.as_slice()],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| storage_error())?
        .ok_or(SynchronizationError::WorkspaceNotFound)?;
    let journal = fixed::<16>(journal)?;
    let start = match after {
        Some(cursor) => {
            let bytes = cursor.as_bytes();
            if bytes[1..17] != journal {
                return Err(SynchronizationError::CursorScopeMismatch);
            }
            i64::try_from(u64::from_be_bytes(
                bytes[17..25].try_into().expect("cursor width"),
            ))
            .map_err(|_| SynchronizationError::CursorAhead)?
        }
        None => 0,
    };
    let mut statement = conn.prepare("SELECT d.delivery_sequence,r.record_id,o.order_key,o.author_public_identity,r.purpose,r.signature,r.sealed_record FROM record_delivery d JOIN immutable_records r ON r.record_id=d.record_id JOIN immutable_operations o ON o.record_id=r.record_id WHERE d.scope_id=?1 AND d.delivery_sequence>?2 ORDER BY d.delivery_sequence").map_err(|_| storage_error())?;
    let rows = statement
        .query_map(rusqlite::params![scope.as_slice(), start], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, Vec<u8>>(2)?,
                r.get::<_, Vec<u8>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Vec<u8>>(5)?,
                r.get::<_, Vec<u8>>(6)?,
            ))
        })
        .map_err(|_| storage_error())?;
    let mut durable = VecDeque::new();
    for row in rows {
        let (sequence, id, order, author, purpose, sig, sealed) =
            row.map_err(|_| storage_error())?;
        let sealed_record =
            SealedRecordV1::decode(&sealed).map_err(|_| SynchronizationError::CorruptRecord)?;
        let key: [u8; 32] = conn
            .query_row(
                "SELECT content_key FROM sync_scopes WHERE scope_id=?1",
                [scope.as_slice()],
                |r| r.get(0),
            )
            .map_err(|_| storage_error())
            .and_then(fixed)?;
        let protection = P2pandaRecordProtection {
            package_identity: package.to_owned(),
            workspace_id: workspace,
            content_key: key,
        };
        let opened = protection
            .open(&sealed_record)
            .map_err(|_| SynchronizationError::CorruptRecord)?;
        let record = record_from_row((
            &id,
            &order,
            &author,
            &purpose,
            &sig,
            &opened.canonical,
            &sealed,
        ))?;
        durable.push_back(RecordEvent {
            record,
            cursor: Cursor::new(
                journal,
                u64::try_from(sequence).map_err(|_| SynchronizationError::CorruptRecord)?,
            ),
        });
    }
    Ok(RecordSubscription { durable, live })
}

pub(crate) fn insert_remote(
    conn: &mut rusqlite::Connection,
    workspace: WorkspaceId,
    package: &str,
    bytes: &[u8],
) -> Result<ImmutableRecord, SynchronizationError> {
    let scope = scope_digest(package, workspace);
    let sealed = SealedRecordV1::decode(bytes).map_err(|_| SynchronizationError::CorruptRecord)?;
    if sealed.scope_digest != scope {
        return Err(SynchronizationError::WrongScope);
    }
    let scope_keys: Option<([u8; 16], [u8; 32], [u8; 32])> = conn
        .query_row(
            "SELECT journal_id,content_key,content_key_id FROM sync_scopes WHERE scope_id=?1",
            [scope.as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    row.get::<_, Vec<u8>>(1)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    row.get::<_, Vec<u8>>(2)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                ))
            },
        )
        .optional()
        .map_err(|_| storage_error())?;
    let Some((journal, key, content_key_id)) = scope_keys else {
        conn.execute(
            "INSERT OR IGNORE INTO pending_protection_records(scope_id,package_identity,workspace_id,content_key_id,sealed_record) VALUES(?1,?2,?3,?4,?5)",
            rusqlite::params![scope.as_slice(), package, workspace.as_bytes().as_slice(), sealed.content_key_id.as_slice(), bytes],
        ).map_err(|_| storage_error())?;
        return Err(SynchronizationError::RuntimeUnavailable);
    };
    if content_key_id != sealed.content_key_id {
        conn.execute(
            "INSERT OR IGNORE INTO pending_protection_records(scope_id,package_identity,workspace_id,content_key_id,sealed_record) VALUES(?1,?2,?3,?4,?5)",
            rusqlite::params![scope.as_slice(), package, workspace.as_bytes().as_slice(), sealed.content_key_id.as_slice(), bytes],
        ).map_err(|_| storage_error())?;
        return Err(SynchronizationError::RuntimeUnavailable);
    }
    let protector = P2pandaRecordProtection {
        package_identity: package.to_owned(),
        workspace_id: workspace,
        content_key: key,
    };
    let signed = protector
        .open(&sealed)
        .map_err(|_| SynchronizationError::CorruptRecord)?;
    let record_id = RecordId::digest(&signed.canonical_bytes());
    if record_id != sealed.record_id {
        return Err(SynchronizationError::CorruptRecord);
    }
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| storage_error())?;
    let existing: Option<StoredRecordFields> = tx.query_row("SELECT o.record_id,o.order_key,o.author_public_identity,r.purpose,r.signature,r.sealed_record FROM immutable_operations o JOIN immutable_records r ON r.record_id=o.record_id WHERE o.record_id=?1", [record_id.as_bytes().as_slice()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional().map_err(|_| storage_error())?;
    if let Some((id, order, author, purpose, signature, sealed)) = existing {
        tx.commit()
            .map_err(|_| SynchronizationError::Storage(StorageError::UncertainOutcome))?;
        return record_from_row((
            &id,
            &order,
            &author,
            &purpose,
            &signature,
            &signed.canonical,
            &sealed,
        ));
    }
    let operation_id = RecordId::digest(bytes);
    let sequence: i64 = tx.query_row("SELECT COALESCE(MAX(author_sequence),-1)+1 FROM immutable_operations WHERE scope_id=?1 AND author_public_identity=?2", rusqlite::params![scope.as_slice(),signed.author.as_bytes().as_slice()], |row| row.get(0)).map_err(|_| storage_error())?;
    tx.execute("INSERT INTO immutable_operations(operation_id,scope_id,record_id,order_key,author_public_identity,author_sequence,operation_bytes,parents,causal_height,status) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'accepted')", rusqlite::params![operation_id.as_bytes().as_slice(),scope.as_slice(),record_id.as_bytes().as_slice(),sealed.order_key.as_bytes().as_slice(),signed.author.as_bytes().as_slice(),sequence,bytes,Vec::<u8>::new(),sealed.order_key.height()]).map_err(|_| storage_error())?;
    operation_store::queue_recovery(&tx, operation_id).map_err(|_| storage_error())?;
    tx.execute("INSERT INTO immutable_records(record_id,scope_id,purpose,canonical_digest,signature,sealed_record) VALUES(?1,?2,?3,?4,?5,?6)", rusqlite::params![record_id.as_bytes().as_slice(),scope.as_slice(),signed.purpose,Hash::digest(&signed.canonical).as_bytes().as_slice(),signed.signature.to_bytes().as_slice(),bytes]).map_err(|_| storage_error())?;
    tx.execute(
        "INSERT INTO sealed_envelopes(record_id,scope_id,envelope) VALUES(?1,?2,?3)",
        rusqlite::params![record_id.as_bytes().as_slice(), scope.as_slice(), bytes],
    )
    .map_err(|_| storage_error())?;
    tx.execute("INSERT INTO immutable_idempotency(scope_id,author_public_identity,purpose,canonical_digest,record_id) VALUES(?1,?2,?3,?4,?5)", rusqlite::params![scope.as_slice(),signed.author.as_bytes().as_slice(),signed.purpose,Hash::digest(&signed.canonical).as_bytes().as_slice(),record_id.as_bytes().as_slice()]).map_err(|_| storage_error())?;
    tx.execute("INSERT INTO immutable_author_heads(scope_id,author_public_identity,operation_id) VALUES(?1,?2,?3) ON CONFLICT(scope_id,author_public_identity) DO UPDATE SET operation_id=excluded.operation_id", rusqlite::params![scope.as_slice(),signed.author.as_bytes().as_slice(),operation_id.as_bytes().as_slice()]).map_err(|_| storage_error())?;
    tx.execute("INSERT INTO record_delivery(scope_id,journal_id,record_id,created_at_unix_ms) VALUES(?1,?2,?3,0)", rusqlite::params![scope.as_slice(),journal.as_slice(),record_id.as_bytes().as_slice()]).map_err(|_| storage_error())?;
    tx.commit()
        .map_err(|_| SynchronizationError::Storage(StorageError::UncertainOutcome))?;
    record_from_row((
        record_id.as_bytes().as_slice(),
        sealed.order_key.as_bytes().as_slice(),
        signed.author.as_bytes(),
        &signed.purpose,
        signed.signature.to_bytes().as_slice(),
        &signed.canonical,
        bytes,
    ))
}

pub(crate) fn insert_remote_operation(
    conn: &mut rusqlite::Connection,
    workspace: WorkspaceId,
    package: &str,
    operation: &Operation<()>,
) -> Result<ImmutableRecord, SynchronizationError> {
    // Validate the complete P2Panda operation before touching the Teamarness
    // journal. In particular, an occupied sequence or backlink conflict must
    // not leave behind a sealed record that recovery cannot project.
    p2panda_core::operation::validate_operation(operation)
        .map_err(|_| SynchronizationError::CorruptRecord)?;
    let payload = operation
        .body
        .as_ref()
        .ok_or(SynchronizationError::CorruptRecord)?
        .to_bytes();
    let scope = scope_digest(package, workspace);
    let sealed =
        SealedRecordV1::decode(&payload).map_err(|_| SynchronizationError::CorruptRecord)?;
    if sealed.scope_digest != scope {
        return Err(SynchronizationError::WrongScope);
    }
    let (journal, key, content_key_id): ([u8; 16], [u8; 32], [u8; 32]) = conn
        .query_row(
            "SELECT journal_id,content_key,content_key_id FROM sync_scopes WHERE scope_id=?1",
            [scope.as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    row.get::<_, Vec<u8>>(1)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    row.get::<_, Vec<u8>>(2)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                ))
            },
        )
        .optional()
        .map_err(|_| storage_error())?
        .ok_or(SynchronizationError::RuntimeUnavailable)?;
    if sealed.content_key_id != content_key_id {
        return Err(SynchronizationError::RuntimeUnavailable);
    }
    let protector = P2pandaRecordProtection {
        package_identity: package.to_owned(),
        workspace_id: workspace,
        content_key: key,
    };
    let signed = protector
        .open(&sealed)
        .map_err(|_| SynchronizationError::CorruptRecord)?;
    if operation.header.verifying_key.as_bytes() != signed.author.as_bytes() {
        return Err(SynchronizationError::CorruptRecord);
    }
    let record_id = RecordId::digest(&signed.canonical_bytes());
    if record_id != sealed.record_id {
        return Err(SynchronizationError::CorruptRecord);
    }
    let teamarness_operation_id = RecordId::digest(&payload);
    let transaction = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| storage_error())?;
    let existing: Option<Vec<u8>> = transaction
        .query_row(
            "SELECT teamarness_operation_id FROM p2panda_operations WHERE operation_id=?1",
            [operation.hash.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| storage_error())?;
    if let Some(existing) = existing {
        if existing != teamarness_operation_id.as_bytes() {
            return Err(SynchronizationError::CorruptRecord);
        }
        transaction
            .commit()
            .map_err(|_| SynchronizationError::Storage(StorageError::UncertainOutcome))?;
        return record_from_row((
            record_id.as_bytes().as_slice(),
            sealed.order_key.as_bytes().as_slice(),
            signed.author.as_bytes(),
            &signed.purpose,
            signed.signature.to_bytes().as_slice(),
            &signed.canonical,
            &payload,
        ));
    }
    let expected = transaction
        .query_row(
            "SELECT COALESCE(MAX(author_sequence),-1)+1 FROM p2panda_operations
             WHERE scope_id=?1 AND author_public_identity=?2",
            rusqlite::params![scope.as_slice(), operation.header.verifying_key.as_bytes()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|_| storage_error())?;
    if i64::from(operation.header.seq_num) != expected {
        return Err(SynchronizationError::CorruptRecord);
    }
    let expected_backlink: Option<Vec<u8>> = if operation.header.seq_num == 0 {
        None
    } else {
        transaction
            .query_row(
                "SELECT operation_id FROM p2panda_operations
                 WHERE scope_id=?1 AND author_public_identity=?2 AND author_sequence=?3",
                rusqlite::params![
                    scope.as_slice(),
                    operation.header.verifying_key.as_bytes(),
                    i64::from(operation.header.seq_num - 1),
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| storage_error())?
    };
    let actual_backlink = operation
        .header
        .backlink
        .map(|value| value.as_bytes().to_vec());
    if expected_backlink != actual_backlink {
        return Err(SynchronizationError::CorruptRecord);
    }
    // All Teamarness rows and the P2Panda projection share this transaction.
    // An invalid projection therefore rolls back the accepted record too.
    transaction
        .execute(
            "INSERT INTO immutable_operations(operation_id,scope_id,record_id,order_key,author_public_identity,author_sequence,operation_bytes,parents,causal_height,status)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'accepted')",
            rusqlite::params![
                teamarness_operation_id.as_bytes().as_slice(),
                scope.as_slice(),
                record_id.as_bytes().as_slice(),
                sealed.order_key.as_bytes().as_slice(),
                signed.author.as_bytes(),
                i64::from(operation.header.seq_num),
                payload.as_slice(),
                Vec::<u8>::new(),
                sealed.order_key.height(),
            ],
        )
        .map_err(|_| SynchronizationError::CorruptRecord)?;
    operation_store::insert_remote(
        &transaction,
        scope,
        teamarness_operation_id,
        record_id,
        sealed.order_key,
        operation,
        &signed.purpose,
        sealed.content_key_id,
    )
    .map_err(|_| SynchronizationError::CorruptRecord)?;
    transaction
        .execute(
            "INSERT INTO immutable_records(record_id,scope_id,purpose,canonical_digest,signature,sealed_record)
             VALUES(?1,?2,?3,?4,?5,?6)",
            rusqlite::params![
                record_id.as_bytes().as_slice(),
                scope.as_slice(),
                signed.purpose,
                Hash::digest(&signed.canonical).as_bytes().as_slice(),
                signed.signature.to_bytes().as_slice(),
                payload.as_slice(),
            ],
        )
        .map_err(|_| SynchronizationError::CorruptRecord)?;
    transaction
        .execute(
            "INSERT INTO sealed_envelopes(record_id,scope_id,envelope) VALUES(?1,?2,?3)",
            rusqlite::params![
                record_id.as_bytes().as_slice(),
                scope.as_slice(),
                payload.as_slice()
            ],
        )
        .map_err(|_| SynchronizationError::CorruptRecord)?;
    transaction
        .execute(
            "INSERT INTO immutable_idempotency(scope_id,author_public_identity,purpose,canonical_digest,record_id)
             VALUES(?1,?2,?3,?4,?5)",
            rusqlite::params![
                scope.as_slice(),
                signed.author.as_bytes(),
                signed.purpose,
                Hash::digest(&signed.canonical).as_bytes().as_slice(),
                record_id.as_bytes().as_slice(),
            ],
        )
        .map_err(|_| SynchronizationError::CorruptRecord)?;
    transaction
        .execute(
            "INSERT INTO immutable_author_heads(scope_id,author_public_identity,operation_id)
             VALUES(?1,?2,?3)
             ON CONFLICT(scope_id,author_public_identity) DO UPDATE SET operation_id=excluded.operation_id",
            rusqlite::params![scope.as_slice(), signed.author.as_bytes(), teamarness_operation_id.as_bytes().as_slice()],
        )
        .map_err(|_| SynchronizationError::CorruptRecord)?;
    transaction
        .execute(
            "INSERT INTO record_delivery(scope_id,journal_id,record_id,created_at_unix_ms)
             VALUES(?1,?2,?3,0)",
            rusqlite::params![
                scope.as_slice(),
                journal.as_slice(),
                record_id.as_bytes().as_slice()
            ],
        )
        .map_err(|_| SynchronizationError::CorruptRecord)?;
    transaction
        .execute(
            "UPDATE durable_wakes SET wake_generation=wake_generation+1 WHERE scope_id=?1",
            [scope.as_slice()],
        )
        .map_err(|_| SynchronizationError::CorruptRecord)?;
    transaction
        .commit()
        .map_err(|_| SynchronizationError::Storage(StorageError::UncertainOutcome))?;
    record_from_row((
        record_id.as_bytes().as_slice(),
        sealed.order_key.as_bytes().as_slice(),
        signed.author.as_bytes(),
        &signed.purpose,
        signed.signature.to_bytes().as_slice(),
        &signed.canonical,
        &payload,
    ))
}

pub(crate) fn records_for_replication(
    conn: &rusqlite::Connection,
    workspace: WorkspaceId,
    package: &str,
) -> Result<Vec<Vec<u8>>, SynchronizationError> {
    let scope = scope_digest(package, workspace);
    let mut statement = conn
        .prepare("SELECT sealed_record FROM immutable_records WHERE scope_id=?1 ORDER BY record_id")
        .map_err(|_| storage_error())?;
    let rows = statement
        .query_map([scope.as_slice()], |row| row.get::<_, Vec<u8>>(0))
        .map_err(|_| storage_error())?;
    rows.map(|row| row.map_err(|_| storage_error())).collect()
}

pub(crate) fn event_for_record(
    conn: &rusqlite::Connection,
    workspace: WorkspaceId,
    package: &str,
    record: ImmutableRecord,
) -> Option<RecordEvent> {
    let scope = scope_digest(package, workspace);
    let (journal, sequence): (Vec<u8>, i64) = conn
        .query_row(
            "SELECT sync_scopes.journal_id, record_delivery.delivery_sequence
         FROM sync_scopes JOIN record_delivery USING(scope_id)
         WHERE sync_scopes.scope_id=?1 AND record_delivery.record_id=?2",
            rusqlite::params![scope.as_slice(), record.record_id.as_bytes().as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok()?;
    let journal: [u8; 16] = journal.try_into().ok()?;
    let sequence = u64::try_from(sequence).ok()?;
    Some(RecordEvent {
        record,
        cursor: Cursor::new(journal, sequence),
    })
}

pub(crate) fn recover(
    conn: &mut rusqlite::Connection,
    secret: &SecretKey,
) -> Result<(), StorageError> {
    let scopes: Vec<ScopeRecoveryRow> = {
        let mut stmt = conn.prepare("SELECT scope_id,package_identity,workspace_id,content_key,content_key_id FROM sync_scopes").map_err(|_| StorageError::Database)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                    r.get::<_, Vec<u8>>(3)?,
                    r.get::<_, Vec<u8>>(4)?,
                ))
            })
            .map_err(|_| StorageError::Database)?;
        rows.map(|row| {
            let (scope, package, workspace, key, id) =
                row.map_err(|_| StorageError::CorruptWorkspace)?;
            Ok((
                scope
                    .try_into()
                    .map_err(|_| StorageError::CorruptWorkspace)?,
                package,
                WorkspaceId::from_bytes(
                    workspace
                        .try_into()
                        .map_err(|_| StorageError::CorruptWorkspace)?,
                ),
                key.try_into().map_err(|_| StorageError::CorruptWorkspace)?,
                id.try_into().map_err(|_| StorageError::CorruptWorkspace)?,
            ))
        })
        .collect::<Result<_, StorageError>>()?
    };
    operation_store::repair(conn, secret)?;
    operation_store::validate(conn)?;
    for (scope, package, workspace, key, id) in scopes {
        if scope_digest(&package, workspace) != scope || key_id(&key) != id {
            return Err(StorageError::CorruptWorkspace);
        }
        let protector = P2pandaRecordProtection {
            package_identity: package,
            workspace_id: workspace,
            content_key: key,
        };
        let mut stmt = conn
            .prepare(
                "SELECT r.record_id,o.order_key,r.sealed_record FROM immutable_records r JOIN immutable_operations o ON o.record_id=r.record_id WHERE r.scope_id=?1",
            )
            .map_err(|_| StorageError::Database)?;
        let rows = stmt
            .query_map([scope.as_slice()], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(|_| StorageError::Database)?;
        for row in rows {
            let (record_id, order, bytes) = row.map_err(|_| StorageError::CorruptWorkspace)?;
            let sealed =
                SealedRecordV1::decode(&bytes).map_err(|_| StorageError::CorruptWorkspace)?;
            let expected_id: [u8; 32] = record_id
                .try_into()
                .map_err(|_| StorageError::CorruptWorkspace)?;
            let expected_order: [u8; 41] = order
                .try_into()
                .map_err(|_| StorageError::CorruptWorkspace)?;
            if sealed.scope_digest != scope
                || sealed.record_id.as_bytes() != expected_id
                || sealed.order_key.as_bytes() != expected_order
                || protector.open(&sealed).is_err()
            {
                return Err(StorageError::CorruptWorkspace);
            }
        }
    }
    Ok(())
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
