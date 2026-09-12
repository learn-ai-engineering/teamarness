use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use iroh_base::PublicKey;

use crate::error::{
    CoreError, IdentityError, InvitationError, MembershipError, StorageError, SubscriptionError,
    ValueParseError, WorkspaceError,
};

/// Configuration for opening a [`CoreRuntime`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreRuntimeConfig {
    private_data_directory: PathBuf,
}

impl CoreRuntimeConfig {
    /// Creates a configuration rooted at the runtime's private data directory.
    #[must_use]
    pub fn new(private_data_directory: impl Into<PathBuf>) -> Self {
        Self {
            private_data_directory: private_data_directory.into(),
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
pub struct PublicIdentity(PublicKey);

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
                for byte in self.0 {
                    write!(formatter, "{byte:02x}")?;
                }
                Ok(())
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
fixed_hex_value!(
    MembershipOperationId,
    32,
    "membership operation ID",
    "The content-derived identifier of a canonical membership operation."
);
fixed_hex_value!(
    MembershipVersion,
    32,
    "membership version",
    "A content-derived identifier for an accepted membership operation set."
);
fixed_hex_value!(
    MembershipSnapshotDigest,
    32,
    "membership snapshot digest",
    "A digest of a resolved membership snapshot."
);

fn parse_fixed_hex<const N: usize>(
    value: &str,
    label: &'static str,
) -> Result<[u8; N], ValueParseError> {
    if value.len() != N * 2 || !value.is_ascii() {
        return Err(ValueParseError::new(label));
    }

    let mut bytes = [0_u8; N];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let offset = index * 2;
        *byte = u8::from_str_radix(&value[offset..offset + 2], 16)
            .map_err(|_| ValueParseError::new(label))?;
    }
    Ok(bytes)
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
    /// Creates a signing context from a non-empty ASCII domain tag.
    pub fn new(domain: impl Into<String>) -> Result<Self, IdentityError> {
        let domain = domain.into();
        if domain.is_empty() || !domain.is_ascii() {
            return Err(IdentityError::InvalidSigningContext);
        }
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
pub struct PublicSignature([u8; 64]);

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
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl FromStr for PublicSignature {
    type Err = ValueParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_fixed_hex(value, "public signature").map(Self)
    }
}

/// An opaque, transferable signed invitation token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvitationToken(String);

impl InvitationToken {
    /// Returns the token's transport-safe encoded form.
    #[must_use]
    pub fn encoded(&self) -> &str {
        &self.0
    }
}

impl FromStr for InvitationToken {
    type Err = ValueParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() {
            return Err(ValueParseError::new("invitation token"));
        }
        Ok(Self(value.to_owned()))
    }
}

/// An opaque invitation token plus proof of possession by the invitee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvitationClaim(String);

impl InvitationClaim {
    /// Returns the claim's transport-safe encoded form.
    #[must_use]
    pub fn encoded(&self) -> &str {
        &self.0
    }
}

impl FromStr for InvitationClaim {
    type Err = ValueParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() {
            return Err(ValueParseError::new("invitation claim"));
        }
        Ok(Self(value.to_owned()))
    }
}

/// One durable, post-genesis membership transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipTransition {
    workspace_id: WorkspaceId,
    epoch: MembershipEpoch,
    version: MembershipVersion,
    cause: MembershipOperationId,
    snapshot_digest: MembershipSnapshotDigest,
}

impl MembershipTransition {
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

/// A gap-free stream of durable then live membership transitions.
#[derive(Debug)]
pub struct MembershipSubscription {
    state: SubscriptionState,
}

#[derive(Debug)]
enum SubscriptionState {}

impl MembershipSubscription {
    /// Waits for the next transition, returning `None` after orderly shutdown.
    pub fn next_transition(&mut self) -> Result<Option<MembershipTransition>, SubscriptionError> {
        match self.state {}
    }
}

/// The sole public command and query seam for durable core state.
///
/// Phase 1 establishes this contract. Subsequent implementation phases make
/// the operations available without exposing keys, SQLite handles, Iroh
/// transport handles, or mutable p2panda state.
#[derive(Debug)]
pub struct CoreRuntime {
    state: RuntimeState,
}

#[derive(Debug)]
enum RuntimeState {}

impl CoreRuntime {
    /// Opens or initializes the runtime in its private data directory.
    pub fn open(_config: CoreRuntimeConfig) -> Result<Self, CoreError> {
        Err(CoreError::Storage(StorageError::RuntimeUnavailable))
    }

    /// Returns this installation's validated public identity.
    #[must_use]
    pub fn public_identity(&self) -> PublicIdentity {
        match self.state {}
    }

    /// Returns this installation's validated display name.
    #[must_use]
    pub fn display_name(&self) -> &str {
        match self.state {}
    }

    /// Updates this installation's display name.
    pub fn set_display_name(&self, _display_name: &str) -> Result<(), IdentityError> {
        match self.state {}
    }

    /// Signs bytes under a validated, length-delimited domain context.
    pub fn sign(
        &self,
        _context: &SigningContext,
        _payload: &[u8],
    ) -> Result<PublicSignature, IdentityError> {
        match self.state {}
    }

    /// Creates a named workspace with this installation as its first manager.
    pub fn create_workspace(&self, _name: &str) -> Result<WorkspaceSummary, WorkspaceError> {
        match self.state {}
    }

    /// Lists workspaces known to this installation.
    pub fn list_workspaces(&self) -> Result<Vec<WorkspaceSummary>, WorkspaceError> {
        match self.state {}
    }

    /// Issues an expiring invitation as the local manager.
    pub fn issue_invitation(
        &self,
        _workspace_id: WorkspaceId,
        _expires_at_unix_ms: i64,
    ) -> Result<InvitationToken, InvitationError> {
        match self.state {}
    }

    /// Creates proof that this installation claims the supplied invitation.
    pub fn create_invitation_claim(
        &self,
        _token: &InvitationToken,
    ) -> Result<InvitationClaim, InvitationError> {
        match self.state {}
    }

    /// Redeems a claim on the installation that issued its token.
    pub fn redeem_invitation(
        &self,
        _claim: &InvitationClaim,
    ) -> Result<MembershipTransition, InvitationError> {
        match self.state {}
    }

    /// Revokes a workspace member as the local manager.
    pub fn revoke_member(
        &self,
        _workspace_id: WorkspaceId,
        _member: PublicIdentity,
    ) -> Result<MembershipTransition, MembershipError> {
        match self.state {}
    }

    /// Returns the current canonical membership projection.
    pub fn membership_snapshot(
        &self,
        _workspace_id: WorkspaceId,
    ) -> Result<MembershipSnapshot, MembershipError> {
        match self.state {}
    }

    /// Answers declared-time membership history.
    ///
    /// This query is informational and must never authorize a new write.
    pub fn was_member(
        &self,
        _workspace_id: WorkspaceId,
        _identity: PublicIdentity,
        _at_unix_ms: i64,
    ) -> Result<bool, MembershipError> {
        match self.state {}
    }

    /// Subscribes after a durable local epoch cursor.
    pub fn subscribe_membership(
        &self,
        _workspace_id: WorkspaceId,
        _after_epoch: MembershipEpoch,
    ) -> Result<MembershipSubscription, SubscriptionError> {
        match self.state {}
    }
}
