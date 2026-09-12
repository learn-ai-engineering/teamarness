use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Mutex, mpsc};
use std::time::{SystemTime, UNIX_EPOCH};

use iroh_base::{PublicKey, SecretKey, Signature};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{
    CoreError, IdentityError, InvitationError, MembershipError, StorageError, SubscriptionError,
    ValueParseError, WorkspaceError,
};
use crate::{InvitationClaim, InvitationToken, MembershipOperationId};

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

    /// Verifies a signature using the same versioned, domain-separated envelope as [`CoreRuntime::sign`].
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

impl PublicSignature {
    fn as_iroh_signature(&self) -> Signature {
        Signature::from_bytes(&self.0)
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
    pub(crate) const fn new(
        workspace_id: WorkspaceId,
        epoch: MembershipEpoch,
        version: MembershipVersion,
        cause: MembershipOperationId,
        snapshot_digest: MembershipSnapshotDigest,
    ) -> Self {
        Self {
            workspace_id,
            epoch,
            version,
            cause,
            snapshot_digest,
        }
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

/// A gap-free stream of durable then live membership transitions.
#[derive(Debug)]
pub struct MembershipSubscription {
    state: crate::subscription::SubscriptionState,
}

impl MembershipSubscription {
    /// Waits for the next transition, returning `None` after orderly shutdown.
    pub fn next_transition(&mut self) -> Result<Option<MembershipTransition>, SubscriptionError> {
        Ok(self.state.next())
    }
}

/// The sole public command and query seam for durable core state.
///
/// The runtime owns its SQLite connection and installation secret. Neither
/// those values nor private test dependencies can be obtained by callers.
pub struct CoreRuntime {
    public_identity: PublicIdentity,
    state: Mutex<RuntimeState>,
    clock: Box<dyn Clock>,
}

impl fmt::Debug for CoreRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CoreRuntime")
            .field("public_identity", &self.public_identity)
            .finish_non_exhaustive()
    }
}

struct RuntimeState {
    connection: rusqlite::Connection,
    secret_key: iroh_base::SecretKey,
    display_name: String,
    memberships: BTreeMap<WorkspaceId, crate::membership::WorkspaceProjection>,
    subscribers: BTreeMap<WorkspaceId, Vec<crate::subscription::TransitionSender>>,
    poisoned: bool,
}

trait Clock: Send + Sync {
    fn now_unix_ms(&self) -> i64;
}

struct SystemClock;

impl Clock for SystemClock {
    fn now_unix_ms(&self) -> i64 {
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
    }
}

impl CoreRuntime {
    /// Authenticates a canonical membership envelope without consulting workspace state.
    ///
    /// This verifies only version-1 structure, content identity, admission proof, and signatures.
    /// Trust anchoring, dependency resolution, timestamp policy, and authorization remain part of
    /// durable workspace processing.
    pub fn authenticate_membership_operation(
        canonical_envelope: &[u8],
    ) -> Result<MembershipOperationId, MembershipError> {
        crate::membership::authenticate(canonical_envelope).map(|envelope| envelope.id)
    }

    /// Opens or initializes the runtime in its private data directory.
    pub fn open(config: CoreRuntimeConfig) -> Result<Self, CoreError> {
        Self::open_inner(config, Box::new(SystemClock))
    }

    #[cfg(test)]
    fn open_with_clock(
        config: CoreRuntimeConfig,
        clock: Box<dyn Clock>,
    ) -> Result<Self, CoreError> {
        Self::open_inner(config, clock)
    }

    fn open_inner(config: CoreRuntimeConfig, clock: Box<dyn Clock>) -> Result<Self, CoreError> {
        let opened_at_unix_ms = clock.now_unix_ms();
        let stored = crate::storage::open(config.private_data_directory())?;
        let crate::storage::StoredRuntime {
            mut connection,
            secret_key,
            display_name,
        } = stored;
        let public_identity = PublicIdentity(secret_key.public());
        let memberships =
            crate::membership::projection::recover(&mut connection, opened_at_unix_ms)?;
        Ok(Self {
            public_identity,
            state: Mutex::new(RuntimeState {
                connection,
                secret_key,
                display_name,
                memberships,
                subscribers: BTreeMap::new(),
                poisoned: false,
            }),
            clock,
        })
    }

    /// Returns this installation's validated public identity.
    #[must_use]
    pub const fn public_identity(&self) -> PublicIdentity {
        self.public_identity
    }

    /// Returns a snapshot of this installation's validated display name.
    #[must_use]
    pub fn display_name(&self) -> String {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .display_name
            .clone()
    }

    /// Updates this installation's display name.
    ///
    /// Names contain 1–64 Unicode alphanumeric characters, spaces, hyphens,
    /// underscores, apostrophes, or periods, with no surrounding whitespace.
    pub fn set_display_name(&self, display_name: &str) -> Result<(), IdentityError> {
        crate::identity::validate_display_name(display_name)?;
        let mut state = self.lock_identity_state()?;
        match crate::storage::update_display_name(&mut state.connection, display_name) {
            Ok(()) => {
                state.display_name = display_name.to_owned();
                Ok(())
            }
            Err(error) => {
                if error == StorageError::UncertainOutcome {
                    state.poisoned = true;
                }
                Err(IdentityError::Storage(error))
            }
        }
    }

    /// Signs bytes under a validated, versioned, length-delimited domain context.
    pub fn sign(
        &self,
        context: &SigningContext,
        payload: &[u8],
    ) -> Result<PublicSignature, IdentityError> {
        let state = self.lock_identity_state()?;
        crate::identity::sign(&state.secret_key, context.as_str(), payload)
            .map(|signature| PublicSignature(signature.to_bytes()))
    }

    fn lock_identity_state(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, RuntimeState>, IdentityError> {
        let state = self
            .state
            .lock()
            .map_err(|_| IdentityError::Storage(StorageError::RuntimePoisoned))?;
        if state.poisoned {
            return Err(IdentityError::Storage(StorageError::RuntimePoisoned));
        }
        Ok(state)
    }

    /// Creates a named workspace with this installation as its first manager.
    ///
    /// Names contain 1–64 Unicode alphanumeric characters, spaces, hyphens,
    /// underscores, apostrophes, or periods, with no surrounding whitespace.
    pub fn create_workspace(&self, name: &str) -> Result<WorkspaceSummary, WorkspaceError> {
        crate::workspace::validate_name(name)?;
        let mut state = self.lock_workspace_state()?;
        let now_unix_ms = self.clock.now_unix_ms();
        let secret_key = state.secret_key.clone();
        let result =
            crate::workspace::create(&mut state.connection, &secret_key, name, now_unix_ms);
        match result {
            Ok(summary) => {
                match crate::membership::projection::recover(&mut state.connection, now_unix_ms) {
                    Ok(memberships) => state.memberships = memberships,
                    Err(error) => {
                        if error == StorageError::UncertainOutcome {
                            state.poisoned = true;
                        }
                        return Err(WorkspaceError::Storage(error));
                    }
                }
                Ok(summary)
            }
            Err(error) => {
                if matches!(
                    error,
                    WorkspaceError::Storage(StorageError::UncertainOutcome)
                ) {
                    state.poisoned = true;
                }
                Err(error)
            }
        }
    }

    /// Lists workspaces known to this installation.
    pub fn list_workspaces(&self) -> Result<Vec<WorkspaceSummary>, WorkspaceError> {
        let state = self.lock_workspace_state()?;
        crate::workspace::list(&state.connection)
    }

    fn lock_workspace_state(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, RuntimeState>, WorkspaceError> {
        let state = self
            .state
            .lock()
            .map_err(|_| WorkspaceError::Storage(StorageError::RuntimePoisoned))?;
        if state.poisoned {
            return Err(WorkspaceError::Storage(StorageError::RuntimePoisoned));
        }
        Ok(state)
    }

    /// Issues an expiring invitation as the local manager.
    pub fn issue_invitation(
        &self,
        workspace_id: WorkspaceId,
        expires_at_unix_ms: i64,
    ) -> Result<InvitationToken, InvitationError> {
        let mut state = self.lock_invitation_state()?;
        let now_unix_ms = self.clock.now_unix_ms();
        self.maintain_and_publish(&mut state, now_unix_ms)
            .map_err(InvitationError::Membership)?;
        let RuntimeState {
            secret_key,
            memberships,
            ..
        } = &mut *state;
        let projection = memberships
            .get(&workspace_id)
            .ok_or(InvitationError::WorkspaceNotFound)?;
        let local_is_manager = projection.snapshot().members().iter().any(|member| {
            member.identity() == self.public_identity && member.role() == MembershipRole::Manager
        });
        if !local_is_manager {
            return Err(InvitationError::NotManager);
        }
        if expires_at_unix_ms <= now_unix_ms {
            return Err(InvitationError::Expired);
        }
        let nonce = SecretKey::generate().to_bytes();
        let token = crate::invitation::sign_token(
            secret_key,
            workspace_id,
            now_unix_ms,
            expires_at_unix_ms,
            nonce,
        )?;
        Ok(InvitationToken::from_verified(&token))
    }

    /// Creates proof that this installation claims the supplied invitation.
    pub fn create_invitation_claim(
        &self,
        token: &InvitationToken,
    ) -> Result<InvitationClaim, InvitationError> {
        let token = token.verified()?;
        let state = self.lock_invitation_state()?;
        let claim = crate::invitation::sign_claim(&state.secret_key, &token)?;
        Ok(InvitationClaim::from_verified(&claim))
    }

    /// Redeems a claim on the installation that issued its token.
    pub fn redeem_invitation(
        &self,
        claim: &InvitationClaim,
    ) -> Result<MembershipTransition, InvitationError> {
        self.redeem_invitation_inner(claim, |_| Ok(()))
    }

    fn redeem_invitation_inner(
        &self,
        claim: &InvitationClaim,
        fault: impl FnMut(crate::membership::projection::RedemptionBoundary) -> Result<(), StorageError>,
    ) -> Result<MembershipTransition, InvitationError> {
        let claim = claim.verified()?;
        let mut state = self.lock_invitation_state()?;
        let now_unix_ms = self.clock.now_unix_ms();
        self.maintain_and_publish(&mut state, now_unix_ms)
            .map_err(InvitationError::Membership)?;
        let RuntimeState {
            connection,
            secret_key,
            memberships,
            poisoned,
            ..
        } = &mut *state;
        let result = crate::membership::projection::redeem_invitation_with_fault(
            connection,
            memberships,
            secret_key,
            claim,
            now_unix_ms,
            fault,
        );
        match result {
            Ok(applied) => {
                let transition = Self::transition_from_applied(applied);
                Self::publish_transition(&mut state, transition.clone());
                Ok(transition)
            }
            Err(error) => {
                if matches!(
                    error,
                    InvitationError::Storage(StorageError::UncertainOutcome)
                        | InvitationError::Membership(MembershipError::Storage(
                            StorageError::UncertainOutcome
                        ))
                ) {
                    *poisoned = true;
                }
                Err(error)
            }
        }
    }

    #[cfg(test)]
    fn redeem_invitation_with_test_fault(
        &self,
        claim: &InvitationClaim,
        boundary: crate::membership::projection::RedemptionBoundary,
    ) -> Result<MembershipTransition, InvitationError> {
        self.redeem_invitation_inner(claim, |current| {
            if current == boundary {
                Err(
                    if current == crate::membership::projection::RedemptionBoundary::AfterCommit {
                        StorageError::UncertainOutcome
                    } else {
                        StorageError::Database
                    },
                )
            } else {
                Ok(())
            }
        })
    }

    fn lock_invitation_state(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, RuntimeState>, InvitationError> {
        let state = self
            .state
            .lock()
            .map_err(|_| InvitationError::Storage(StorageError::RuntimePoisoned))?;
        if state.poisoned {
            return Err(InvitationError::Storage(StorageError::RuntimePoisoned));
        }
        Ok(state)
    }

    /// Revokes a workspace member as the local manager.
    pub fn revoke_member(
        &self,
        workspace_id: WorkspaceId,
        member: PublicIdentity,
    ) -> Result<MembershipTransition, MembershipError> {
        let mut state = self.lock_membership_state()?;
        let now_unix_ms = self.clock.now_unix_ms();
        self.maintain_and_publish(&mut state, now_unix_ms)?;
        let RuntimeState {
            connection,
            secret_key,
            memberships,
            poisoned,
            ..
        } = &mut *state;
        let current = memberships
            .get(&workspace_id)
            .cloned()
            .ok_or(MembershipError::WorkspaceNotFound)?;
        if current.creator() == member {
            return Err(MembershipError::CannotRevokeManager);
        }
        if !current.contains_member(member) {
            return Err(MembershipError::MemberNotFound);
        }
        let envelope = crate::membership::MembershipEnvelope::sign(
            secret_key,
            workspace_id,
            now_unix_ms,
            crate::membership::MembershipAction::Revoke { target: member },
            current.heads(),
        )?;
        match crate::membership::projection::apply_canonical(
            connection,
            memberships,
            envelope.canonical_bytes(),
            now_unix_ms,
        ) {
            Ok(applied) if applied.accepted => {
                let transition = Self::transition_from_applied(applied);
                Self::publish_transition(&mut state, transition.clone());
                Ok(transition)
            }
            Ok(_) => Err(MembershipError::AuthorizationRejected),
            Err(error) => {
                if matches!(
                    error,
                    MembershipError::Storage(StorageError::UncertainOutcome)
                ) {
                    *poisoned = true;
                }
                Err(error)
            }
        }
    }

    /// Returns the current canonical membership projection.
    pub fn membership_snapshot(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<MembershipSnapshot, MembershipError> {
        let mut state = self.lock_membership_state()?;
        let now_unix_ms = self.clock.now_unix_ms();
        self.maintain_and_publish(&mut state, now_unix_ms)?;
        state
            .memberships
            .get(&workspace_id)
            .map(crate::membership::WorkspaceProjection::snapshot)
            .ok_or(MembershipError::WorkspaceNotFound)
    }

    fn maintain_and_publish(
        &self,
        state: &mut RuntimeState,
        now_unix_ms: i64,
    ) -> Result<(), MembershipError> {
        let applied = match crate::membership::projection::maintain_quarantine(
            &mut state.connection,
            &mut state.memberships,
            now_unix_ms,
        ) {
            Ok(applied) => applied,
            Err(error) => {
                if matches!(
                    error,
                    MembershipError::Storage(StorageError::UncertainOutcome)
                ) {
                    state.poisoned = true;
                }
                return Err(error);
            }
        };
        for applied in applied {
            Self::publish_transition(state, Self::transition_from_applied(applied));
        }
        Ok(())
    }

    fn transition_from_applied(
        applied: crate::membership::projection::AppliedMembership,
    ) -> MembershipTransition {
        MembershipTransition::new(
            applied.snapshot.workspace_id(),
            applied.snapshot.epoch(),
            applied.snapshot.version(),
            applied.operation_id,
            applied.snapshot_digest,
        )
    }

    fn publish_transition(state: &mut RuntimeState, transition: MembershipTransition) {
        if let Some(listeners) = state.subscribers.get_mut(&transition.workspace_id()) {
            listeners.retain(|listener| listener.send(transition.clone()).is_ok());
        }
    }

    fn lock_membership_state(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, RuntimeState>, MembershipError> {
        let state = self
            .state
            .lock()
            .map_err(|_| MembershipError::Storage(StorageError::RuntimePoisoned))?;
        if state.poisoned {
            return Err(MembershipError::Storage(StorageError::RuntimePoisoned));
        }
        Ok(state)
    }

    // Phase 6 invitation redemption will call this same production path.
    #[allow(dead_code)]
    pub(crate) fn apply_membership_envelope(
        &self,
        canonical_envelope: &[u8],
    ) -> Result<crate::membership::projection::AppliedMembership, MembershipError> {
        let mut state = self.lock_membership_state()?;
        let now_unix_ms = self.clock.now_unix_ms();
        self.maintain_and_publish(&mut state, now_unix_ms)?;
        let result = {
            let RuntimeState {
                connection,
                memberships,
                ..
            } = &mut *state;
            crate::membership::projection::apply_canonical(
                connection,
                memberships,
                canonical_envelope,
                now_unix_ms,
            )
        };
        match result {
            Ok(applied) => {
                if applied.accepted {
                    let transition = MembershipTransition::new(
                        applied.snapshot.workspace_id(),
                        applied.snapshot.epoch(),
                        applied.snapshot.version(),
                        applied.operation_id,
                        applied.snapshot_digest,
                    );
                    Self::publish_transition(&mut state, transition);
                }
                Ok(applied)
            }
            Err(error) => {
                if matches!(
                    error,
                    MembershipError::Storage(StorageError::UncertainOutcome)
                ) {
                    state.poisoned = true;
                }
                Err(error)
            }
        }
    }

    /// Returns the current content-derived membership version.
    pub fn membership_version(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<MembershipVersion, MembershipError> {
        self.membership_snapshot(workspace_id)
            .map(|snapshot| snapshot.version())
    }

    /// Answers authenticated author-declared membership history.
    ///
    /// Admission is inclusive at its signed authored millisecond and revocation
    /// is exclusive at its signed authored millisecond. Late operations can
    /// revise an earlier answer. This informational query must never authorize
    /// a new write; authorization must use current membership and its version.
    pub fn was_member(
        &self,
        workspace_id: WorkspaceId,
        identity: PublicIdentity,
        at_unix_ms: i64,
    ) -> Result<bool, MembershipError> {
        let mut state = self.lock_membership_state()?;
        let now_unix_ms = self.clock.now_unix_ms();
        self.maintain_and_publish(&mut state, now_unix_ms)?;
        let projection = state
            .memberships
            .get(&workspace_id)
            .ok_or(MembershipError::WorkspaceNotFound)?;
        crate::membership::was_member(projection, workspace_id, identity, at_unix_ms)
    }

    /// Subscribes after a durable local epoch cursor.
    ///
    /// Durable catch-up is returned first in epoch order, followed by live
    /// post-commit delivery. Registering the listener and reading catch-up both
    /// happen while the runtime state lock is held, preventing a handoff gap.
    pub fn subscribe_membership(
        &self,
        workspace_id: WorkspaceId,
        after_epoch: MembershipEpoch,
    ) -> Result<MembershipSubscription, SubscriptionError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| SubscriptionError::Storage(StorageError::RuntimePoisoned))?;
        if state.poisoned {
            return Err(SubscriptionError::Storage(StorageError::RuntimePoisoned));
        }
        let now_unix_ms = self.clock.now_unix_ms();
        self.maintain_and_publish(&mut state, now_unix_ms)
            .map_err(|error| match error {
                MembershipError::Storage(error) => SubscriptionError::Storage(error),
                _ => SubscriptionError::Storage(StorageError::CorruptWorkspace),
            })?;
        let (sender, receiver) = mpsc::channel();
        state
            .subscribers
            .entry(workspace_id)
            .or_default()
            .push(sender);
        let durable =
            match crate::subscription::load_after(&state.connection, workspace_id, after_epoch) {
                Ok(durable) => durable,
                Err(error) => {
                    state
                        .subscribers
                        .get_mut(&workspace_id)
                        .expect("listener was registered")
                        .pop();
                    return Err(error);
                }
            };
        Ok(MembershipSubscription {
            state: crate::subscription::SubscriptionState::new(durable, receiver, after_epoch),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

    use iroh_base::SecretKey;

    use super::*;
    use crate::invitation::{sign_claim, sign_token};
    use crate::membership::{AdmissionProof, MembershipAction, MembershipEnvelope};

    struct RecordingClock {
        calls: Arc<AtomicU64>,
    }

    impl Clock for RecordingClock {
        fn now_unix_ms(&self) -> i64 {
            self.calls.fetch_add(1, Ordering::Relaxed);
            1_234
        }
    }

    #[test]
    fn test_clock_is_injected_without_entering_the_public_configuration() {
        let calls = Arc::new(AtomicU64::new(0));
        let directory =
            std::env::temp_dir().join(format!("resonance-runtime-clock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);

        let runtime = CoreRuntime::open_with_clock(
            CoreRuntimeConfig::new(&directory),
            Box::new(RecordingClock {
                calls: Arc::clone(&calls),
            }),
        )
        .unwrap();

        assert_eq!(calls.load(Ordering::Relaxed), 1);
        drop(runtime);
        fs::remove_dir_all(directory).unwrap();
    }

    fn member_secret(byte: u8) -> SecretKey {
        SecretKey::from_bytes(&[byte; 32])
    }

    fn runtime_secret(runtime: &CoreRuntime) -> SecretKey {
        runtime
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .secret_key
            .clone()
    }

    fn runtime_heads(
        runtime: &CoreRuntime,
        workspace_id: WorkspaceId,
    ) -> Vec<MembershipOperationId> {
        runtime
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .memberships
            .get(&workspace_id)
            .unwrap()
            .heads()
    }

    fn admit(
        manager: &SecretKey,
        invitee: &SecretKey,
        workspace_id: WorkspaceId,
        timestamp: i64,
        dependencies: Vec<MembershipOperationId>,
        nonce: u8,
    ) -> MembershipEnvelope {
        let token = sign_token(
            manager,
            workspace_id,
            timestamp - 1,
            timestamp + 100_000,
            [nonce; 32],
        )
        .unwrap();
        let claim = sign_claim(invitee, &token).unwrap();
        let target = claim.invitee;
        MembershipEnvelope::sign(
            manager,
            workspace_id,
            timestamp,
            MembershipAction::Admit {
                target,
                proof: Box::new(AdmissionProof { claim }),
            },
            dependencies,
        )
        .unwrap()
    }

    #[test]
    fn uncertain_invitation_commit_poisons_runtime_until_authenticated_reopen() {
        let issuer_directory = std::env::temp_dir().join(format!(
            "resonance-runtime-invitation-uncertain-{}",
            std::process::id()
        ));
        let invitee_directory = std::env::temp_dir().join(format!(
            "resonance-runtime-invitation-claimant-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&issuer_directory);
        let _ = fs::remove_dir_all(&invitee_directory);
        let config = CoreRuntimeConfig::new(&issuer_directory);
        let issuer = CoreRuntime::open(config.clone()).unwrap();
        let invitee = CoreRuntime::open(CoreRuntimeConfig::new(&invitee_directory)).unwrap();
        let workspace = issuer.create_workspace("Uncertain invitation").unwrap();
        let token = issuer
            .issue_invitation(
                workspace.id(),
                issuer.clock.now_unix_ms().saturating_add(60_000),
            )
            .unwrap();
        let claim = invitee.create_invitation_claim(&token).unwrap();

        assert_eq!(
            issuer
                .redeem_invitation_with_test_fault(
                    &claim,
                    crate::membership::projection::RedemptionBoundary::AfterCommit,
                )
                .unwrap_err(),
            InvitationError::Storage(StorageError::UncertainOutcome)
        );
        assert_eq!(
            issuer
                .issue_invitation(
                    workspace.id(),
                    issuer.clock.now_unix_ms().saturating_add(60_000),
                )
                .unwrap_err(),
            InvitationError::Storage(StorageError::RuntimePoisoned)
        );
        drop(issuer);

        let reopened = CoreRuntime::open(config).unwrap();
        let snapshot = reopened.membership_snapshot(workspace.id()).unwrap();
        assert_eq!(snapshot.epoch(), MembershipEpoch::new(1));
        assert_eq!(snapshot.members().len(), 2);
        assert_eq!(
            reopened.redeem_invitation(&claim).unwrap_err(),
            InvitationError::AlreadyUsed
        );

        drop(reopened);
        drop(invitee);
        fs::remove_dir_all(issuer_directory).unwrap();
        fs::remove_dir_all(invitee_directory).unwrap();
    }

    #[test]
    fn canonical_path_admits_revokes_and_rejects_invalid_authority_without_advancing_state() {
        let directory = std::env::temp_dir().join(format!(
            "resonance-runtime-membership-validation-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        let runtime = CoreRuntime::open(CoreRuntimeConfig::new(&directory)).unwrap();
        let workspace = runtime.create_workspace("Membership validation").unwrap();
        let manager = runtime_secret(&runtime);
        let member = member_secret(31);
        let base = workspace.created_at_unix_ms();

        let admission = admit(
            &manager,
            &member,
            workspace.id(),
            base + 1,
            runtime_heads(&runtime, workspace.id()),
            1,
        );
        let accepted = runtime
            .apply_membership_envelope(admission.canonical_bytes())
            .unwrap();
        assert!(accepted.accepted);
        assert!(!accepted.quarantined);
        assert_eq!(accepted.snapshot.epoch(), MembershipEpoch::new(1));

        let duplicate = runtime
            .apply_membership_envelope(admission.canonical_bytes())
            .unwrap();
        assert!(!duplicate.accepted);
        assert_eq!(duplicate.snapshot.epoch(), MembershipEpoch::new(1));
        let transition_count: i64 = runtime
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .connection
            .query_row("SELECT COUNT(*) FROM membership_transitions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(transition_count, 1);

        let outsider = member_secret(32);
        let wrong_token =
            sign_token(&outsider, workspace.id(), base, base + 100_000, [2; 32]).unwrap();
        let wrong_claim = sign_claim(&member_secret(33), &wrong_token).unwrap();
        let invalid_proof = MembershipEnvelope::sign(
            &manager,
            workspace.id(),
            base + 2,
            MembershipAction::Admit {
                target: wrong_claim.invitee,
                proof: Box::new(AdmissionProof { claim: wrong_claim }),
            },
            runtime_heads(&runtime, workspace.id()),
        )
        .unwrap();
        assert_eq!(
            runtime
                .apply_membership_envelope(invalid_proof.canonical_bytes())
                .unwrap_err(),
            MembershipError::InvalidAdmissionProof
        );

        let writer_action = MembershipEnvelope::sign(
            &member,
            workspace.id(),
            base + 2,
            MembershipAction::Revoke {
                target: runtime.public_identity(),
            },
            runtime_heads(&runtime, workspace.id()),
        )
        .unwrap();
        assert_eq!(
            runtime
                .apply_membership_envelope(writer_action.canonical_bytes())
                .unwrap_err(),
            MembershipError::NotManager
        );
        assert_eq!(
            runtime.membership_snapshot(workspace.id()).unwrap().epoch(),
            MembershipEpoch::new(1)
        );

        let revocation = runtime
            .revoke_member(
                workspace.id(),
                PublicIdentity::from_bytes(*member.public().as_bytes()).unwrap(),
            )
            .unwrap();
        assert_eq!(revocation.epoch(), MembershipEpoch::new(2));
        let readmission = admit(
            &manager,
            &member,
            workspace.id(),
            base + 100_000,
            vec![revocation.cause()],
            3,
        );
        assert_eq!(
            runtime
                .apply_membership_envelope(readmission.canonical_bytes())
                .unwrap_err(),
            MembershipError::CausalReadmission
        );
        let final_snapshot = runtime.membership_snapshot(workspace.id()).unwrap();
        assert_eq!(final_snapshot.epoch(), MembershipEpoch::new(2));
        assert_eq!(final_snapshot.members().len(), 1);

        drop(runtime);
        fs::remove_dir_all(directory).unwrap();
    }

    fn topological_orders(prerequisites: &[Vec<usize>]) -> Vec<Vec<usize>> {
        fn visit(
            prerequisites: &[Vec<usize>],
            current: &mut Vec<usize>,
            orders: &mut Vec<Vec<usize>>,
        ) {
            if current.len() == prerequisites.len() {
                orders.push(current.clone());
                return;
            }
            for candidate in 0..prerequisites.len() {
                if current.contains(&candidate)
                    || !prerequisites[candidate]
                        .iter()
                        .all(|required| current.contains(required))
                {
                    continue;
                }
                current.push(candidate);
                visit(prerequisites, current, orders);
                current.pop();
            }
        }
        let mut orders = Vec::new();
        visit(prerequisites, &mut Vec::new(), &mut orders);
        orders
    }

    #[test]
    fn every_reachable_topological_order_converges_for_single_manager_branches() {
        let baseline = std::env::temp_dir().join(format!(
            "resonance-runtime-convergence-baseline-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&baseline);
        let runtime = CoreRuntime::open(CoreRuntimeConfig::new(&baseline)).unwrap();
        let workspace = runtime.create_workspace("Reachable convergence").unwrap();
        let manager = runtime_secret(&runtime);
        let genesis = runtime_heads(&runtime, workspace.id())[0];
        let base = workspace.created_at_unix_ms();
        let member_a = member_secret(41);
        let member_b = member_secret(42);
        let member_c = member_secret(43);

        let admit_a = admit(
            &manager,
            &member_a,
            workspace.id(),
            base + 1,
            vec![genesis],
            11,
        );
        let duplicate_a = admit(
            &manager,
            &member_a,
            workspace.id(),
            base + 2,
            vec![genesis],
            12,
        );
        let admit_b = admit(
            &manager,
            &member_b,
            workspace.id(),
            base + 3,
            vec![genesis],
            13,
        );
        let revoke_a = MembershipEnvelope::sign(
            &manager,
            workspace.id(),
            base + 4,
            MembershipAction::Revoke {
                target: PublicIdentity::from_bytes(*member_a.public().as_bytes()).unwrap(),
            },
            vec![admit_a.id],
        )
        .unwrap();
        let admit_c = admit(
            &manager,
            &member_c,
            workspace.id(),
            base + 4,
            vec![admit_b.id],
            14,
        );
        let operations = [admit_a, duplicate_a, admit_b, revoke_a, admit_c]
            .map(|operation| operation.canonical_bytes().to_vec());
        drop(runtime);

        let orders = topological_orders(&[vec![], vec![], vec![], vec![0], vec![2]]);
        assert!(orders.len() > 1);
        let mut expected = None;
        for (index, order) in orders.into_iter().enumerate() {
            let directory = std::env::temp_dir().join(format!(
                "resonance-runtime-convergence-{}-{index}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&directory);
            fs::create_dir_all(&directory).unwrap();
            fs::copy(
                baseline.join(crate::storage::DATABASE_FILE_NAME),
                directory.join(crate::storage::DATABASE_FILE_NAME),
            )
            .unwrap();
            let branch = CoreRuntime::open(CoreRuntimeConfig::new(&directory)).unwrap();
            for operation in order {
                assert!(
                    branch
                        .apply_membership_envelope(&operations[operation])
                        .unwrap()
                        .accepted
                );
            }
            let snapshot = branch.membership_snapshot(workspace.id()).unwrap();
            assert_eq!(snapshot.epoch(), MembershipEpoch::new(5));
            let member_a_identity =
                PublicIdentity::from_bytes(*member_a.public().as_bytes()).unwrap();
            assert!(
                branch
                    .was_member(workspace.id(), member_a_identity, base + 1)
                    .unwrap()
            );
            assert!(
                branch
                    .was_member(workspace.id(), member_a_identity, base + 3)
                    .unwrap()
            );
            assert!(
                !branch
                    .was_member(workspace.id(), member_a_identity, base + 4)
                    .unwrap()
            );
            match &expected {
                Some(expected) => assert_eq!(&snapshot, expected),
                None => expected = Some(snapshot),
            }
            drop(branch);
            fs::remove_dir_all(directory).unwrap();
        }

        fs::remove_dir_all(baseline).unwrap();
    }

    struct AdjustableClock(Arc<AtomicI64>);

    impl Clock for AdjustableClock {
        fn now_unix_ms(&self) -> i64 {
            self.0.load(Ordering::Relaxed)
        }
    }

    #[test]
    fn quarantine_retries_dependency_first_and_never_advances_twice() {
        let directory = std::env::temp_dir().join(format!(
            "resonance-runtime-quarantine-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        let now = Arc::new(AtomicI64::new(1_000));
        let config = CoreRuntimeConfig::new(&directory);
        let runtime = CoreRuntime::open_with_clock(
            config.clone(),
            Box::new(AdjustableClock(Arc::clone(&now))),
        )
        .unwrap();
        let workspace = runtime.create_workspace("Quarantine").unwrap();
        let manager = runtime_secret(&runtime);
        let member = member_secret(51);
        let genesis = runtime_heads(&runtime, workspace.id())[0];
        let admission = admit(
            &manager,
            &member,
            workspace.id(),
            301_001,
            vec![genesis],
            21,
        );
        let revocation = MembershipEnvelope::sign(
            &manager,
            workspace.id(),
            301_002,
            MembershipAction::Revoke {
                target: PublicIdentity::from_bytes(*member.public().as_bytes()).unwrap(),
            },
            vec![admission.id],
        )
        .unwrap();

        let first = runtime
            .apply_membership_envelope(admission.canonical_bytes())
            .unwrap();
        let second = runtime
            .apply_membership_envelope(revocation.canonical_bytes())
            .unwrap();
        assert!(first.quarantined && second.quarantined);
        assert_eq!(
            runtime.membership_snapshot(workspace.id()).unwrap().epoch(),
            MembershipEpoch::new(0)
        );
        let member_identity = PublicIdentity::from_bytes(*member.public().as_bytes()).unwrap();
        assert!(
            !runtime
                .was_member(workspace.id(), member_identity, 301_001)
                .unwrap()
        );
        let transition_count: i64 = runtime
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .connection
            .query_row("SELECT COUNT(*) FROM membership_transitions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(transition_count, 0);

        drop(runtime);
        now.store(1_002, Ordering::Relaxed);
        let recovered = CoreRuntime::open_with_clock(
            config.clone(),
            Box::new(AdjustableClock(Arc::clone(&now))),
        )
        .unwrap();
        let eligible = recovered.membership_snapshot(workspace.id()).unwrap();
        assert_eq!(eligible.epoch(), MembershipEpoch::new(2));
        assert_eq!(eligible.members().len(), 1);
        assert!(
            recovered
                .was_member(workspace.id(), member_identity, 301_001)
                .unwrap()
        );
        assert!(
            !recovered
                .was_member(workspace.id(), member_identity, 301_002)
                .unwrap()
        );
        let transition_count: i64 = recovered
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .connection
            .query_row("SELECT COUNT(*) FROM membership_transitions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(transition_count, 2);
        assert_eq!(
            recovered.membership_snapshot(workspace.id()).unwrap(),
            eligible
        );
        drop(recovered);

        let reopened =
            CoreRuntime::open_with_clock(config, Box::new(AdjustableClock(Arc::clone(&now))))
                .unwrap();
        assert_eq!(
            reopened.membership_snapshot(workspace.id()).unwrap(),
            eligible
        );

        drop(reopened);
        fs::remove_dir_all(directory).unwrap();
    }
}
