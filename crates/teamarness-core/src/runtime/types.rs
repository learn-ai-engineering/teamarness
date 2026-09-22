//! The public domain values the runtime exchanges with callers.
//!
//! Every one is validated on the way in and prints as a stable string. None
//! exposes a dependency type: the key, signature and identifier bytes behind
//! them stay inside the crate.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use iroh_base::{PublicKey, Signature};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::MembershipOperationId;
use crate::error::{IdentityError, ValueParseError};

/// Configuration for opening a [`CoreRuntime`](super::CoreRuntime).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreRuntimeConfig {
    private_data_directory: PathBuf,
    /// Tests bound a session far below production so a bound is reached in a
    /// few records instead of a few thousand.
    #[cfg(test)]
    pub(in crate::runtime) session_limits: crate::synchronization::limits::SessionLimits,
}

impl CoreRuntimeConfig {
    /// Creates a configuration rooted at the runtime's private data directory.
    #[must_use]
    pub fn new(private_data_directory: impl Into<PathBuf>) -> Self {
        Self {
            private_data_directory: private_data_directory.into(),
            #[cfg(test)]
            session_limits: crate::synchronization::limits::SessionLimits::CURRENT,
        }
    }

    /// Returns the private data directory owned by the runtime.
    #[must_use]
    pub fn private_data_directory(&self) -> &Path {
        &self.private_data_directory
    }
}

/// A validated installation public identity compatible with an Iroh endpoint ID.
///
/// The underlying key bytes are deliberately not exposed by this API.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PublicIdentity(pub(in crate::runtime) PublicKey);

impl fmt::Debug for PublicIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("PublicIdentity")
            .field(&self.to_string())
            .finish()
    }
}

impl fmt::Display for PublicIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for PublicIdentity {
    type Err = ValueParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value
            .parse()
            .map(Self)
            .map_err(|_| ValueParseError::new("public identity"))
    }
}

impl Serialize for PublicIdentity {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for PublicIdentity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

impl PublicIdentity {
    pub(crate) fn from_bytes(bytes: [u8; 32]) -> Result<Self, ValueParseError> {
        PublicKey::from_bytes(&bytes)
            .map(Self)
            .map_err(|_| ValueParseError::new("public identity"))
    }

    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }

    pub(crate) const fn as_public_key(self) -> PublicKey {
        self.0
    }

    /// Verifies a signature using the same versioned, domain-separated envelope as [`CoreRuntime::sign`](super::CoreRuntime::sign).
    pub fn verify(
        self,
        context: &SigningContext,
        payload: &[u8],
        signature: &PublicSignature,
    ) -> Result<(), IdentityError> {
        crate::identity::verify(
            self.0,
            context.as_str(),
            payload,
            &signature.as_iroh_signature(),
        )
    }
}

macro_rules! fixed_hex_value {
    ($name:ident, $bytes:literal, $label:literal, $docs:literal) => {
        #[doc = $docs]
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name([u8; $bytes]);

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_tuple(stringify!($name))
                    .field(&self.to_string())
                    .finish()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                $crate::encoding::write_hex(&self.0, formatter)
            }
        }

        impl FromStr for $name {
            type Err = ValueParseError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                parse_fixed_hex(value, $label).map(Self)
            }
        }
    };
}

fixed_hex_value!(
    WorkspaceId,
    16,
    "workspace ID",
    "The immutable identifier of a workspace."
);

impl WorkspaceId {
    pub(crate) const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub(crate) const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}
fixed_hex_value!(
    MembershipVersion,
    32,
    "membership version",
    "A content-derived identifier for an accepted membership operation set."
);

impl MembershipVersion {
    pub(crate) const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub(crate) const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}
fixed_hex_value!(
    MembershipSnapshotDigest,
    32,
    "membership snapshot digest",
    "A digest of a resolved membership snapshot."
);

impl MembershipSnapshotDigest {
    pub(crate) const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub(crate) const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

fn parse_fixed_hex<const N: usize>(
    value: &str,
    label: &'static str,
) -> Result<[u8; N], ValueParseError> {
    crate::encoding::hex_decode_fixed(value).ok_or_else(|| ValueParseError::new(label))
}

/// A workspace-local durable cursor for committed membership transitions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MembershipEpoch(u64);

impl MembershipEpoch {
    /// Creates an epoch cursor from its persisted numeric value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the numeric cursor value.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

/// The public Phase 1 role of a workspace member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MembershipRole {
    /// A member that can participate but cannot change canonical membership.
    Member,
    /// A member that may issue invitations and author membership changes.
    Manager,
}

/// One member in a resolved membership snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberSnapshot {
    identity: PublicIdentity,
    role: MembershipRole,
}

impl MemberSnapshot {
    pub(crate) const fn new(identity: PublicIdentity, role: MembershipRole) -> Self {
        Self { identity, role }
    }

    /// Returns the member's installation identity.
    #[must_use]
    pub const fn identity(&self) -> PublicIdentity {
        self.identity
    }

    /// Returns the member's semantic Phase 1 role.
    #[must_use]
    pub const fn role(&self) -> MembershipRole {
        self.role
    }
}

/// Public metadata for a workspace known to this installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSummary {
    id: WorkspaceId,
    name: String,
    created_by: PublicIdentity,
    created_at_unix_ms: i64,
}

impl WorkspaceSummary {
    pub(crate) fn new(
        id: WorkspaceId,
        name: String,
        created_by: PublicIdentity,
        created_at_unix_ms: i64,
    ) -> Self {
        Self {
            id,
            name,
            created_by,
            created_at_unix_ms,
        }
    }

    /// Returns the immutable workspace identifier.
    #[must_use]
    pub const fn id(&self) -> WorkspaceId {
        self.id
    }

    /// Returns the validated workspace display name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the installation that created the workspace.
    #[must_use]
    pub const fn created_by(&self) -> PublicIdentity {
        self.created_by
    }

    /// Returns the signed creation time as Unix milliseconds.
    #[must_use]
    pub const fn created_at_unix_ms(&self) -> i64 {
        self.created_at_unix_ms
    }
}

/// The current deterministic projection of canonical workspace membership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipSnapshot {
    workspace_id: WorkspaceId,
    epoch: MembershipEpoch,
    version: MembershipVersion,
    members: Vec<MemberSnapshot>,
}

impl MembershipSnapshot {
    pub(crate) fn new(
        workspace_id: WorkspaceId,
        epoch: MembershipEpoch,
        version: MembershipVersion,
        members: Vec<MemberSnapshot>,
    ) -> Self {
        Self {
            workspace_id,
            epoch,
            version,
            members,
        }
    }

    /// Returns the workspace represented by this snapshot.
    #[must_use]
    pub const fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }

    /// Returns the local transition cursor at which this snapshot was stored.
    #[must_use]
    pub const fn epoch(&self) -> MembershipEpoch {
        self.epoch
    }

    /// Returns the content-derived accepted-set version.
    #[must_use]
    pub const fn version(&self) -> MembershipVersion {
        self.version
    }

    /// Returns members in canonical identity order.
    #[must_use]
    pub fn members(&self) -> &[MemberSnapshot] {
        &self.members
    }
}

/// A validated domain tag for installation signing.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SigningContext(String);

impl SigningContext {
    /// Creates a domain tag of at most 64 bytes.
    ///
    /// Tags start with an ASCII lowercase letter and otherwise contain only
    /// lowercase letters, digits, `.`, `_`, `/`, or `-`.
    pub fn new(domain: impl Into<String>) -> Result<Self, IdentityError> {
        let domain = domain.into();
        crate::identity::validate_signing_context(&domain)?;
        Ok(Self(domain))
    }

    /// Returns the domain tag.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A public Ed25519 signature returned by domain-separated signing.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PublicSignature(pub(in crate::runtime) [u8; 64]);

impl fmt::Debug for PublicSignature {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("PublicSignature")
            .field(&self.to_string())
            .finish()
    }
}

impl fmt::Display for PublicSignature {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        crate::encoding::write_hex(&self.0, formatter)
    }
}

impl FromStr for PublicSignature {
    type Err = ValueParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_fixed_hex(value, "public signature").map(Self)
    }
}

impl PublicSignature {
    pub(crate) const fn from_bytes(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }

    fn as_iroh_signature(&self) -> Signature {
        Signature::from_bytes(&self.0)
    }
}

/// What a membership transition changed.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipChange {
    /// The transition admitted `member` to the workspace.
    Admitted {
        /// The admitted installation.
        member: PublicIdentity,
    },
    /// The transition revoked `member` from the workspace.
    Revoked {
        /// The revoked installation.
        member: PublicIdentity,
    },
}

/// One durable, post-genesis membership transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipTransition {
    workspace_id: WorkspaceId,
    epoch: MembershipEpoch,
    version: MembershipVersion,
    cause: MembershipOperationId,
    snapshot_digest: MembershipSnapshotDigest,
    change: MembershipChange,
    author: PublicIdentity,
}

impl MembershipTransition {
    pub(crate) const fn new(
        workspace_id: WorkspaceId,
        epoch: MembershipEpoch,
        version: MembershipVersion,
        cause: MembershipOperationId,
        snapshot_digest: MembershipSnapshotDigest,
        change: MembershipChange,
        author: PublicIdentity,
    ) -> Self {
        Self {
            workspace_id,
            epoch,
            version,
            cause,
            snapshot_digest,
            change,
            author,
        }
    }

    /// Returns whether this transition admitted or revoked, and whom.
    #[must_use]
    pub const fn change(&self) -> MembershipChange {
        self.change
    }

    /// Returns the installation that authored the causing operation.
    #[must_use]
    pub const fn author(&self) -> PublicIdentity {
        self.author
    }

    /// Returns the workspace whose membership changed.
    #[must_use]
    pub const fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }

    /// Returns the transition's local durable cursor.
    #[must_use]
    pub const fn epoch(&self) -> MembershipEpoch {
        self.epoch
    }

    /// Returns the content-derived accepted-set version after the transition.
    #[must_use]
    pub const fn version(&self) -> MembershipVersion {
        self.version
    }

    /// Returns the accepted operation that caused this transition.
    #[must_use]
    pub const fn cause(&self) -> MembershipOperationId {
        self.cause
    }

    /// Returns the digest of the resulting member snapshot.
    #[must_use]
    pub const fn snapshot_digest(&self) -> MembershipSnapshotDigest {
        self.snapshot_digest
    }
}
