//! Private implementation seams for the immutable-record synchronization capability.
//!
//! The public API deliberately exposes only semantic values. P2Panda operation,
//! reconciliation, encryption, storage, and transport types stay in this module.

pub(crate) mod client;
mod content_key;
pub(crate) mod exchange;
pub(crate) mod frames;
pub(crate) mod health;
mod journal_sql;
pub(crate) mod key_bridge;
pub(crate) mod limits;
pub(crate) mod log_view;
pub(crate) mod network_handler;
mod operation_store;
pub(crate) mod p2panda_adapter;
pub(crate) mod parking;
pub(crate) mod peer_sessions;
pub(crate) mod protection_control;
pub(crate) mod quarantine;
pub(crate) mod record_subscription;
pub(crate) mod scheduler;
#[cfg(test)]
pub(crate) mod test_io;
pub(crate) mod worker;

pub(crate) use content_key::ContentKey;
pub(crate) use health::{PeerOutcome, SyncHealthRegistry};
#[cfg(test)]
pub(crate) use operation_store::log_id as operation_log_id;
pub(crate) use quarantine::RecoveryReport;
pub use record_subscription::{
    RecordAvailability, RecordPage, RecordSubscription, SyncAvailability, SyncWarning,
};
pub(crate) use worker::SyncWorker;

pub(crate) use exchange::ExchangeReport;
pub(crate) use network_handler::{
    FatalSyncError, LogSessionError, LogSyncHandler, OperationOutcome, ProtectionBundle,
    RangeLoadError, Refusal, RejectReason, ReplicationEntry, SessionGrant, SessionSeed,
    SyncReceiver, SyncRejection,
};

/// How many stored operation rows sessions have read, per scope.
///
/// A session that serves a peer which is already up to date must read none.
/// The count is per scope because every test in this binary runs in one
/// process: a single global counter measures every concurrent test's reads as
/// well as the caller's, which is what made
/// `session_does_not_load_the_whole_scope` flake. A scope digest is derived
/// from a Package identity and a Workspace identity, and a test's Workspace is
/// its own.
#[cfg(test)]
static RECORDS_LOADED: std::sync::Mutex<std::collections::BTreeMap<[u8; 32], usize>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

#[cfg(test)]
pub(crate) fn note_record_loaded_for_test(scope: [u8; 32]) {
    *RECORDS_LOADED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(scope)
        .or_default() += 1;
}

#[cfg(test)]
pub(crate) fn records_loaded_for_test(scope: [u8; 32]) -> usize {
    RECORDS_LOADED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&scope)
        .copied()
        .unwrap_or(0)
}
use crate::diag::fault;
use crate::encoding::ByteReader;
pub(crate) use peer_sessions::PeerSessions;

/// The Core services one bound Network needs to serve and run synchronization
/// sessions. Plan phase 3 Step 8 adds the membership replica to this bundle.
#[derive(Clone)]
pub(crate) struct SyncServices {
    pub(crate) receiver: std::sync::Arc<dyn SyncReceiver>,
    pub(crate) sessions: std::sync::Arc<PeerSessions>,
    /// Where every session reports what it saw of its peer, so subscriptions
    /// can say whether this Installation is reaching anyone.
    pub(crate) health: std::sync::Arc<SyncHealthRegistry>,
    /// Every bound one session obeys. Tests inject small values here, the same
    /// way they inject parking limits.
    pub(crate) limits: limits::SessionLimits,
}

impl fmt::Debug for SyncServices {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SyncServices")
            .finish_non_exhaustive()
    }
}

use std::fmt;
use std::str::FromStr;

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
const RECORD_ID_DOMAIN: &[u8] = b"teamarness.immutable-record-id/v1";
const SCOPE_DOMAIN: &[u8] = b"teamarness.immutable-record-scope/v1";
const SIGNING_DOMAIN: &str = "teamarness.immutable-record/v1";
type ScopeState = ([u8; 16], ContentKey, [u8; 32]);
type StoredRecordFields = (Vec<u8>, Vec<u8>, Vec<u8>, String, Vec<u8>, Vec<u8>);
type ScopeRecoveryRow = ([u8; 32], String, WorkspaceId, ContentKey, [u8; 32]);
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

    /// Names a record identity that belongs to no record, for forgery tests.
    #[cfg(test)]
    pub(crate) const fn from_bytes_for_test(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl fmt::Display for RecordId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        crate::encoding::write_hex(&self.0, f)
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
    pub(crate) fn from_height_and_id(height: u64, record_id: RecordId) -> Self {
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
        crate::encoding::write_hex(&self.0, f)
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
        crate::encoding::write_hex(&self.0, f)
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
        let mut reader = ByteReader::new(&bytes[2..]);
        let scope_digest = reader.take_array::<32>().ok_or_else(malformed)?;
        let record_id = RecordId(reader.take_array::<32>().ok_or_else(malformed)?);
        let order_key = OrderKey(reader.take_array::<41>().ok_or_else(malformed)?);
        // An order key is part of the sealed bytes, so a peer cannot move a
        // record in the order without breaking this binding.
        if order_key.0[0] != VERSION || order_key.0[9..41] != record_id.0 {
            return Err(ValueParseError::new("order key binding"));
        }
        let content_key_id = reader.take_array::<32>().ok_or_else(malformed)?;
        let nonce = reader.take_array::<24>().ok_or_else(malformed)?;
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
    content_key: ContentKey,
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
        self.content_key.id()
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
            self.content_key.expose(),
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
            self.content_key.expose(),
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
        // SEC-05: the author signs the package and workspace it wrote for, so a
        // record sealed into another scope never opens there.
        if parsed.package_identity != self.package_identity
            || parsed.workspace_id != self.workspace_id
        {
            return Err(ValueParseError::new(SCOPE_BINDING));
        }
        Ok(parsed)
    }
}

/// The value name [`RecordProtection::open`] reports when a record is signed for
/// a package or workspace other than the scope that sealed it.
const SCOPE_BINDING: &str = "record scope binding";

/// Distinguishes a record signed for another scope from an otherwise unreadable
/// record, so one forged record is rejected as such instead of as corruption.
fn map_open_error(error: &ValueParseError) -> SynchronizationError {
    if error.value_name() == SCOPE_BINDING {
        SynchronizationError::WrongScope
    } else {
        SynchronizationError::CorruptRecord
    }
}

fn decode_signed(unsigned: &[u8], signature: &[u8]) -> Result<SignedRecordV1, ValueParseError> {
    if signature.len() != 64 || unsigned.len() < 1 + 2 + 16 + 2 + 4 + 32 {
        return Err(ValueParseError::new("signed record"));
    }
    let mut reader = ByteReader::new(unsigned);
    if reader.take_u8().ok_or_else(malformed)? != VERSION {
        return Err(ValueParseError::new("signed record version"));
    }
    let package_len = u16::from_be_bytes(reader.take_array::<2>().ok_or_else(malformed)?) as usize;
    let package = String::from_utf8(reader.take(package_len).ok_or_else(malformed)?.to_vec())
        .map_err(|_| ValueParseError::new("package identity"))?;
    let workspace_id = WorkspaceId::from_bytes(reader.take_array::<16>().ok_or_else(malformed)?);
    let purpose_len = u16::from_be_bytes(reader.take_array::<2>().ok_or_else(malformed)?) as usize;
    let purpose = String::from_utf8(reader.take(purpose_len).ok_or_else(malformed)?.to_vec())
        .map_err(|_| ValueParseError::new("record purpose"))?;
    let canonical_len =
        u32::from_be_bytes(reader.take_array::<4>().ok_or_else(malformed)?) as usize;
    let canonical = reader.take(canonical_len).ok_or_else(malformed)?.to_vec();
    let author = PublicIdentity::from_bytes(reader.take_array::<32>().ok_or_else(malformed)?)
        .map_err(|_| ValueParseError::new("record author"))?;
    if reader.finish().is_none() {
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

/// The shared reader, with this codec's one error value.
fn malformed() -> ValueParseError {
    ValueParseError::new("canonical record")
}

fn parse_hex<const N: usize>(value: &str, label: &'static str) -> Result<[u8; N], ValueParseError> {
    crate::encoding::hex_decode_fixed(value).ok_or_else(|| ValueParseError::new(label))
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
    ContentKeyUnavailable,
    KeyOriginRejected,
    CorruptRecord,
    ScopeQuarantined,
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
            Self::ContentKeyUnavailable => "the package content key has not been received yet",
            Self::KeyOriginRejected => {
                "protection grant does not trace to the workspace creator's key"
            }
            Self::CorruptRecord => "immutable record is corrupt",
            Self::ScopeQuarantined => {
                "the package record log is quarantined after a local integrity failure"
            }
            Self::CursorScopeMismatch => "cursor belongs to another delivery journal",
            Self::CursorAhead => "cursor is ahead of the delivery journal",
            Self::RuntimeUnavailable => "synchronization runtime is unavailable",
            Self::Storage(_) => "immutable record storage failed",
        })
    }
}
impl std::error::Error for SynchronizationError {}
/// Keeps a refusal of retired version-1 protection controls distinguishable
/// from an ordinary corrupt record, so startup can tell the user to reset.
fn storage_or_corrupt(error: StorageError) -> SynchronizationError {
    if matches!(error, StorageError::LegacyProtectionState) {
        SynchronizationError::Storage(error)
    } else {
        SynchronizationError::CorruptRecord
    }
}

impl From<StorageError> for SynchronizationError {
    fn from(e: StorageError) -> Self {
        Self::Storage(e)
    }
}

pub(crate) fn scope_digest(package_identity: &str, workspace_id: WorkspaceId) -> [u8; 32] {
    let mut input = Vec::with_capacity(SCOPE_DOMAIN.len() + package_identity.len() + 16);
    input.extend_from_slice(SCOPE_DOMAIN);
    input.extend_from_slice(package_identity.as_bytes());
    input.extend_from_slice(workspace_id.as_bytes());
    *Hash::digest(input).as_bytes()
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
    secret: &SecretKey,
    package: &str,
    workspace: WorkspaceId,
) -> Result<(ScopeState, bool), SynchronizationError> {
    let digest = scope_digest(package, workspace);
    let found: Option<(Vec<u8>, Vec<u8>)> = conn
        .query_row(
            "SELECT journal_id, content_key FROM sync_scopes WHERE scope_id=?1",
            [digest.as_slice()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(fault!(storage_error()))?;
    if let Some((journal, key)) = found {
        return Ok((
            (fixed(journal)?, ContentKey::from_column(key)?, digest),
            false,
        ));
    }
    let creator = PublicIdentity::from_bytes(*secret.public().as_bytes())
        .map_err(|_| SynchronizationError::CorruptRecord)?;
    let mut journal = [0_u8; 16];
    getrandom::fill(&mut journal).map_err(fault!(storage_error()))?;
    let key = ContentKey::generate()?;
    conn.execute("INSERT INTO sync_scopes(scope_id,package_identity,workspace_id,journal_id,content_key_id,content_key) VALUES(?1,?2,?3,?4,?5,?6)", rusqlite::params![digest.as_slice(), package, workspace.as_bytes().as_slice(), journal.as_slice(), key.id().as_slice(), key.expose().as_slice()]).map_err(fault!(storage_error()))?;
    conn.execute(
        "INSERT INTO protection_state(scope_id,content_key_id,creator_public_identity) VALUES(?1,?2,?3)",
        rusqlite::params![digest.as_slice(), key.id().as_slice(), creator.as_bytes().as_slice()],
    )
    .map_err(fault!(storage_error()))?;
    conn.execute(
        "INSERT INTO durable_wakes(scope_id,wake_generation) VALUES(?1,0)",
        [digest.as_slice()],
    )
    .map_err(fault!(storage_error()))?;
    let control = protection_control::create(secret, digest, workspace, creator, [0; 32], &key)
        .map_err(fault!(storage_error()))?;
    let parsed = protection_control::decode(&control).map_err(fault!(storage_error()))?;
    protection_control::insert(
        conn,
        digest,
        RecordId::digest(&control).as_bytes(),
        "create",
        &parsed,
        &control,
    )?;
    Ok(((journal, key, digest), true))
}

/// Only a current local creator's subscription can initialize an empty scope.
pub(crate) fn declare_creator_scope(
    conn: &mut rusqlite::Connection,
    secret: &SecretKey,
    workspace: WorkspaceId,
    package: &str,
) -> Result<bool, SynchronizationError> {
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(fault!(storage_error()))?;
    let (_, created) = ensure_scope(&tx, secret, package, workspace)?;
    tx.commit()
        .map_err(|_| SynchronizationError::Storage(StorageError::UncertainOutcome))?;
    Ok(created)
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
        .map_err(fault!(storage_error()))?
        .ok_or(SynchronizationError::WorkspaceNotFound)
        .and_then(ContentKey::from_column)?;
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
        .map_err(fault!(storage_error()))?;
    if let Some(existing) = existing
        && existing.starts_with(protection_control::DOMAIN_V2)
    {
        return Ok(existing);
    }
    let bytes = protection_control::grant(secret, scope, workspace, recipient, admission, &key)
        .map_err(storage_or_corrupt)?;
    let control =
        protection_control::decode(&bytes).map_err(|_| SynchronizationError::CorruptRecord)?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(fault!(storage_error()))?;
    protection_control::insert(
        &tx,
        scope,
        RecordId::digest(&bytes).as_bytes(),
        "grant",
        &control,
        &bytes,
    )
    .map_err(fault!(storage_error()))?;
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

/// Re-seals one of this Installation's own records into another scope.
///
/// Every signature is genuine and the operation is well formed: only the
/// record's own signed Package and Workspace binding (SEC-05) refuses it.
#[cfg(test)]
pub(crate) fn forge_cross_scope_operation_for_test(
    conn: &rusqlite::Connection,
    secret: &SecretKey,
    workspace: WorkspaceId,
    from_package: &str,
    to_package: &str,
    record_id: RecordId,
) -> Operation<()> {
    let from_scope = scope_digest(from_package, workspace);
    let to_scope = scope_digest(to_package, workspace);
    let sealed: Vec<u8> = conn
        .query_row(
            "SELECT sealed_record FROM immutable_records WHERE scope_id=?1 AND record_id=?2",
            rusqlite::params![from_scope.as_slice(), record_id.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .expect("a record in the source scope");
    let content_key = |scope: [u8; 32]| -> ContentKey {
        conn.query_row(
            "SELECT content_key FROM sync_scopes WHERE scope_id=?1",
            [scope.as_slice()],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .ok()
        .and_then(|bytes| ContentKey::from_column(bytes).ok())
        .expect("a held content key")
    };
    let signed = P2pandaRecordProtection {
        package_identity: from_package.to_owned(),
        workspace_id: workspace,
        content_key: content_key(from_scope),
    }
    .open(&SealedRecordV1::decode(&sealed).expect("a sealed record"))
    .expect("the source record opens in its own scope");
    let (sequence, backlink): (i64, Option<Vec<u8>>) = {
        let next: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(author_sequence),-1)+1 FROM p2panda_operations
                 WHERE scope_id=?1 AND author_public_identity=?2",
                rusqlite::params![to_scope.as_slice(), secret.public().as_bytes().as_slice()],
                |row| row.get(0),
            )
            .expect("an author sequence");
        let backlink = if next == 0 {
            None
        } else {
            conn.query_row(
                "SELECT operation_id FROM p2panda_operations
                 WHERE scope_id=?1 AND author_public_identity=?2 AND author_sequence=?3",
                rusqlite::params![
                    to_scope.as_slice(),
                    secret.public().as_bytes().as_slice(),
                    next - 1
                ],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .ok()
        };
        (next, backlink)
    };
    let payload = P2pandaRecordProtection {
        package_identity: to_package.to_owned(),
        workspace_id: workspace,
        content_key: content_key(to_scope),
    }
    .seal(
        &signed,
        OrderKey::from_height_and_id(
            u64::try_from(sequence).expect("a sequence height"),
            RecordId::digest(&signed.canonical_bytes()),
        ),
    )
    .expect("the record re-seals under the target scope")
    .encode();
    let header = p2panda_core::Header::<()>::builder()
        .body(payload.as_slice())
        .seq_num(u32::try_from(sequence).expect("a p2panda sequence"))
        .backlink(backlink.map(|bytes| {
            Hash::from_bytes(bytes.try_into().expect("a stored operation identifier"))
        }))
        .build(
            &p2panda_core::SigningKey::from_bytes(&secret.to_bytes()),
            (),
        );
    Operation::from_parts(header, Some(p2panda_core::Body::from_bytes(&payload)))
}

/// Seals a new record for this Installation under an order key the test picks.
///
/// Every signature is genuine and the P2Panda sequence and backlink are
/// correct, so only the order key's own rules refuse it. `order_key` receives
/// the record's identity, so a test can bind the key correctly and still choose
/// an impossible height.
#[cfg(test)]
pub(crate) fn forge_order_key_operation_for_test(
    conn: &rusqlite::Connection,
    secret: &SecretKey,
    workspace: WorkspaceId,
    package: &str,
    canonical: &[u8],
    order_key: impl FnOnce(RecordId) -> [u8; 41],
) -> Operation<()> {
    let scope = scope_digest(package, workspace);
    let signed = SignedRecordV1::sign(secret, package, workspace, "message.post", canonical)
        .expect("a signable test record");
    let record_id = RecordId::digest(&signed.canonical_bytes());
    let content_key = conn
        .query_row(
            "SELECT content_key FROM sync_scopes WHERE scope_id=?1",
            [scope.as_slice()],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .ok()
        .and_then(|bytes| ContentKey::from_column(bytes).ok())
        .expect("a held content key");
    let sequence: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(author_sequence),-1)+1 FROM p2panda_operations
             WHERE scope_id=?1 AND author_public_identity=?2",
            rusqlite::params![scope.as_slice(), secret.public().as_bytes().as_slice()],
            |row| row.get(0),
        )
        .expect("an author sequence");
    let backlink: Option<Vec<u8>> = if sequence == 0 {
        None
    } else {
        conn.query_row(
            "SELECT operation_id FROM p2panda_operations
             WHERE scope_id=?1 AND author_public_identity=?2 AND author_sequence=?3",
            rusqlite::params![
                scope.as_slice(),
                secret.public().as_bytes().as_slice(),
                sequence - 1
            ],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .ok()
    };
    let payload = P2pandaRecordProtection {
        package_identity: package.to_owned(),
        workspace_id: workspace,
        content_key,
    }
    .seal(&signed, OrderKey(order_key(record_id)))
    .expect("the record seals under its own scope")
    .encode();
    let header = p2panda_core::Header::<()>::builder()
        .body(payload.as_slice())
        .seq_num(u32::try_from(sequence).expect("a p2panda sequence"))
        .backlink(backlink.map(|bytes| {
            Hash::from_bytes(bytes.try_into().expect("a stored operation identifier"))
        }))
        .build(
            &p2panda_core::SigningKey::from_bytes(&secret.to_bytes()),
            (),
        );
    Operation::from_parts(header, Some(p2panda_core::Body::from_bytes(&payload)))
}

/// Mints a self-authored origin and grant for a key this Installation invented.
///
/// This is the forgery a non-creator Member could attempt: every signature is
/// genuine, so only the creator-origin rule refuses it.
#[cfg(test)]
pub(crate) fn forge_protection_bundle_for_test(
    secret: &SecretKey,
    workspace: WorkspaceId,
    package: &str,
    recipient: PublicIdentity,
) -> (Vec<u8>, Vec<u8>) {
    let scope = scope_digest(package, workspace);
    let author = PublicIdentity::from_bytes(*secret.public().as_bytes()).expect("test identity");
    let key = ContentKey::generate().expect("test randomness");
    let origin = protection_control::create(secret, scope, workspace, author, [0; 32], &key)
        .expect("test origin");
    let grant = protection_control::grant(secret, scope, workspace, recipient, [5; 32], &key)
        .expect("test grant");
    (origin, grant)
}

/// Imports a content-key grant that traces to the Workspace creator's origin.
///
/// `origin` is the creator's own `create` control for the scope. Any current
/// Member holding the key may author the grant, but a scope's first key is
/// accepted only with the creator's signed origin (ADR 0008), and a `create`
/// control is never accepted as a grant.
pub(crate) fn import_protection_grant(
    conn: &mut rusqlite::Connection,
    local_secret: &SecretKey,
    workspace: WorkspaceId,
    package: &str,
    creator: PublicIdentity,
    origin_bytes: &[u8],
    bytes: &[u8],
) -> Result<usize, SynchronizationError> {
    let local_identity = PublicIdentity::from_bytes(*local_secret.public().as_bytes())
        .map_err(|_| SynchronizationError::CorruptRecord)?;
    let control = protection_control::decode(bytes).map_err(storage_or_corrupt)?;
    let scope = scope_digest(package, workspace);
    if control.scope != scope
        || control.workspace != workspace
        || control.recipient != local_identity
        || control.kind != protection_control::KIND_GRANT
    {
        return Err(SynchronizationError::WrongScope);
    }
    let origin = protection_control::decode(origin_bytes).map_err(storage_or_corrupt)?;
    protection_control::verify_origin(&origin, &control, creator)
        .map_err(|_| SynchronizationError::KeyOriginRejected)?;
    let key = protection_control::open_control(
        &control,
        &bytes[..protection_control::AAD_END],
        local_secret,
    )
    .map_err(storage_or_corrupt)?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(fault!(storage_error()))?;
    let existing: Option<Vec<u8>> = tx
        .query_row(
            "SELECT content_key_id FROM sync_scopes WHERE scope_id=?1",
            [scope.as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(fault!(storage_error()))?;
    if let Some(existing) = existing {
        if existing != control.content_key_id {
            return Err(SynchronizationError::CorruptRecord);
        }
    } else {
        let mut journal = [0; 16];
        getrandom::fill(&mut journal).map_err(fault!(storage_error()))?;
        tx.execute(
            "INSERT INTO sync_scopes(scope_id,package_identity,workspace_id,journal_id,content_key_id,content_key) VALUES(?1,?2,?3,?4,?5,?6)",
            rusqlite::params![scope.as_slice(), package, workspace.as_bytes().as_slice(), journal.as_slice(), control.content_key_id.as_slice(), key.expose().as_slice()],
        ).map_err(fault!(storage_error()))?;
        tx.execute(
            "INSERT INTO protection_state(scope_id,content_key_id,creator_public_identity) VALUES(?1,?2,?3)",
            rusqlite::params![scope.as_slice(), control.content_key_id.as_slice(), control.author.as_bytes().as_slice()],
        ).map_err(fault!(storage_error()))?;
        tx.execute(
            "INSERT INTO durable_wakes(scope_id,wake_generation) VALUES(?1,0)",
            [scope.as_slice()],
        )
        .map_err(fault!(storage_error()))?;
    }
    // The importer keeps the creator's origin so it can later prove the key's
    // authority when it grants to someone else.
    let stored_origin: Option<Vec<u8>> = tx
        .query_row(
            "SELECT control_bytes FROM protection_control_operations
             WHERE scope_id=?1 AND operation_kind='create' LIMIT 1",
            [scope.as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(fault!(storage_error()))?;
    match stored_origin {
        Some(stored) if stored != origin_bytes => return Err(SynchronizationError::CorruptRecord),
        Some(_) => {}
        None => protection_control::insert(
            &tx,
            scope,
            RecordId::digest(origin_bytes).as_bytes(),
            "create",
            &origin,
            origin_bytes,
        )
        .map_err(fault!(storage_error()))?,
    }
    protection_control::insert(
        &tx,
        scope,
        RecordId::digest(bytes).as_bytes(),
        "grant",
        &control,
        bytes,
    )
    .map_err(fault!(storage_error()))?;
    tx.commit()
        .map_err(|_| SynchronizationError::Storage(StorageError::UncertainOutcome))?;
    // Records that arrived before the key are parked, not pending: plan phase 3
    // Step 7 promotes them from `parked_operations` after a successful import.
    Ok(0)
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

/// What the local Installation's membership says about an append.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MembershipStanding {
    Current,
    /// Known to this Workspace's membership history, but not a current Member.
    Revoked,
    None,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn append(
    conn: &mut rusqlite::Connection,
    secret: &SecretKey,
    workspace: WorkspaceId,
    package: &str,
    purpose: &str,
    canonical: &[u8],
    standing: MembershipStanding,
    is_creator: bool,
    now_unix_ms: i64,
) -> Result<(ImmutableRecord, i64), SynchronizationError> {
    match standing {
        MembershipStanding::Current => {}
        // An Installation that was a Member and is not one now was revoked;
        // one that was never a Member is simply not in this Workspace.
        MembershipStanding::Revoked => return Err(SynchronizationError::RevokedMember),
        MembershipStanding::None => return Err(SynchronizationError::NotCurrentMember),
    }
    if quarantine::is_scope_quarantined(conn, scope_digest(package, workspace))? {
        return Err(SynchronizationError::ScopeQuarantined);
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
        .map_err(fault!(storage_error()))?
        .is_some();
    if !scope_exists && !is_creator {
        return Err(SynchronizationError::ContentKeyUnavailable);
    }
    let canonical_signed = signed.canonical_bytes();
    let record_id = RecordId::digest(&canonical_signed);
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(fault!(storage_error()))?;
    let ((journal, key, scope), _) = ensure_scope(&tx, secret, package, workspace)?;
    let protection = P2pandaRecordProtection {
        package_identity: package.to_owned(),
        workspace_id: workspace,
        content_key: key.clone(),
    };
    let existing: Option<StoredRecordFields> = tx.query_row("SELECT o.record_id,o.order_key,o.author_public_identity,r.purpose,r.signature,r.sealed_record FROM immutable_operations o JOIN immutable_records r ON r.record_id=o.record_id JOIN immutable_idempotency i ON i.record_id=o.record_id WHERE i.scope_id=?1 AND i.record_id=?2", rusqlite::params![scope.as_slice(), record_id.as_bytes().as_slice()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional().map_err(fault!(storage_error()))?;
    if let Some((id, order, author, p, sig, sealed)) = existing {
        let opened = protection
            .open(
                &SealedRecordV1::decode(&sealed)
                    .map_err(|_| SynchronizationError::CorruptRecord)?,
            )
            .map_err(|_| SynchronizationError::CorruptRecord)?;
        let delivery: i64 = tx
            .query_row(
                "SELECT delivery_sequence FROM record_delivery WHERE record_id=?1",
                [id.as_slice()],
                |row| row.get(0),
            )
            .map_err(fault!(storage_error()))?;
        let record = record_from_row((&id, &order, &author, &p, &sig, &opened.canonical, &sealed))?;
        return Ok((record, delivery));
    }
    let max_height: Option<i64> = tx.query_row("SELECT MAX(causal_height) FROM immutable_operations WHERE scope_id=?1 AND status='accepted'", [scope.as_slice()], |r| r.get(0)).map_err(fault!(storage_error()))?;
    let height = match max_height {
        Some(max) => u64::try_from(max)
            .ok()
            .and_then(|max| max.checked_add(1))
            .ok_or(SynchronizationError::CorruptRecord)?,
        None => 0,
    };
    let order = OrderKey::from_height_and_id(height, record_id);
    let sealed = protection
        .seal(&signed, order)
        .map_err(|_| SynchronizationError::CorruptRecord)?;
    let sealed_bytes = sealed.encode();
    let operation_id = RecordId::digest(&sealed_bytes);
    let sequence: i64 = tx.query_row("SELECT COALESCE(MAX(author_sequence),-1)+1 FROM immutable_operations WHERE scope_id=?1 AND author_public_identity=?2", rusqlite::params![scope.as_slice(), signed.author.as_bytes().as_slice()], |r| r.get(0)).map_err(fault!(storage_error()))?;
    let operation_id_bytes = operation_id.as_bytes();
    let record_id_bytes = record_id.as_bytes();
    let order_bytes = order.as_bytes();
    let author_bytes = signed.author.as_bytes();
    let signature_bytes = signed.signature.to_bytes();
    let row = journal_sql::AcceptedOperation {
        operation_id: operation_id_bytes.as_slice(),
        scope: scope.as_slice(),
        record_id: record_id_bytes.as_slice(),
        order_key: order_bytes.as_slice(),
        author: author_bytes.as_slice(),
        sequence,
        operation_bytes: sealed_bytes.as_slice(),
        parents: &[],
        causal_height: height,
        purpose,
        canonical,
        signature: signature_bytes.as_slice(),
        sealed_record: sealed_bytes.as_slice(),
        journal: journal.as_slice(),
        created_at_unix_ms: now_unix_ms,
    };
    journal_sql::insert_operation(&tx, &row).map_err(fault!(storage_error()))?;
    operation_store::insert(
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
        key.id(),
    )
    .map_err(fault!(storage_error()))?;
    let delivery_sequence =
        journal_sql::insert_record_rows(&tx, &row).map_err(fault!(storage_error()))?;
    tx.commit()
        .map_err(|_| SynchronizationError::Storage(StorageError::UncertainOutcome))?;
    let record = record_from_row((
        record_id.as_bytes().as_slice(),
        order.as_bytes().as_slice(),
        signed.author.as_bytes(),
        purpose,
        signed.signature.to_bytes().as_slice(),
        canonical,
        sealed_bytes.as_slice(),
    ))?;
    Ok((record, delivery_sequence))
}

pub(crate) fn open_records(
    conn: &rusqlite::Connection,
    workspace: WorkspaceId,
    package: &str,
    envelopes: &[Vec<u8>],
    now_unix_ms: i64,
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
        .map_err(fault!(storage_error()))?
        .ok_or(SynchronizationError::WorkspaceNotFound)?;
    let key = ContentKey::from_column(key)?;
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
            let record = match protector.open(&sealed) {
                Ok(record) => record,
                Err(error) => {
                    // An envelope that names a record this Installation stores
                    // and no longer opens is a local integrity failure, so the
                    // record is isolated before it is read again.
                    note_unopenable_record(conn, scope, sealed.record_id, now_unix_ms)?;
                    return Err(map_open_error(&error));
                }
            };
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

/// Isolates a stored record that no longer opens under its scope's key.
///
/// An envelope that this Installation does not store is a caller's value, not a
/// local fault, so nothing is recorded for it.
fn note_unopenable_record(
    conn: &rusqlite::Connection,
    scope: [u8; 32],
    record_id: RecordId,
    now_unix_ms: i64,
) -> Result<(), SynchronizationError> {
    let stored = conn
        .query_row(
            "SELECT 1 FROM immutable_records WHERE record_id=?1 AND scope_id=?2",
            rusqlite::params![record_id.as_bytes().as_slice(), scope.as_slice()],
            |_| Ok(()),
        )
        .optional()
        .map_err(fault!(storage_error()))?
        .is_some();
    if stored {
        quarantine::quarantine_record(
            conn,
            scope,
            record_id.as_bytes(),
            quarantine::RecordFault::Open,
            now_unix_ms,
        )?;
    }
    Ok(())
}

/// A row insert that violates a constraint is one peer's conflicting history,
/// not a local fault; anything else from SQLite is a storage failure.
fn insert_error(error: rusqlite::Error) -> RemoteInsertError {
    match error {
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            RemoteInsertError::Conflict
        }
        _ => RemoteInsertError::Storage,
    }
}

/// Whether a remote operation was newly applied or already stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RemoteInsert {
    Applied {
        record: ImmutableRecord,
        delivery_sequence: i64,
    },
    Duplicate(ImmutableRecord),
}

/// Why a remote operation was not applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteInsertError {
    MissingScope,
    KeyMismatch,
    WrongScope,
    Invalid,
    Conflict,
    /// The order key claims a causal height too far beyond this scope's own.
    HeightOutOfRange,
    Storage,
}

/// Applies one authenticated remote operation, classifying every refusal.
///
/// A caller decides what each refusal means for the session: only
/// [`RemoteInsertError::Storage`] is fatal, and `MissingScope` means the
/// operation should be parked until the scope's content key arrives.
pub(crate) fn insert_remote_operation(
    conn: &mut rusqlite::Connection,
    workspace: WorkspaceId,
    package: &str,
    operation: &Operation<()>,
    now_unix_ms: i64,
) -> Result<RemoteInsert, RemoteInsertError> {
    // Validate the complete P2Panda operation before touching the Teamarness
    // journal. In particular, an occupied sequence or backlink conflict must
    // not leave behind a sealed record that recovery cannot project.
    p2panda_core::operation::validate_operation(operation)
        .map_err(|_| RemoteInsertError::Invalid)?;
    let payload = operation
        .body
        .as_ref()
        .ok_or(RemoteInsertError::Invalid)?
        .to_bytes();
    let scope = scope_digest(package, workspace);
    let sealed = SealedRecordV1::decode(&payload).map_err(|_| RemoteInsertError::Invalid)?;
    if sealed.scope_digest != scope {
        return Err(RemoteInsertError::WrongScope);
    }
    let (journal, key, content_key_id): ([u8; 16], ContentKey, [u8; 32]) = conn
        .query_row(
            "SELECT journal_id,content_key,content_key_id FROM sync_scopes WHERE scope_id=?1",
            [scope.as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    ContentKey::from_column(row.get::<_, Vec<u8>>(1)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    row.get::<_, Vec<u8>>(2)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                ))
            },
        )
        .optional()
        .map_err(|_| RemoteInsertError::Storage)?
        .ok_or(RemoteInsertError::MissingScope)?;
    if sealed.content_key_id != content_key_id {
        return Err(RemoteInsertError::KeyMismatch);
    }
    let protector = P2pandaRecordProtection {
        package_identity: package.to_owned(),
        workspace_id: workspace,
        content_key: key,
    };
    let signed = protector.open(&sealed).map_err(|error| {
        if matches!(map_open_error(&error), SynchronizationError::WrongScope) {
            RemoteInsertError::WrongScope
        } else {
            RemoteInsertError::Invalid
        }
    })?;
    if operation.header.verifying_key.as_bytes() != signed.author.as_bytes() {
        return Err(RemoteInsertError::Invalid);
    }
    let record_id = RecordId::digest(&signed.canonical_bytes());
    if record_id != sealed.record_id {
        return Err(RemoteInsertError::Invalid);
    }
    let teamarness_operation_id = RecordId::digest(&payload);
    let transaction = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| RemoteInsertError::Storage)?;
    let existing: Option<Vec<u8>> = transaction
        .query_row(
            "SELECT teamarness_operation_id FROM p2panda_operations WHERE operation_id=?1",
            [operation.hash.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| RemoteInsertError::Storage)?;
    if let Some(existing) = existing {
        if existing != teamarness_operation_id.as_bytes() {
            return Err(RemoteInsertError::Invalid);
        }
        transaction
            .commit()
            .map_err(|_| RemoteInsertError::Storage)?;
        return Ok(RemoteInsert::Duplicate(
            record_from_row((
                record_id.as_bytes().as_slice(),
                sealed.order_key.as_bytes().as_slice(),
                signed.author.as_bytes(),
                &signed.purpose,
                signed.signature.to_bytes().as_slice(),
                &signed.canonical,
                &payload,
            ))
            .map_err(|_| RemoteInsertError::Invalid)?,
        ));
    }
    let expected = transaction
        .query_row(
            "SELECT COALESCE(MAX(author_sequence),-1)+1 FROM p2panda_operations
             WHERE scope_id=?1 AND author_public_identity=?2",
            rusqlite::params![scope.as_slice(), operation.header.verifying_key.as_bytes()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|_| RemoteInsertError::Storage)?;
    if i64::from(operation.header.seq_num) != expected {
        return Err(RemoteInsertError::Conflict);
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
            .map_err(|_| RemoteInsertError::Storage)?
    };
    let actual_backlink = operation
        .header
        .backlink
        .map(|value| value.as_bytes().to_vec());
    if expected_backlink != actual_backlink {
        return Err(RemoteInsertError::Conflict);
    }
    // A height far beyond this scope's own is a claim nothing local can check,
    // and it would push every later record behind it.
    let local_max: Option<i64> = transaction
        .query_row(
            "SELECT MAX(causal_height) FROM immutable_operations
             WHERE scope_id=?1 AND status='accepted'",
            [scope.as_slice()],
            |row| row.get(0),
        )
        .map_err(|_| RemoteInsertError::Storage)?;
    let ceiling = local_max
        .and_then(|height| u64::try_from(height).ok())
        .unwrap_or(0)
        .saturating_add(limits::MAX_HEIGHT_ADVANCE);
    if sealed.order_key.height() > ceiling {
        return Err(RemoteInsertError::HeightOutOfRange);
    }
    // All Teamarness rows and the P2Panda projection share this transaction.
    // An invalid projection therefore rolls back the accepted record too.
    let operation_id_bytes = teamarness_operation_id.as_bytes();
    let record_id_bytes = record_id.as_bytes();
    let order_bytes = sealed.order_key.as_bytes();
    let author_bytes = signed.author.as_bytes();
    let signature_bytes = signed.signature.to_bytes();
    let row = journal_sql::AcceptedOperation {
        operation_id: operation_id_bytes.as_slice(),
        scope: scope.as_slice(),
        record_id: record_id_bytes.as_slice(),
        order_key: order_bytes.as_slice(),
        author: author_bytes.as_slice(),
        sequence: i64::from(operation.header.seq_num),
        operation_bytes: payload.as_slice(),
        parents: &[],
        causal_height: sealed.order_key.height(),
        purpose: &signed.purpose,
        canonical: &signed.canonical,
        signature: signature_bytes.as_slice(),
        sealed_record: payload.as_slice(),
        journal: journal.as_slice(),
        created_at_unix_ms: now_unix_ms,
    };
    journal_sql::insert_operation(&transaction, &row).map_err(insert_error)?;
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
    .map_err(|_| RemoteInsertError::Conflict)?;
    let delivery_sequence =
        journal_sql::insert_record_rows(&transaction, &row).map_err(insert_error)?;
    transaction
        .commit()
        .map_err(|_| RemoteInsertError::Storage)?;
    Ok(RemoteInsert::Applied {
        record: record_from_row((
            record_id.as_bytes().as_slice(),
            sealed.order_key.as_bytes().as_slice(),
            signed.author.as_bytes(),
            &signed.purpose,
            signed.signature.to_bytes().as_slice(),
            &signed.canonical,
            &payload,
        ))
        .map_err(|_| RemoteInsertError::Invalid)?,
        delivery_sequence,
    })
}

/// Refuses a data directory that still holds retired version-1 protection
/// controls, and proves that every control addressed to this Installation still
/// opens to the content key its scope records.
fn validate_protection_controls(
    conn: &rusqlite::Connection,
    secret: &SecretKey,
) -> Result<(), StorageError> {
    let local = PublicIdentity::from_bytes(*secret.public().as_bytes())
        .map_err(|_| StorageError::CorruptIdentity)?;
    let controls: Vec<(Vec<u8>, Vec<u8>, Vec<u8>)> = {
        let mut statement = conn
            .prepare(
                "SELECT scope_id,recipient_public_identity,control_bytes
                 FROM protection_control_operations",
            )
            .map_err(fault!(StorageError::Database))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(fault!(StorageError::Database))?;
        rows.collect::<Result<_, _>>()
            .map_err(|_| StorageError::CorruptWorkspace)?
    };
    for (scope, recipient, bytes) in controls {
        let control = protection_control::decode(&bytes)?;
        if recipient != control.recipient.as_bytes().as_slice() {
            return Err(StorageError::CorruptWorkspace);
        }
        if control.recipient != local {
            continue;
        }
        let scope_id: [u8; 32] = scope
            .clone()
            .try_into()
            .map_err(|_| StorageError::CorruptWorkspace)?;
        // A scope that restart already isolated is not held against the
        // Installation a second time.
        if quarantine::is_scope_quarantined(conn, scope_id)
            .map_err(fault!(StorageError::Database))?
        {
            continue;
        }
        let key = protection_control::open_control(
            &control,
            &bytes[..protection_control::AAD_END],
            secret,
        )?;
        let scope_key_id: Option<Vec<u8>> = conn
            .query_row(
                "SELECT content_key_id FROM sync_scopes WHERE scope_id=?1",
                [scope.as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(fault!(StorageError::Database))?;
        let Some(scope_key_id) = scope_key_id else {
            return Err(StorageError::CorruptWorkspace);
        };
        if scope_key_id != key.id().as_slice() {
            return Err(StorageError::CorruptWorkspace);
        }
    }
    Ok(())
}

pub(crate) fn recover(
    conn: &mut rusqlite::Connection,
    secret: &SecretKey,
    now_unix_ms: i64,
) -> Result<RecoveryReport, StorageError> {
    let scopes: Vec<ScopeRecoveryRow> = {
        let mut stmt = conn.prepare("SELECT scope_id,package_identity,workspace_id,content_key,content_key_id FROM sync_scopes").map_err(fault!(StorageError::Database))?;
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
            .map_err(fault!(StorageError::Database))?;
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
                ContentKey::from_column(key).map_err(|_| StorageError::CorruptWorkspace)?,
                id.try_into().map_err(|_| StorageError::CorruptWorkspace)?,
            ))
        })
        .collect::<Result<_, StorageError>>()?
    };
    operation_store::repair(conn, secret)?;
    let mut report = RecoveryReport::default();
    // Scope identity comes first: a scope whose binding or key identity is
    // broken is isolated before anything that reads it runs.
    let mut sound: Vec<([u8; 32], String, WorkspaceId, ContentKey)> =
        Vec::with_capacity(scopes.len());
    for (scope, package, workspace, key, id) in scopes {
        report.scopes_checked += 1;
        if quarantine::is_scope_quarantined(conn, scope).map_err(fault!(StorageError::Database))? {
            report.scopes_quarantined += 1;
            continue;
        }
        let fault = if scope_digest(&package, workspace) != scope {
            Some(quarantine::ScopeFault::ScopeBinding)
        } else if key.id() != id {
            Some(quarantine::ScopeFault::ContentKeyId)
        } else {
            None
        };
        if let Some(fault) = fault {
            quarantine::quarantine_scope(conn, scope, fault, now_unix_ms)
                .map_err(fault!(StorageError::Database))?;
            report.scopes_quarantined += 1;
            continue;
        }
        sound.push((scope, package, workspace, key));
    }
    validate_protection_controls(conn, secret)?;
    for (scope, package, workspace, key) in sound {
        validate_scope_records(
            conn,
            scope,
            &package,
            workspace,
            key,
            now_unix_ms,
            &mut report,
        )?;
    }
    Ok(report)
}

/// Validates the records of one scope committed since its last validated start
/// and moves the scope's validation mark forward.
///
/// A projection fault isolates the scope and stops it here; a decode, order-key
/// or open fault isolates that one record and validation continues.
fn validate_scope_records(
    conn: &mut rusqlite::Connection,
    scope: [u8; 32],
    package: &str,
    workspace: WorkspaceId,
    key: ContentKey,
    now_unix_ms: i64,
    report: &mut RecoveryReport,
) -> Result<(), StorageError> {
    let mark: i64 = conn
        .query_row(
            "SELECT validated_through_sequence FROM scope_validation WHERE scope_id=?1",
            [scope.as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(fault!(StorageError::Database))?
        .unwrap_or(0);
    type PendingRow = (i64, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>);
    let pending: Vec<PendingRow> = {
        let mut stmt = conn
            .prepare(
                "SELECT d.delivery_sequence,r.record_id,o.order_key,r.sealed_record,o.operation_id
                 FROM record_delivery d
                 JOIN immutable_records r ON r.record_id=d.record_id
                 JOIN immutable_operations o ON o.record_id=r.record_id
                 WHERE d.scope_id=?1 AND d.delivery_sequence>?2
                 ORDER BY d.delivery_sequence",
            )
            .map_err(fault!(StorageError::Database))?;
        let rows = stmt
            .query_map(rusqlite::params![scope.as_slice(), mark], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                    r.get::<_, Vec<u8>>(3)?,
                    r.get::<_, Vec<u8>>(4)?,
                ))
            })
            .map_err(fault!(StorageError::Database))?;
        rows.collect::<Result<_, _>>()
            .map_err(fault!(StorageError::Database))?
    };
    if pending.is_empty() {
        return Ok(());
    }
    let protector = P2pandaRecordProtection {
        package_identity: package.to_owned(),
        workspace_id: workspace,
        content_key: key,
    };
    let transaction = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(fault!(StorageError::Database))?;
    let mut scope_isolated = false;
    let mut validated_through = mark;
    for (sequence, record_id, order, bytes, operation_id) in pending {
        if operation_store::validate_projection_row(&transaction, &operation_id).is_err() {
            quarantine::quarantine_scope(
                &transaction,
                scope,
                quarantine::ScopeFault::Projection,
                now_unix_ms,
            )
            .map_err(fault!(StorageError::Database))?;
            report.scopes_quarantined += 1;
            scope_isolated = true;
            break;
        }
        let expected_id: [u8; 32] = record_id
            .try_into()
            .map_err(|_| StorageError::CorruptWorkspace)?;
        let expected_order: [u8; 41] = order
            .try_into()
            .map_err(|_| StorageError::CorruptWorkspace)?;
        let fault = match SealedRecordV1::decode(&bytes) {
            Err(error) if error.value_name() == "order key binding" => {
                Some(quarantine::RecordFault::OrderKey)
            }
            Err(_) => Some(quarantine::RecordFault::Decode),
            Ok(sealed) => {
                if sealed.scope_digest != scope
                    || sealed.record_id.as_bytes() != expected_id
                    || sealed.order_key.as_bytes() != expected_order
                {
                    Some(quarantine::RecordFault::Decode)
                } else if protector.open(&sealed).is_err() {
                    Some(quarantine::RecordFault::Open)
                } else {
                    None
                }
            }
        };
        if let Some(fault) = fault {
            quarantine::quarantine_record(&transaction, scope, expected_id, fault, now_unix_ms)
                .map_err(fault!(StorageError::Database))?;
            report.records_quarantined += 1;
        }
        report.records_validated += 1;
        validated_through = sequence;
    }
    if !scope_isolated {
        transaction
            .execute(
                "INSERT INTO scope_validation(scope_id,validated_through_sequence) VALUES(?1,?2)
                 ON CONFLICT(scope_id) DO UPDATE SET validated_through_sequence=excluded.validated_through_sequence",
                rusqlite::params![scope.as_slice(), validated_through],
            )
            .map_err(fault!(StorageError::Database))?;
    }
    transaction
        .commit()
        .map_err(fault!(StorageError::UncertainOutcome))?;
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
        fn decode_rejects_order_key_with_wrong_version() {
            let record = signed();
            let id = RecordId::digest(&record.canonical_bytes());
            let mut order = OrderKey::from_height_and_id(3, id).as_bytes();
            order[0] = 2;
            let sealed = SealedRecordV1 {
                scope_digest: [1; 32],
                record_id: id,
                order_key: OrderKey(order),
                content_key_id: [2; 32],
                nonce: [3; 24],
                ciphertext: vec![4],
            };

            let error = SealedRecordV1::decode(&sealed.encode())
                .expect_err("an order key of another version must not decode");

            assert_eq!(error.to_string(), "invalid order key binding");
        }

        #[test]
        fn decode_rejects_order_key_embedding_another_record_id() {
            let record = signed();
            let id = RecordId::digest(&record.canonical_bytes());
            let sealed = SealedRecordV1 {
                scope_digest: [1; 32],
                record_id: id,
                order_key: OrderKey::from_height_and_id(3, RecordId([9; 32])),
                content_key_id: [2; 32],
                nonce: [3; 24],
                ciphertext: vec![4],
            };

            let error = SealedRecordV1::decode(&sealed.encode())
                .expect_err("an order key naming another record must not decode");

            assert_eq!(error.to_string(), "invalid order key binding");
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
                content_key: ContentKey::from_column(vec![9; 32]).unwrap(),
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

        fn seal_into(
            record: &SignedRecordV1,
            package_identity: &str,
            workspace_id: WorkspaceId,
        ) -> (P2pandaRecordProtection, SealedRecordV1) {
            let protector = P2pandaRecordProtection {
                package_identity: package_identity.to_owned(),
                workspace_id,
                content_key: ContentKey::from_column(vec![9; 32]).unwrap(),
            };
            let id = RecordId::digest(&record.canonical_bytes());
            let sealed = protector
                .seal(record, OrderKey::from_height_and_id(2, id))
                .unwrap();
            (protector, sealed)
        }

        // Verifies: TEAM.SYS.TRANS-R06
        #[test]
        fn open_rejects_record_signed_for_another_package() {
            let record = signed();
            let (protector, sealed) = seal_into(&record, "other.package", record.workspace_id);

            let Err(error) = protector.open(&sealed) else {
                panic!("a record signed for another scope must not open");
            };

            assert_eq!(error.to_string(), "invalid record scope binding");
            assert_eq!(map_open_error(&error), SynchronizationError::WrongScope);
        }

        // Verifies: TEAM.SYS.TRANS-R06
        #[test]
        fn open_rejects_record_signed_for_another_workspace() {
            let record = signed();
            let (protector, sealed) = seal_into(
                &record,
                &record.package_identity,
                WorkspaceId::from_bytes([4; 16]),
            );

            let Err(error) = protector.open(&sealed) else {
                panic!("a record signed for another scope must not open");
            };

            assert_eq!(error.to_string(), "invalid record scope binding");
            assert_eq!(map_open_error(&error), SynchronizationError::WrongScope);
        }

        #[test]
        fn open_accepts_record_for_its_own_scope() {
            let record = signed();
            let (protector, sealed) =
                seal_into(&record, &record.package_identity, record.workspace_id);

            let opened = protector.open(&sealed).unwrap();

            assert_eq!(opened.canonical, record.canonical);
            assert_eq!(opened.package_identity, record.package_identity);
            assert_eq!(opened.workspace_id, record.workspace_id);
            assert_eq!(
                map_open_error(&ValueParseError::new("record decryption")),
                SynchronizationError::CorruptRecord
            );
        }

        #[test]
        fn concurrent_holders_keep_one_key_id() {
            let first = P2pandaRecordProtection {
                package_identity: "pkg".into(),
                workspace_id: WorkspaceId::from_bytes([1; 16]),
                content_key: ContentKey::from_column(vec![8; 32]).unwrap(),
            };
            let second = P2pandaRecordProtection {
                content_key: first.content_key.clone(),
                ..first.clone()
            };

            assert_eq!(first.key_id(), second.key_id());
        }
    }
}
