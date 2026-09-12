use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use iroh_base::{PublicKey, Signature};

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
        let _opened_at_unix_ms = clock.now_unix_ms();
        let stored = crate::storage::open(config.private_data_directory())?;
        let public_identity = PublicIdentity(stored.secret_key.public());
        Ok(Self {
            public_identity,
            state: Mutex::new(RuntimeState {
                connection: stored.connection,
                secret_key: stored.secret_key,
                display_name: stored.display_name,
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
        if matches!(
            result,
            Err(WorkspaceError::Storage(StorageError::UncertainOutcome))
        ) {
            state.poisoned = true;
        }
        result
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
        _workspace_id: WorkspaceId,
        _expires_at_unix_ms: i64,
    ) -> Result<InvitationToken, InvitationError> {
        Err(InvitationError::RuntimeUnavailable)
    }

    /// Creates proof that this installation claims the supplied invitation.
    pub fn create_invitation_claim(
        &self,
        _token: &InvitationToken,
    ) -> Result<InvitationClaim, InvitationError> {
        Err(InvitationError::RuntimeUnavailable)
    }

    /// Redeems a claim on the installation that issued its token.
    pub fn redeem_invitation(
        &self,
        _claim: &InvitationClaim,
    ) -> Result<MembershipTransition, InvitationError> {
        Err(InvitationError::RuntimeUnavailable)
    }

    /// Revokes a workspace member as the local manager.
    pub fn revoke_member(
        &self,
        _workspace_id: WorkspaceId,
        _member: PublicIdentity,
    ) -> Result<MembershipTransition, MembershipError> {
        Err(MembershipError::RuntimeUnavailable)
    }

    /// Returns the current canonical membership projection.
    pub fn membership_snapshot(
        &self,
        _workspace_id: WorkspaceId,
    ) -> Result<MembershipSnapshot, MembershipError> {
        Err(MembershipError::RuntimeUnavailable)
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
        Err(MembershipError::RuntimeUnavailable)
    }

    /// Subscribes after a durable local epoch cursor.
    pub fn subscribe_membership(
        &self,
        _workspace_id: WorkspaceId,
        _after_epoch: MembershipEpoch,
    ) -> Result<MembershipSubscription, SubscriptionError> {
        Err(SubscriptionError::RuntimeUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

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
}
