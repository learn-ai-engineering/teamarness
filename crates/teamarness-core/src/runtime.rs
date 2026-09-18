use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::time::{SystemTime, UNIX_EPOCH};

use iroh_base::{PublicKey, SecretKey, Signature};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::MembershipOperationId;
use crate::error::{
    CoreError, IdentityError, InvitationError, MembershipError, StorageError, SubscriptionError,
    ValueParseError, WorkspaceError,
};
use crate::invitation::{InvitationClaim, InvitationToken};

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

/// A gap-free stream of durable then live membership transitions.
pub struct MembershipSubscription {
    state: crate::subscription::SubscriptionState,
    runtime_state: Weak<Mutex<RuntimeState>>,
    workspace_id: WorkspaceId,
    subscriber_id: u64,
}

impl fmt::Debug for MembershipSubscription {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MembershipSubscription")
            .finish_non_exhaustive()
    }
}

impl Drop for MembershipSubscription {
    fn drop(&mut self) {
        let Some(runtime_state) = self.runtime_state.upgrade() else {
            return;
        };
        let Ok(mut state) = runtime_state.lock() else {
            return;
        };
        if let Some(listeners) = state.subscribers.get_mut(&self.workspace_id) {
            listeners.retain(|listener| listener.id != self.subscriber_id);
            if listeners.is_empty() {
                state.subscribers.remove(&self.workspace_id);
            }
        }
    }
}

impl MembershipSubscription {
    /// Waits for the next transition, returning `None` after orderly shutdown.
    pub fn next_transition(&mut self) -> Result<Option<MembershipTransition>, SubscriptionError> {
        Ok(self.state.next())
    }

    /// Waits up to `timeout` for the next transition.
    ///
    /// `None` means no transition arrived before the timeout or the runtime shut down.
    /// Reopening after the last delivered epoch preserves durable catch-up.
    pub fn next_transition_timeout(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<Option<MembershipTransition>, SubscriptionError> {
        Ok(self.state.next_timeout(timeout))
    }
}

/// The sole public command and query seam for durable core state.
///
/// The runtime owns its SQLite connection and installation secret. Neither
/// those values nor private test dependencies can be obtained by callers.
pub struct CoreRuntime {
    public_identity: PublicIdentity,
    state: Arc<Mutex<RuntimeState>>,
    clock: Arc<dyn Clock>,
    transport: Arc<crate::onboarding::transport::Transport>,
    sync_worker: Arc<crate::synchronization::SyncWorker>,
    membership_demand: Arc<crate::membership_sync::MembershipDemand>,
    #[cfg(test)]
    membership_replica: Arc<dyn crate::membership_sync::MembershipReplica>,
    #[cfg(test)]
    membership_paused: Arc<std::sync::atomic::AtomicBool>,
}

impl fmt::Debug for CoreRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CoreRuntime")
            .field("public_identity", &self.public_identity)
            .finish_non_exhaustive()
    }
}

struct MembershipSubscriber {
    id: u64,
    sender: crate::subscription::TransitionSender,
}

type RecordSubscriber = std::sync::mpsc::Sender<crate::RecordEvent>;

/// The Core side of every package synchronization session.
///
/// Every method re-checks current membership, so a revocation that lands while
/// a session is running stops that session serving or accepting anything more.
pub(crate) struct RuntimeSyncReceiver {
    pub(crate) state: Weak<Mutex<RuntimeState>>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) demand: Arc<crate::membership_sync::MembershipDemand>,
}

impl crate::synchronization::SyncReceiver for RuntimeSyncReceiver {
    fn authorize_session(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
    ) -> Result<crate::synchronization::SessionGrant, crate::synchronization::Refusal> {
        let state = self
            .state
            .upgrade()
            .ok_or(crate::synchronization::Refusal::Unavailable)?;
        let state = state
            .lock()
            .map_err(|_| crate::synchronization::Refusal::Unavailable)?;
        if state.poisoned {
            return Err(crate::synchronization::Refusal::Unavailable);
        }
        let local = PublicIdentity(state.secret_key.public());
        let projection = state
            .memberships
            .get(&workspace)
            .ok_or(crate::synchronization::Refusal::NotAuthorized)?;
        if !projection.contains_member(local) || !projection.contains_member(peer) {
            return Err(crate::synchronization::Refusal::NotAuthorized);
        }
        let membership_version = *projection.snapshot().version().as_bytes();
        let scope = crate::synchronization::scope_digest(package, workspace);
        let content_key_id = state
            .connection
            .query_row(
                "SELECT content_key_id FROM sync_scopes WHERE scope_id=?1",
                [scope.as_slice()],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()
            .map_err(|_| crate::synchronization::Refusal::Unavailable)?
            .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok());
        Ok(crate::synchronization::SessionGrant {
            membership_version,
            content_key_id,
        })
    }

    fn protection_bundle_for(
        &self,
        workspace: WorkspaceId,
        package: &str,
        recipient: PublicIdentity,
    ) -> Result<Option<crate::synchronization::ProtectionBundle>, ()> {
        let state = self.state.upgrade().ok_or(())?;
        let mut state = state.lock().map_err(|_| ())?;
        let projection = state.memberships.get(&workspace).ok_or(())?;
        let local = PublicIdentity(state.secret_key.public());
        if !projection.contains_member(local) || !projection.contains_member(recipient) {
            return Err(());
        }
        let admission = *projection.snapshot().version().as_bytes();
        let scope = crate::synchronization::scope_digest(package, workspace);
        let origin: Option<Vec<u8>> = state
            .connection
            .query_row(
                "SELECT control_bytes FROM protection_control_operations
                 WHERE scope_id=?1 AND operation_kind='create' LIMIT 1",
                [scope.as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| ())?;
        // Without the creator's origin this Installation cannot prove the key's
        // authority, so it offers nothing rather than an unusable grant.
        let Some(origin) = origin else {
            return Ok(None);
        };
        let secret_key = state.secret_key.clone();
        match crate::synchronization::export_protection_grant(
            &mut state.connection,
            &secret_key,
            workspace,
            package,
            recipient,
            admission,
        ) {
            Ok(grant) => Ok(Some(crate::synchronization::ProtectionBundle {
                origin,
                grant,
            })),
            Err(crate::SynchronizationError::WorkspaceNotFound) => Ok(None),
            Err(_) => Err(()),
        }
    }

    fn import_protection_bundle(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
        bundle: crate::synchronization::ProtectionBundle,
    ) -> Result<usize, ()> {
        let state = self.state.upgrade().ok_or(())?;
        let mut state = state.lock().map_err(|_| ())?;
        if crate::synchronization::protection_control_author(&bundle.grant).map_err(|_| ())? != peer
        {
            return Err(());
        }
        let projection = state.memberships.get(&workspace).ok_or(())?;
        if !projection.contains_member(peer) {
            return Err(());
        }
        let creator = projection.creator();
        let secret = state.secret_key.clone();
        crate::synchronization::import_protection_grant(
            &mut state.connection,
            &secret,
            workspace,
            package,
            creator,
            &bundle.origin,
            &bundle.grant,
        )
        .map_err(|_| ())?;
        // Records that arrived before the key can now be applied.
        replication::promote_scope(&mut state, workspace, package).map_err(|_| ())
    }

    /// Two Installations that disagree about membership are exactly the case a
    /// membership session resolves, so the worker is asked to run one.
    fn note_membership_mismatch(&self, workspace: WorkspaceId, peer: PublicIdentity) {
        self.demand.note(workspace, peer);
    }

    fn authors(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
    ) -> Result<Vec<PublicIdentity>, ()> {
        let state = self.state.upgrade().ok_or(())?;
        let state = state.lock().map_err(|_| ())?;
        let projection = state.memberships.get(&workspace).ok_or(())?;
        if !projection.contains_member(peer)
            || !projection.contains_member(PublicIdentity(state.secret_key.public()))
        {
            return Err(());
        }
        Ok(projection
            .snapshot()
            .members()
            .iter()
            .map(|member| member.identity())
            .collect())
    }

    fn operations(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
    ) -> Result<Vec<([u8; 32], p2panda_core::Operation<()>)>, ()> {
        let state = self.state.upgrade().ok_or(())?;
        let state = state.lock().map_err(|_| ())?;
        if !state.memberships.get(&workspace).is_some_and(|projection| {
            projection.contains_member(PublicIdentity(state.secret_key.public()))
                && projection.contains_member(peer)
        }) {
            return Err(());
        }
        let scope = crate::synchronization::scope_digest(package, workspace);
        let store = crate::synchronization::p2panda_adapter::TeamarnessLogStore::from_connection(
            &state.connection,
            scope,
        )
        .map_err(|_| ())?;
        let log_id = crate::synchronization::operation_log_id(scope);
        Ok(store
            .operations()
            .into_iter()
            .map(|operation| (log_id, operation))
            .collect())
    }

    /// Classifies one received operation in the order design §5.5 fixes.
    ///
    /// Only a revocation or a storage failure ends the session; anything else
    /// is applied, recognised as a duplicate, parked, or rejected.
    fn receive_operation(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
        operation: p2panda_core::Operation<()>,
    ) -> Result<crate::synchronization::OperationOutcome, crate::synchronization::FatalSyncError>
    {
        use crate::synchronization::{FatalSyncError, OperationOutcome, RejectReason};
        let Ok(author) = PublicIdentity::from_bytes(*operation.header.verifying_key.as_bytes())
        else {
            return Ok(OperationOutcome::Rejected(RejectReason::Invalid));
        };
        let state = self.state.upgrade().ok_or(FatalSyncError::Storage)?;
        let mut state = state.lock().map_err(|_| FatalSyncError::Storage)?;
        let Some(projection) = state.memberships.get(&workspace) else {
            return Err(FatalSyncError::PeerRevoked);
        };
        if !projection.contains_member(peer) {
            return Err(FatalSyncError::PeerRevoked);
        }
        let known_author = projection.has_membership_history(author);
        let now = self.clock.now_unix_ms();
        if !known_author {
            return park_outcome(
                &mut state,
                workspace,
                package,
                peer,
                crate::synchronization::parking::ParkReason::UnknownAuthor,
                &operation,
                now,
            );
        }
        match crate::synchronization::insert_remote_operation(
            &mut state.connection,
            workspace,
            package,
            &operation,
        ) {
            Ok(crate::synchronization::RemoteInsert::Applied(record)) => {
                let record_id = record.record_id();
                if let Some(event) = crate::synchronization::event_for_record(
                    &state.connection,
                    workspace,
                    package,
                    record,
                ) {
                    if let Some(listeners) = state
                        .record_subscribers
                        .get_mut(&(workspace, package.to_owned()))
                    {
                        listeners.retain(|sender| sender.send(event.clone()).is_ok());
                    }
                } else {
                    eprintln!(
                        "remote record event unavailable: package={package} record={record_id}"
                    );
                }
                Ok(OperationOutcome::Applied)
            }
            Ok(crate::synchronization::RemoteInsert::Duplicate(_)) => {
                Ok(OperationOutcome::Duplicate)
            }
            Err(crate::synchronization::RemoteInsertError::MissingScope) => park_outcome(
                &mut state,
                workspace,
                package,
                peer,
                crate::synchronization::parking::ParkReason::MissingContentKey,
                &operation,
                now,
            ),
            Err(crate::synchronization::RemoteInsertError::KeyMismatch) => {
                Ok(OperationOutcome::Rejected(RejectReason::KeyMismatch))
            }
            Err(crate::synchronization::RemoteInsertError::WrongScope) => {
                Ok(OperationOutcome::Rejected(RejectReason::WrongScope))
            }
            Err(crate::synchronization::RemoteInsertError::Conflict) => {
                Ok(OperationOutcome::Rejected(RejectReason::Conflict))
            }
            Err(crate::synchronization::RemoteInsertError::Invalid) => {
                Ok(OperationOutcome::Rejected(RejectReason::Invalid))
            }
            Err(crate::synchronization::RemoteInsertError::Storage) => Err(FatalSyncError::Storage),
        }
    }
}

/// Parks one operation and reports the session outcome that follows.
#[allow(clippy::too_many_arguments)]
fn park_outcome(
    state: &mut RuntimeState,
    workspace: WorkspaceId,
    package: &str,
    peer: PublicIdentity,
    reason: crate::synchronization::parking::ParkReason,
    operation: &p2panda_core::Operation<()>,
    now_unix_ms: i64,
) -> Result<crate::synchronization::OperationOutcome, crate::synchronization::FatalSyncError> {
    use crate::synchronization::parking::{ParkResult, ParkingLimits};
    use crate::synchronization::{FatalSyncError, OperationOutcome, RejectReason};
    match crate::synchronization::parking::park(
        &mut state.connection,
        workspace,
        package,
        peer,
        reason,
        operation,
        now_unix_ms,
        ParkingLimits::CURRENT,
    ) {
        Ok(ParkResult::Parked | ParkResult::AlreadyParked) => Ok(OperationOutcome::Parked(reason)),
        Ok(ParkResult::QuotaExceeded) => Ok(OperationOutcome::ParkRefused),
        Err(crate::SynchronizationError::WrongScope) => {
            Ok(OperationOutcome::Rejected(RejectReason::WrongScope))
        }
        Err(crate::SynchronizationError::InvalidInput) => {
            Ok(OperationOutcome::Rejected(RejectReason::Invalid))
        }
        Err(_) => Err(FatalSyncError::Storage),
    }
}

pub(crate) struct RuntimeState {
    pub(crate) connection: rusqlite::Connection,
    pub(crate) peer_sessions: Arc<crate::synchronization::PeerSessions>,
    secret_key: iroh_base::SecretKey,
    display_name: String,
    pub(crate) memberships: BTreeMap<WorkspaceId, crate::membership::WorkspaceProjection>,
    subscribers: BTreeMap<WorkspaceId, Vec<MembershipSubscriber>>,
    record_subscribers: BTreeMap<(WorkspaceId, String), Vec<RecordSubscriber>>,
    next_subscriber_id: u64,
    pub(crate) poisoned: bool,
}

pub(crate) trait Clock: Send + Sync {
    fn now_unix_ms(&self) -> i64;
}

pub(crate) struct SystemClock;

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

    /// Opens storage and starts the authenticated onboarding transport.
    ///
    /// Opening does not wait for public relay readiness, so local commands remain
    /// available while offline. Call [`Self::shutdown`] and await it before exit.
    pub async fn open(config: CoreRuntimeConfig) -> Result<Self, CoreError> {
        Self::open_inner(config, Box::new(SystemClock), None).await
    }

    #[cfg(test)]
    async fn open_with_clock(
        config: CoreRuntimeConfig,
        clock: Box<dyn Clock>,
    ) -> Result<Self, CoreError> {
        Self::open_inner(config, clock, Some(true)).await
    }

    #[cfg(test)]
    async fn open_with_clock_and_readiness(
        config: CoreRuntimeConfig,
        clock: Box<dyn Clock>,
        ready_immediately: bool,
    ) -> Result<Self, CoreError> {
        Self::open_inner(config, clock, Some(ready_immediately)).await
    }

    async fn open_inner(
        config: CoreRuntimeConfig,
        clock: Box<dyn Clock>,
        #[cfg_attr(not(test), allow(unused_variables))] test_readiness: Option<bool>,
    ) -> Result<Self, CoreError> {
        let clock: Arc<dyn Clock> = Arc::from(clock);
        let opened_at_unix_ms = clock.now_unix_ms();
        // Opening storage and replaying the membership projection are both
        // blocking SQLite and signature work, so they run together on a
        // blocking thread and never on an asynchronous worker.
        let directory = config.private_data_directory().to_path_buf();
        let opened: Result<_, CoreError> = tokio::task::spawn_blocking(move || {
            let crate::storage::StoredRuntime {
                mut connection,
                secret_key,
                display_name,
            } = crate::storage::open(&directory)?;
            let memberships =
                crate::membership::projection::recover(&mut connection, opened_at_unix_ms)?;
            // Parking is bounded in time as well as in size: a peer that went
            // away never leaves work here for longer than the time-to-live.
            let _ = crate::synchronization::parking::purge_expired(
                &connection,
                opened_at_unix_ms,
                crate::synchronization::parking::ParkingLimits::CURRENT,
            );
            Ok((connection, secret_key, display_name, memberships))
        })
        .await
        .map_err(|_| CoreError::Storage(StorageError::Database))?;
        let (connection, secret_key, display_name, memberships) = opened?;
        let public_identity = PublicIdentity(secret_key.public());
        let peer_sessions = crate::synchronization::PeerSessions::new();
        let state = Arc::new(Mutex::new(RuntimeState {
            connection,
            peer_sessions: Arc::clone(&peer_sessions),
            secret_key: secret_key.clone(),
            display_name,
            memberships,
            subscribers: BTreeMap::new(),
            record_subscribers: BTreeMap::new(),
            next_subscriber_id: 0,
            poisoned: false,
        }));
        let issuer = crate::onboarding::handler::IssuerService::new(
            Arc::downgrade(&state),
            Arc::clone(&clock),
        );
        let demand = crate::membership_sync::MembershipDemand::new();
        let services = crate::synchronization::SyncServices {
            receiver: Arc::new(RuntimeSyncReceiver {
                state: Arc::downgrade(&state),
                clock: Arc::clone(&clock),
                demand: Arc::clone(&demand),
            }),
            sessions: Arc::clone(&peer_sessions),
        };
        // The composition point owns the membership protocol: `network` keeps
        // the Endpoint and Router and names no membership type.
        #[cfg(test)]
        let membership_paused = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let membership_replica: Arc<dyn crate::membership_sync::MembershipReplica> =
            Arc::new(replication::RuntimeMembershipReplica::new(
                Arc::downgrade(&state),
                Arc::clone(&clock),
                #[cfg(test)]
                Arc::clone(&membership_paused),
            ));
        let protocols = vec![(
            crate::membership_sync::MEMBERSHIP_SYNC_ALPN.to_vec(),
            Box::new(crate::membership_sync::MembershipSyncHandler::new(
                Arc::clone(&membership_replica),
            )) as Box<dyn iroh::protocol::DynProtocolHandler>,
        )];
        #[cfg(test)]
        let transport = if let Some(ready_immediately) = test_readiness {
            crate::onboarding::transport::Transport::bind_loopback_with_issuer_and_services(
                secret_key.clone(),
                std::time::Duration::from_millis(5),
                ready_immediately,
                issuer,
                services,
                protocols,
            )
            .await
        } else {
            crate::onboarding::transport::Transport::bind_production(
                secret_key.clone(),
                issuer,
                Some(services),
                protocols,
            )
            .await
        };
        #[cfg(not(test))]
        let transport = crate::onboarding::transport::Transport::bind_production(
            secret_key.clone(),
            issuer,
            Some(services),
            protocols,
        )
        .await;
        let transport = transport.map_err(CoreError::Onboarding)?;
        debug_assert_eq!(transport.endpoint_id(), secret_key.public());
        // Recovery has completed before the worker is started. The worker owns
        // wake consumption and scheduling, while all network awaits happen
        // outside the runtime state mutex.
        let sync_worker = crate::synchronization::SyncWorker::start(
            Arc::downgrade(&state),
            Arc::clone(&transport),
            public_identity,
            Arc::clone(&peer_sessions),
            Arc::clone(&membership_replica),
            Arc::clone(&demand),
        );
        Ok(Self {
            public_identity,
            state,
            clock,
            transport,
            sync_worker,
            membership_demand: demand,
            #[cfg(test)]
            membership_replica,
            #[cfg(test)]
            membership_paused,
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
    #[allow(dead_code)]
    pub(crate) fn issue_invitation(
        &self,
        workspace_id: WorkspaceId,
        expires_at_unix_ms: i64,
    ) -> Result<InvitationToken, InvitationError> {
        self.issue_invitation_at(workspace_id, self.clock.now_unix_ms(), expires_at_unix_ms)
    }

    fn issue_invitation_at(
        &self,
        workspace_id: WorkspaceId,
        issued_at_unix_ms: i64,
        expires_at_unix_ms: i64,
    ) -> Result<InvitationToken, InvitationError> {
        let mut state = self.lock_invitation_state()?;
        self.maintain_and_publish(&mut state, issued_at_unix_ms)
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
        if expires_at_unix_ms <= issued_at_unix_ms {
            return Err(InvitationError::Expired);
        }
        let nonce = SecretKey::generate().to_bytes();
        let token = crate::invitation::sign_token(
            secret_key,
            workspace_id,
            issued_at_unix_ms,
            expires_at_unix_ms,
            nonce,
        )?;
        Ok(InvitationToken::from_verified(&token))
    }

    /// Creates a fixed-lifetime opaque invitation after endpoint readiness succeeds.
    pub async fn create_workspace_invitation(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<crate::WorkspaceInvitation, crate::OnboardingError> {
        const INVITATION_LIFETIME_MS: i64 = 15 * 60 * 1_000;

        let operation = self.transport.begin_invitation().await?;
        #[cfg(test)]
        self.transport.pause_after_invitation_readiness().await;
        let issued_at_unix_ms = self.clock.now_unix_ms();
        let token = self
            .issue_invitation_at(
                workspace_id,
                issued_at_unix_ms,
                issued_at_unix_ms.saturating_add(INVITATION_LIFETIME_MS),
            )
            .map_err(map_invitation_onboarding_error)?;
        let verified = token
            .verified()
            .map_err(|_| crate::OnboardingError::Authentication)?;
        let invitation = crate::WorkspaceInvitation::from_parts(&verified, operation.address())
            .map_err(|_| crate::OnboardingError::InvalidInput);
        drop(operation);
        invitation
    }

    /// Joins the invited Workspace after authenticated transport and atomic local import.
    pub async fn join_workspace(
        &self,
        invitation: &crate::WorkspaceInvitation,
    ) -> Result<(WorkspaceSummary, MembershipSnapshot), crate::OnboardingError> {
        self.join_workspace_with_budget(
            invitation,
            crate::onboarding::transport::WHOLE_JOIN_TIMEOUT,
        )
        .await
    }

    async fn join_workspace_with_budget(
        &self,
        invitation: &crate::WorkspaceInvitation,
        budget: std::time::Duration,
    ) -> Result<(WorkspaceSummary, MembershipSnapshot), crate::OnboardingError> {
        let deadline = std::time::Instant::now()
            .checked_add(budget)
            .ok_or(crate::OnboardingError::Timeout)?;
        let permit = self.transport.begin_outbound().await?;
        let (verified, claim, response) = self
            .transport
            .whole_join_until(
                self.join_network_phase(invitation, deadline, &permit),
                deadline,
            )
            .await?;
        // Validation commits nothing, so the whole-join timer still bounds it.
        Self::ensure_join_deadline(deadline)?;
        let local_identity = self.public_identity;
        #[cfg(test)]
        let transport = Arc::clone(&self.transport);
        let validating = async move {
            tokio::task::spawn_blocking(move || {
                #[cfg(test)]
                transport.block_after_response();
                crate::membership::projection::validate_bootstrap(
                    response,
                    &verified.token,
                    &claim,
                    local_identity,
                )
            })
            .await
            .map_err(|_| crate::OnboardingError::InvalidBootstrap)?
        };
        let bootstrap = self
            .transport
            .whole_join_until(validating, deadline)
            .await?;
        // The import runs outside the timer: a timer must never report
        // `Timeout` for an import that then commits. The closure enforces the
        // deadline itself, and the outbound permit is held until it finishes.
        Self::ensure_join_deadline(deadline)?;
        let state = Arc::clone(&self.state);
        let imported = tokio::task::spawn_blocking(move || {
            let state = match crate::blocking::lock_until(&state, deadline) {
                Ok(state) => state,
                Err(crate::blocking::LockUntilError::Timeout) => {
                    return Err(crate::OnboardingError::Timeout);
                }
                Err(crate::blocking::LockUntilError::Poisoned) => {
                    return Err(crate::OnboardingError::UncertainStorageOutcome);
                }
            };
            import_validated_bootstrap_locked(state, &bootstrap, Some(deadline), |_| Ok(()))
        })
        .await
        .map_err(|_| crate::OnboardingError::UncertainStorageOutcome)?;
        drop(permit);
        imported
    }

    /// The network half of a join: the pre-network claim and the exchange.
    ///
    /// Everything here is bounded by the whole-join timer, because nothing here
    /// commits.
    async fn join_network_phase(
        &self,
        invitation: &crate::WorkspaceInvitation,
        deadline: std::time::Instant,
        permit: &crate::onboarding::transport::OutboundPermit,
    ) -> Result<
        (
            crate::onboarding::transfer::VerifiedWorkspaceInvitation,
            crate::invitation::VerifiedInvitationClaim,
            crate::onboarding::wire::SuccessResponse,
        ),
        crate::OnboardingError,
    > {
        let verified = invitation
            .verified()
            .map_err(|_| crate::OnboardingError::InvalidInput)?;
        Self::ensure_join_deadline(deadline)?;
        let state = Arc::clone(&self.state);
        let token = verified.token.clone();
        let claim = tokio::task::spawn_blocking(move || {
            let state = match crate::blocking::lock_until(&state, deadline) {
                Ok(state) => state,
                Err(crate::blocking::LockUntilError::Timeout) => {
                    return Err(crate::OnboardingError::Timeout);
                }
                Err(crate::blocking::LockUntilError::Poisoned) => {
                    return Err(crate::OnboardingError::UncertainStorageOutcome);
                }
            };
            if state.poisoned {
                return Err(crate::OnboardingError::UncertainStorageOutcome);
            }
            let local_creator: Option<Vec<u8>> = state
                .connection
                .query_row(
                    "SELECT creator_public_identity FROM workspaces WHERE workspace_id = ?1",
                    [token.workspace_id.as_bytes().as_slice()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| crate::OnboardingError::LocalConflict)?;
            if matches!(local_creator, Some(creator) if creator != token.inviter.as_bytes()) {
                return Err(crate::OnboardingError::LocalConflict);
            }
            crate::invitation::sign_claim(&state.secret_key, &token)
                .map_err(map_invitation_onboarding_error)
        })
        .await
        .map_err(|_| crate::OnboardingError::UncertainStorageOutcome)??;
        Self::ensure_join_deadline(deadline)?;

        let response = crate::onboarding::client::join(
            &self.transport,
            permit,
            verified.clone(),
            claim.clone(),
        )
        .await?;
        #[cfg(test)]
        self.transport.pause_after_response().await;
        Ok((verified, claim, response))
    }

    fn ensure_join_deadline(deadline: std::time::Instant) -> Result<(), crate::OnboardingError> {
        if std::time::Instant::now() < deadline {
            Ok(())
        } else {
            Err(crate::OnboardingError::Timeout)
        }
    }

    #[cfg(test)]
    fn import_validated_bootstrap(
        &self,
        bootstrap: &crate::membership::projection::ValidatedBootstrap,
        deadline: Option<std::time::Instant>,
        fault: impl FnMut(crate::membership::projection::ImportBoundary) -> Result<(), StorageError>,
    ) -> Result<(WorkspaceSummary, MembershipSnapshot), crate::OnboardingError> {
        if let Some(deadline) = deadline {
            Self::ensure_join_deadline(deadline)?;
        }
        let state = self
            .state
            .lock()
            .map_err(|_| crate::OnboardingError::UncertainStorageOutcome)?;
        import_validated_bootstrap_locked(state, bootstrap, deadline, fault)
    }

    #[cfg(test)]
    pub(crate) fn create_workspace_invitation_for_test(
        &self,
        workspace_id: WorkspaceId,
        issued_at_unix_ms: i64,
        expires_at_unix_ms: i64,
    ) -> Result<crate::WorkspaceInvitation, crate::OnboardingError> {
        let token = self
            .issue_invitation_at(workspace_id, issued_at_unix_ms, expires_at_unix_ms)
            .map_err(map_invitation_onboarding_error)?;
        crate::WorkspaceInvitation::from_parts(
            &token
                .verified()
                .map_err(|_| crate::OnboardingError::Authentication)?,
            self.transport.endpoint_address(),
        )
        .map_err(|_| crate::OnboardingError::InvalidInput)
    }

    #[cfg(test)]
    pub(crate) fn create_claim_for_invitation_test(
        &self,
        invitation: &crate::WorkspaceInvitation,
    ) -> Result<InvitationClaim, crate::OnboardingError> {
        let invitation = invitation
            .verified()
            .map_err(|_| crate::OnboardingError::InvalidInput)?;
        let state = self
            .state
            .lock()
            .map_err(|_| crate::OnboardingError::UncertainStorageOutcome)?;
        let claim = crate::invitation::sign_claim(&state.secret_key, &invitation.token)
            .map_err(map_invitation_onboarding_error)?;
        Ok(InvitationClaim::from_verified(&claim))
    }

    #[cfg(test)]
    pub(crate) fn export_current_bootstrap_for_test(
        &self,
        workspace_id: WorkspaceId,
        admission_id: MembershipOperationId,
    ) -> Result<crate::onboarding::wire::SuccessResponse, crate::OnboardingError> {
        let state = self
            .state
            .lock()
            .map_err(|_| crate::OnboardingError::UncertainStorageOutcome)?;
        let projection = state
            .memberships
            .get(&workspace_id)
            .ok_or(crate::OnboardingError::LocalConflict)?;
        let frame = crate::membership::projection::encode_current_bootstrap_with_limits(
            &state.connection,
            projection,
            admission_id,
            crate::membership::projection::BootstrapLimits::CURRENT,
        )
        .map_err(|_| crate::OnboardingError::InvalidBootstrap)?;
        crate::onboarding::wire::decode_success(&frame)
            .map_err(|_| crate::OnboardingError::InvalidBootstrap)
    }

    #[cfg(test)]
    pub(crate) fn import_response_with_deadline_delay_for_test(
        &self,
        invitation: &crate::WorkspaceInvitation,
        response: crate::onboarding::wire::SuccessResponse,
        boundary: crate::membership::projection::ImportBoundary,
        delay: std::time::Duration,
        budget: std::time::Duration,
    ) -> Result<(WorkspaceSummary, MembershipSnapshot), crate::OnboardingError> {
        let invitation = invitation
            .verified()
            .map_err(|_| crate::OnboardingError::InvalidInput)?;
        let claim = {
            let state = self
                .state
                .lock()
                .map_err(|_| crate::OnboardingError::UncertainStorageOutcome)?;
            crate::invitation::sign_claim(&state.secret_key, &invitation.token)
                .map_err(map_invitation_onboarding_error)?
        };
        let bootstrap = crate::membership::projection::validate_bootstrap(
            response,
            &invitation.token,
            &claim,
            self.public_identity,
        )?;
        let deadline = std::time::Instant::now()
            .checked_add(budget)
            .ok_or(crate::OnboardingError::Timeout)?;
        self.import_validated_bootstrap(&bootstrap, Some(deadline), |current| {
            if current == boundary {
                std::thread::sleep(delay);
            }
            Ok(())
        })
    }

    #[cfg(test)]
    pub(crate) fn import_response_for_test(
        &self,
        invitation: &crate::WorkspaceInvitation,
        response: crate::onboarding::wire::SuccessResponse,
        boundary: Option<crate::membership::projection::ImportBoundary>,
    ) -> Result<(WorkspaceSummary, MembershipSnapshot), crate::OnboardingError> {
        let invitation = invitation
            .verified()
            .map_err(|_| crate::OnboardingError::InvalidInput)?;
        let claim = {
            let state = self
                .state
                .lock()
                .map_err(|_| crate::OnboardingError::UncertainStorageOutcome)?;
            crate::invitation::sign_claim(&state.secret_key, &invitation.token)
                .map_err(map_invitation_onboarding_error)?
        };
        let bootstrap = crate::membership::projection::validate_bootstrap(
            response,
            &invitation.token,
            &claim,
            self.public_identity,
        )?;
        self.import_validated_bootstrap(&bootstrap, None, |current| {
            if boundary == Some(current) {
                Err(
                    if current == crate::membership::projection::ImportBoundary::AfterCommit {
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

    /// Appends one authenticated, protected immutable record to the workspace journal.
    pub fn append_record(
        &self,
        workspace_id: WorkspaceId,
        package_identity: &str,
        purpose: &str,
        canonical: &[u8],
    ) -> Result<crate::ImmutableRecord, crate::SynchronizationError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| crate::SynchronizationError::RuntimeUnavailable)?;
        if state.poisoned {
            return Err(crate::SynchronizationError::RuntimeUnavailable);
        }
        let is_member = state
            .memberships
            .get(&workspace_id)
            .is_some_and(|projection| {
                projection
                    .snapshot()
                    .members()
                    .iter()
                    .any(|member| member.identity() == self.public_identity)
            });
        let is_creator = state
            .memberships
            .get(&workspace_id)
            .is_some_and(|projection| projection.creator() == self.public_identity);
        let secret_key = state.secret_key.clone();
        let record = crate::synchronization::append(
            &mut state.connection,
            &secret_key,
            workspace_id,
            package_identity,
            purpose,
            canonical,
            is_member,
            is_creator,
        )?;
        if let Some(event) = crate::synchronization::event_for_record(
            &state.connection,
            workspace_id,
            package_identity,
            record.clone(),
        ) && let Some(listeners) = state
            .record_subscribers
            .get_mut(&(workspace_id, package_identity.to_owned()))
        {
            listeners.retain(|sender| sender.send(event.clone()).is_ok());
        }
        self.sync_worker.wake();
        Ok(record)
    }

    /// Creates the welcome bundle a current recipient needs for one scope.
    ///
    /// The grant is HPKE-sealed to `recipient` and signed by this Installation,
    /// so only the recipient's private key recovers the content key; the origin
    /// is the Workspace creator's `create` control, which authorizes the key.
    #[cfg(test)]
    pub(crate) fn export_protection_bundle_for_test(
        &self,
        workspace_id: WorkspaceId,
        package_identity: &str,
        recipient: PublicIdentity,
    ) -> Result<(Vec<u8>, Vec<u8>), crate::SynchronizationError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| crate::SynchronizationError::RuntimeUnavailable)?;
        if state.poisoned {
            return Err(crate::SynchronizationError::RuntimeUnavailable);
        }
        let projection = state
            .memberships
            .get(&workspace_id)
            .ok_or(crate::SynchronizationError::WorkspaceNotFound)?;
        if !projection.contains_member(self.public_identity)
            || !projection.contains_member(recipient)
        {
            return Err(crate::SynchronizationError::NotCurrentMember);
        }
        let admission = *projection.snapshot().version().as_bytes();
        let scope = crate::synchronization::scope_digest(package_identity, workspace_id);
        let origin: Vec<u8> = state
            .connection
            .query_row(
                "SELECT control_bytes FROM protection_control_operations
                 WHERE scope_id=?1 AND operation_kind='create' LIMIT 1",
                [scope.as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| crate::SynchronizationError::Storage(StorageError::Database))?
            .ok_or(crate::SynchronizationError::ContentKeyUnavailable)?;
        let secret_key = state.secret_key.clone();
        let grant = crate::synchronization::export_protection_grant(
            &mut state.connection,
            &secret_key,
            workspace_id,
            package_identity,
            recipient,
            admission,
        )?;
        Ok((origin, grant))
    }

    /// Imports a welcome bundle that traces to the Workspace creator's origin.
    #[cfg(test)]
    pub(crate) fn import_protection_grant(
        &self,
        workspace_id: WorkspaceId,
        package_identity: &str,
        origin: &[u8],
        grant: &[u8],
    ) -> Result<usize, crate::SynchronizationError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| crate::SynchronizationError::RuntimeUnavailable)?;
        if state.poisoned {
            return Err(crate::SynchronizationError::RuntimeUnavailable);
        }
        let author = crate::synchronization::protection_control_author(grant)?;
        let projection = state
            .memberships
            .get(&workspace_id)
            .ok_or(crate::SynchronizationError::WorkspaceNotFound)?;
        if !projection.contains_member(author) {
            return Err(crate::SynchronizationError::NotCurrentMember);
        }
        let creator = projection.creator();
        let secret = state.secret_key.clone();
        crate::synchronization::import_protection_grant(
            &mut state.connection,
            &secret,
            workspace_id,
            package_identity,
            creator,
            origin,
            grant,
        )
    }

    /// Opens sealed records in request order after authenticating their scope and contents.
    pub fn open_records(
        &self,
        workspace_id: WorkspaceId,
        package_identity: &str,
        sealed_records: &[Vec<u8>],
    ) -> Result<Vec<crate::OpenedRecord>, crate::SynchronizationError> {
        let state = self
            .state
            .lock()
            .map_err(|_| crate::SynchronizationError::RuntimeUnavailable)?;
        if state.poisoned {
            return Err(crate::SynchronizationError::RuntimeUnavailable);
        }
        crate::synchronization::open_records(
            &state.connection,
            workspace_id,
            package_identity,
            sealed_records,
        )
    }

    /// Replays the installation-local immutable-record journal from an optional cursor.
    pub fn subscribe_records(
        &self,
        workspace_id: WorkspaceId,
        package_identity: &str,
        after: Option<crate::Cursor>,
    ) -> Result<crate::RecordSubscription, crate::SynchronizationError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| crate::SynchronizationError::RuntimeUnavailable)?;
        if state.poisoned {
            return Err(crate::SynchronizationError::RuntimeUnavailable);
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        let subscription = crate::synchronization::subscribe(
            &state.connection,
            workspace_id,
            package_identity,
            after,
            receiver,
        )?;
        state
            .record_subscribers
            .entry((workspace_id, package_identity.to_owned()))
            .or_default()
            .push(sender);
        Ok(subscription)
    }

    /// Reconciles the locally accepted immutable records with a current peer.
    ///
    /// The network operation is finite and bounded. The runtime releases its
    /// local state lock before dialing, so replication never blocks local
    /// commands on network I/O.
    pub async fn replicate_records_to(
        &self,
        workspace_id: WorkspaceId,
        package_identity: &str,
        peer: PublicIdentity,
    ) -> Result<usize, crate::SynchronizationError> {
        {
            let state = self
                .state
                .lock()
                .map_err(|_| crate::SynchronizationError::RuntimeUnavailable)?;
            if state.poisoned {
                return Err(crate::SynchronizationError::RuntimeUnavailable);
            }
            let current = state
                .memberships
                .get(&workspace_id)
                .is_some_and(|projection| projection.contains_member(self.public_identity));
            if !current {
                return Err(crate::SynchronizationError::NotCurrentMember);
            }
        }
        let receiver: Arc<dyn crate::synchronization::SyncReceiver> =
            Arc::new(RuntimeSyncReceiver {
                state: Arc::downgrade(&self.state),
                clock: Arc::clone(&self.clock),
                demand: Arc::clone(&self.membership_demand),
            });
        let address = iroh_base::EndpointAddr::new(peer.as_public_key());
        let accepted = self
            .transport
            .send_log_sync(
                address,
                receiver,
                workspace_id,
                package_identity.to_owned(),
                peer,
            )
            .await
            .map_err(map_log_session_error)?;
        Ok(accepted)
    }

    #[cfg(test)]
    async fn replicate_records_to_address_for_test(
        &self,
        workspace_id: WorkspaceId,
        package_identity: &str,
        address: iroh_base::EndpointAddr,
        peer: PublicIdentity,
    ) -> Result<usize, crate::synchronization::LogSessionError> {
        let local = self.public_identity;
        let state = Arc::downgrade(&self.state);
        let current = crate::blocking::run_with_state(&state, move |state| {
            state
                .memberships
                .get(&workspace_id)
                .is_some_and(|projection| projection.contains_member(local))
        })
        .await
        .unwrap_or(false);
        if !current {
            return Err(crate::synchronization::LogSessionError::Refused(
                crate::synchronization::Refusal::NotAuthorized,
            ));
        }
        let receiver: Arc<dyn crate::synchronization::SyncReceiver> =
            Arc::new(RuntimeSyncReceiver {
                state: Arc::downgrade(&self.state),
                clock: Arc::clone(&self.clock),
                demand: Arc::clone(&self.membership_demand),
            });
        self.transport
            .send_log_sync(
                address,
                receiver,
                workspace_id,
                package_identity.to_owned(),
                peer,
            )
            .await
    }

    /// Stops accepting onboarding and synchronization work and awaits closure.
    ///
    /// Local identity, Workspace, and membership commands remain available.
    pub async fn shutdown(&self) -> Result<(), crate::OnboardingError> {
        // Stop the scheduler and await every session before Router/Endpoint
        // shutdown. This ordering prevents SQLite callbacks from racing the
        // shared network teardown.
        self.sync_worker.shutdown().await;
        self.transport.shutdown().await
    }

    /// Creates proof that this installation claims the supplied invitation.
    #[allow(dead_code)]
    pub(crate) fn create_invitation_claim(
        &self,
        token: &InvitationToken,
    ) -> Result<InvitationClaim, InvitationError> {
        let token = token.verified()?;
        let state = self.lock_invitation_state()?;
        let claim = crate::invitation::sign_claim(&state.secret_key, &token)?;
        Ok(InvitationClaim::from_verified(&claim))
    }

    /// Redeems a claim on the installation that issued its token.
    #[allow(dead_code)]
    pub(crate) fn redeem_invitation(
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
        let authenticated_peer = claim.invitee;
        let result = crate::membership::projection::redeem_invitation_with_fault(
            connection,
            memberships,
            secret_key,
            claim,
            authenticated_peer,
            now_unix_ms,
            fault,
        );
        match result {
            Ok(export) => {
                let accepted = export.applied.accepted;
                let transition = Self::transition_from_applied(export.applied).ok_or(
                    InvitationError::Membership(MembershipError::ConflictingGenesis),
                )?;
                if accepted {
                    replication::after_membership_accepted(&mut state, &transition);
                }
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
    pub(crate) async fn open_for_onboarding_test(
        config: CoreRuntimeConfig,
    ) -> Result<Self, CoreError> {
        Self::open_with_clock(config, Box::new(SystemClock)).await
    }

    #[cfg(test)]
    pub(crate) fn drop_next_onboarding_response_for_test(&self) {
        self.transport.handler().drop_next_response();
    }

    #[cfg(test)]
    pub(crate) fn endpoint_address_for_test(&self) -> iroh_base::EndpointAddr {
        self.transport.endpoint_address()
    }

    /// Teaches this loopback runtime how to reach a peer it has never dialled.
    #[cfg(test)]
    pub(crate) fn remember_peer_address_for_test(&self, address: iroh_base::EndpointAddr) {
        self.transport.remember_peer_address(address);
    }

    /// Every accepted membership operation identifier this Installation holds.
    #[cfg(test)]
    pub(crate) fn membership_operation_ids_for_test(
        &self,
        workspace_id: WorkspaceId,
    ) -> BTreeSet<MembershipOperationId> {
        let state = self.state.lock().expect("test runtime state");
        state
            .memberships
            .get(&workspace_id)
            .map(|projection| {
                crate::membership::replication::operation_ids(projection)
                    .into_iter()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Stops this Installation replicating membership in either direction.
    ///
    /// Every replica call reports the runtime as unavailable, which is what an
    /// Installation that cannot currently answer looks like on the wire.
    #[cfg(test)]
    pub(crate) fn pause_membership_replication_for_test(&self, paused: bool) {
        self.membership_paused
            .store(paused, std::sync::atomic::Ordering::Release);
    }

    /// Runs one real membership session against `address`, as the worker would.
    #[cfg(test)]
    pub(crate) async fn membership_session_for_test(
        &self,
        address: iroh_base::EndpointAddr,
        workspace_id: WorkspaceId,
        peer: PublicIdentity,
    ) -> Result<
        crate::membership_sync::MembershipSessionReport,
        crate::membership_sync::MembershipSessionError,
    > {
        self.transport
            .run_membership_session(
                address,
                Arc::clone(&self.membership_replica),
                workspace_id,
                peer,
            )
            .await
    }

    /// Dials the membership protocol pushing nothing, and reports what the
    /// responder served and whether it refused.
    #[cfg(test)]
    pub(crate) async fn probe_membership_sync_for_test(
        &self,
        address: iroh_base::EndpointAddr,
        workspace_id: WorkspaceId,
        operation_ids: Vec<[u8; 32]>,
    ) -> (Vec<Vec<u8>>, bool) {
        crate::membership_sync::session::probe_membership_sync(
            self.transport.endpoint(),
            address,
            workspace_id,
            operation_ids,
        )
        .await
    }

    /// How many operations one scope currently holds parked.
    #[cfg(test)]
    pub(crate) fn parked_count_for_test(&self, workspace_id: WorkspaceId, package: &str) -> usize {
        let state = self.state.lock().expect("test runtime state");
        crate::synchronization::parking::parked_count(&state.connection, workspace_id, package)
            .expect("a parked count")
    }

    /// Runs one synchronization pass now instead of waiting for the idle tick.
    #[cfg(test)]
    pub(crate) fn sync_now_for_test(&self) {
        self.sync_worker.wake();
    }

    /// Dials a peer's package synchronization ALPN as this Installation and
    /// reports whether the peer's gate served it anything.
    #[cfg(test)]
    pub(crate) async fn probe_package_sync_for_test(
        &self,
        address: iroh_base::EndpointAddr,
        workspace_id: WorkspaceId,
        package_identity: &str,
        peer: PublicIdentity,
    ) -> ProbeOutcome {
        let receiver: Arc<dyn crate::synchronization::SyncReceiver> = Arc::new(ProbeReceiver);
        let sessions = Arc::clone(&self.state.lock().expect("test runtime state").peer_sessions);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            crate::synchronization::network_handler::outbound_log_session(
                self.transport.endpoint(),
                address,
                receiver,
                sessions,
                workspace_id,
                package_identity.to_owned(),
                peer,
            ),
        )
        .await;
        match result {
            Ok(Ok(operations_received)) => ProbeOutcome::Welcomed {
                operations_received,
            },
            Ok(Err(crate::synchronization::LogSessionError::Refused(refusal))) => {
                ProbeOutcome::Refused(refusal)
            }
            Ok(Err(error)) => ProbeOutcome::Failed(error.to_string()),
            Err(_) => ProbeOutcome::Failed("probe timed out".to_owned()),
        }
    }

    /// Runs one real outbound session that also pushes `extra` operations.
    ///
    /// The extra operations join this Installation's own, so a test can deliver
    /// a forged record beside genuine ones in a single session.
    #[cfg(test)]
    pub(crate) async fn deliver_operations_for_test(
        &self,
        address: iroh_base::EndpointAddr,
        workspace_id: WorkspaceId,
        package_identity: &str,
        peer: PublicIdentity,
        extra: Vec<p2panda_core::Operation<()>>,
    ) -> Result<usize, crate::synchronization::LogSessionError> {
        let log_id = crate::synchronization::operation_log_id(
            crate::synchronization::scope_digest(package_identity, workspace_id),
        );
        let sessions = Arc::clone(&self.state.lock().expect("test runtime state").peer_sessions);
        let receiver: Arc<dyn crate::synchronization::SyncReceiver> = Arc::new(InjectingReceiver {
            inner: RuntimeSyncReceiver {
                state: Arc::downgrade(&self.state),
                clock: Arc::clone(&self.clock),
                demand: Arc::clone(&self.membership_demand),
            },
            extra: extra
                .into_iter()
                .map(|operation| (log_id, operation))
                .collect(),
        });
        crate::synchronization::network_handler::outbound_log_session(
            self.transport.endpoint(),
            address,
            receiver,
            sessions,
            workspace_id,
            package_identity.to_owned(),
            peer,
        )
        .await
    }

    /// Opens a package synchronization session and then stops talking.
    ///
    /// The responder is left waiting for the rest of the first frame, which is
    /// where a test can observe what ends an inbound session that is doing
    /// nothing wrong. Returns the application close code the responder used.
    #[cfg(test)]
    pub(crate) async fn stalled_package_sync_close_code_for_test(
        &self,
        address: iroh_base::EndpointAddr,
        wait: std::time::Duration,
    ) -> Option<u64> {
        let connection = self
            .transport
            .endpoint()
            .connect(address, crate::network::PACKAGE_SYNC_ALPN)
            .await
            .ok()?;
        let (mut send, _receive) = connection.open_bi().await.ok()?;
        // A length header with no body: the stream exists, so the responder
        // accepts it, and then waits for bytes that never arrive.
        send.write_all(&64_u32.to_be_bytes()).await.ok()?;
        let closed = tokio::time::timeout(wait, connection.closed()).await.ok()?;
        match closed {
            iroh::endpoint::ConnectionError::ApplicationClosed(close) => {
                Some(u64::from(close.error_code))
            }
            _ => None,
        }
    }

    /// Reports whether a peer's Router still offers `alpn` at all.
    #[cfg(test)]
    pub(crate) async fn can_connect_alpn_for_test(
        &self,
        address: iroh_base::EndpointAddr,
        alpn: &[u8],
    ) -> bool {
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.transport.endpoint().connect(address, alpn),
        )
        .await
        .is_ok_and(|connection| connection.is_ok())
    }

    #[cfg(test)]
    pub(crate) fn timeout_next_join_for_test(&self) {
        self.transport.force_next_join_timeout();
    }

    #[cfg(test)]
    pub(crate) fn invalidate_next_bootstrap_for_test(&self) {
        self.transport.force_next_invalid_bootstrap();
    }

    #[cfg(test)]
    pub(crate) fn pause_next_join_after_response_for_test(&self) {
        self.transport.force_next_post_response_pause();
    }

    #[cfg(test)]
    pub(crate) fn hold_next_join_after_response_for_test(&self) {
        self.transport.hold_next_join_after_response();
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_join_held_after_response_for_test(&self) {
        self.transport.wait_for_post_response_hold().await;
    }

    #[cfg(test)]
    pub(crate) fn release_join_held_after_response_for_test(&self) {
        self.transport.release_post_response_hold();
    }

    #[cfg(test)]
    pub(crate) fn block_next_join_after_response_for_test(&self, duration: std::time::Duration) {
        self.transport.force_next_post_response_block(duration);
    }

    #[cfg(test)]
    pub(crate) async fn join_workspace_with_budget_for_test(
        &self,
        invitation: &crate::WorkspaceInvitation,
        budget: std::time::Duration,
    ) -> Result<(WorkspaceSummary, MembershipSnapshot), crate::OnboardingError> {
        self.join_workspace_with_budget(invitation, budget).await
    }

    #[cfg(test)]
    pub(crate) fn hold_runtime_state_for_test(&self) -> RuntimeStateHold {
        self.hold_state_inner()
    }

    /// Holds the runtime state lock until the returned value is released, so a
    /// test in another crate can observe the Host while Core is busy.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn hold_state_for_tests(&self) -> RuntimeStateHold {
        self.hold_state_inner()
    }

    #[cfg(any(test, feature = "test-hooks"))]
    fn hold_state_inner(&self) -> RuntimeStateHold {
        let state = Arc::clone(&self.state);
        let (acquired_sender, acquired_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let _state = state.lock().unwrap();
            acquired_sender.send(()).unwrap();
            release_receiver.recv().unwrap();
        });
        acquired_receiver.recv().unwrap();
        RuntimeStateHold {
            release: Some(release_sender),
            thread: Some(thread),
        }
    }

    #[cfg(test)]
    pub(crate) async fn exhaust_outbound_joins_for_test(
        &self,
    ) -> Vec<crate::onboarding::transport::OutboundPermit> {
        let mut permits = Vec::new();
        for _ in 0..crate::onboarding::transport::MAX_OUTBOUND_JOINS {
            permits.push(self.transport.begin_outbound().await.expect("test permit"));
        }
        permits
    }

    #[cfg(test)]
    pub(crate) async fn hold_outbound_join_for_test(
        &self,
    ) -> Result<crate::onboarding::transport::OutboundPermit, crate::OnboardingError> {
        self.transport.begin_outbound().await
    }

    #[cfg(test)]
    pub(crate) fn onboarding_lifecycle_state_for_test(&self) -> u8 {
        self.transport.lifecycle_state()
    }

    #[cfg(test)]
    pub(crate) fn active_onboarding_operations_for_test(&self) -> usize {
        self.transport.active_operations()
    }

    #[cfg(test)]
    pub(crate) fn available_outbound_joins_for_test(&self) -> usize {
        self.transport.available_outbound_permits()
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_shutdown_idle_waiter_for_test(&self) {
        self.transport.wait_for_idle_waiter().await;
    }

    #[cfg(test)]
    pub(crate) async fn send_raw_onboarding_frame_for_test(
        &self,
        address: iroh_base::EndpointAddr,
        frame: &[u8],
    ) -> Result<Vec<u8>, crate::OnboardingError> {
        let permit = self.transport.begin_outbound().await?;
        let connection = self.transport.connect(address, &permit).await?;
        let (mut send, mut receive) = connection
            .open_bi()
            .await
            .map_err(|_| crate::OnboardingError::Authentication)?;
        send.write_all(frame)
            .await
            .map_err(|_| crate::OnboardingError::Authentication)?;
        send.finish()
            .map_err(|_| crate::OnboardingError::Authentication)?;
        receive
            .read_to_end(1_024 + 4)
            .await
            .map_err(|_| crate::OnboardingError::Overload)
    }

    #[cfg(test)]
    pub(crate) fn create_workspace_invitation_for_workspace_id_for_test(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<crate::WorkspaceInvitation, crate::OnboardingError> {
        let state = self
            .state
            .lock()
            .map_err(|_| crate::OnboardingError::UncertainStorageOutcome)?;
        let issued_at_unix_ms = self.clock.now_unix_ms();
        let token = crate::invitation::sign_token(
            &state.secret_key,
            workspace_id,
            issued_at_unix_ms,
            issued_at_unix_ms.saturating_add(60_000),
            SecretKey::generate().to_bytes(),
        )
        .map_err(map_invitation_onboarding_error)?;
        crate::WorkspaceInvitation::from_parts(&token, self.transport.endpoint_address())
            .map_err(|_| crate::OnboardingError::InvalidInput)
    }

    #[cfg(test)]
    pub(crate) fn admit_and_export_for_test(
        &self,
        claim: &InvitationClaim,
        authenticated_peer: PublicIdentity,
        boundary: Option<crate::membership::projection::RedemptionBoundary>,
    ) -> Result<Vec<u8>, crate::OnboardingError> {
        self.admit_and_export_with_limits_for_test(
            claim,
            authenticated_peer,
            crate::membership::projection::BootstrapLimits::CURRENT,
            boundary,
        )
    }

    #[cfg(test)]
    pub(crate) fn admit_and_export_with_limits_for_test(
        &self,
        claim: &InvitationClaim,
        authenticated_peer: PublicIdentity,
        limits: crate::membership::projection::BootstrapLimits,
        boundary: Option<crate::membership::projection::RedemptionBoundary>,
    ) -> Result<Vec<u8>, crate::OnboardingError> {
        let claim = claim.verified().map_err(map_invitation_onboarding_error)?;
        let mut state = self
            .lock_invitation_state()
            .map_err(map_invitation_onboarding_error)?;
        let now_unix_ms = self.clock.now_unix_ms();
        let RuntimeState {
            connection,
            secret_key,
            memberships,
            poisoned,
            ..
        } = &mut *state;
        let result = crate::membership::projection::redeem_invitation_with_fault_and_limits(
            connection,
            memberships,
            secret_key,
            claim,
            crate::membership::projection::AdmissionContext {
                authenticated_peer,
                now_unix_ms,
                limits,
            },
            |current| {
                if boundary == Some(current) {
                    Err(
                        if current == crate::membership::projection::RedemptionBoundary::AfterCommit
                        {
                            StorageError::UncertainOutcome
                        } else {
                            StorageError::Database
                        },
                    )
                } else {
                    Ok(())
                }
            },
        );
        match result {
            Ok(export) => Ok(export.response),
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
                Err(map_invitation_onboarding_error(error))
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
        self.revoke_member_with_limits(
            workspace_id,
            member,
            crate::membership::projection::BootstrapLimits::CURRENT,
        )
    }

    #[cfg(test)]
    pub(crate) fn revoke_member_with_limits_for_test(
        &self,
        workspace_id: WorkspaceId,
        member: PublicIdentity,
        limits: crate::membership::projection::BootstrapLimits,
    ) -> Result<MembershipTransition, MembershipError> {
        self.revoke_member_with_limits(workspace_id, member, limits)
    }

    fn revoke_member_with_limits(
        &self,
        workspace_id: WorkspaceId,
        member: PublicIdentity,
        limits: crate::membership::projection::BootstrapLimits,
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
        match crate::membership::projection::apply_canonical_with_limits(
            connection,
            memberships,
            envelope.canonical_bytes(),
            now_unix_ms,
            limits,
        ) {
            Ok(applied) if applied.accepted => {
                let transition = Self::transition_from_applied(applied)
                    .ok_or(MembershipError::ConflictingGenesis)?;
                // A revoked Installation must stop being served immediately,
                // not at the end of whatever session is already running.
                replication::after_membership_accepted(&mut state, &transition);
                self.sync_worker.wake();
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
        replication::maintain_and_publish(state, now_unix_ms)
    }

    /// Returns `None` when the causing operation is a workspace `create`, which
    /// starts membership instead of transitioning it.
    fn transition_from_applied(
        applied: crate::membership::projection::AppliedMembership,
    ) -> Option<MembershipTransition> {
        Some(MembershipTransition::new(
            applied.snapshot.workspace_id(),
            applied.snapshot.epoch(),
            applied.snapshot.version(),
            applied.operation_id,
            applied.snapshot_digest,
            applied.change?,
            applied.author,
        ))
    }

    fn publish_transition(state: &mut RuntimeState, transition: MembershipTransition) {
        if let Some(listeners) = state.subscribers.get_mut(&transition.workspace_id()) {
            listeners.retain(|listener| listener.sender.send(transition.clone()).is_ok());
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
                if applied.accepted
                    && let Some(change) = applied.change
                {
                    let transition = MembershipTransition::new(
                        applied.snapshot.workspace_id(),
                        applied.snapshot.epoch(),
                        applied.snapshot.version(),
                        applied.operation_id,
                        applied.snapshot_digest,
                        change,
                        applied.author,
                    );
                    replication::after_membership_accepted(&mut state, &transition);
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
        self.subscribe_membership_inner(workspace_id, after_epoch, None)
    }

    /// Subscribes with at most 32 durable catch-up transitions held in memory.
    ///
    /// Bounded consumers reopen after their last delivered epoch to continue
    /// catch-up without owning an unbounded live receiver or durable buffer.
    pub fn subscribe_membership_bounded(
        &self,
        workspace_id: WorkspaceId,
        after_epoch: MembershipEpoch,
        max_catch_up: usize,
    ) -> Result<MembershipSubscription, SubscriptionError> {
        self.subscribe_membership_inner(workspace_id, after_epoch, Some(max_catch_up.clamp(1, 32)))
    }

    fn subscribe_membership_inner(
        &self,
        workspace_id: WorkspaceId,
        after_epoch: MembershipEpoch,
        catch_up_limit: Option<usize>,
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
        state.next_subscriber_id = state.next_subscriber_id.saturating_add(1);
        let subscriber_id = state.next_subscriber_id;
        state
            .subscribers
            .entry(workspace_id)
            .or_default()
            .push(MembershipSubscriber {
                id: subscriber_id,
                sender,
            });
        let durable = match catch_up_limit {
            Some(limit) => crate::subscription::load_after_bounded(
                &state.connection,
                workspace_id,
                after_epoch,
                limit,
            ),
            None => crate::subscription::load_after(&state.connection, workspace_id, after_epoch),
        };
        let durable = match durable {
            Ok(durable) => durable,
            Err(error) => {
                if let Some(listeners) = state.subscribers.get_mut(&workspace_id) {
                    listeners.retain(|listener| listener.id != subscriber_id);
                    if listeners.is_empty() {
                        state.subscribers.remove(&workspace_id);
                    }
                }
                return Err(error);
            }
        };
        Ok(MembershipSubscription {
            state: crate::subscription::SubscriptionState::new(durable, receiver, after_epoch),
            runtime_state: Arc::downgrade(&self.state),
            workspace_id,
            subscriber_id,
        })
    }
}

/// What a raw package-synchronization probe observed.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProbeOutcome {
    Refused(crate::synchronization::Refusal),
    Welcomed { operations_received: usize },
    Failed(String),
}

/// Serves this Installation's real operations plus a fixed set of extras.
///
/// Only the sending side is altered: the peer applies everything through its
/// own unmodified gate and classification.
#[cfg(test)]
struct InjectingReceiver {
    inner: RuntimeSyncReceiver,
    extra: Vec<([u8; 32], p2panda_core::Operation<()>)>,
}

#[cfg(test)]
impl crate::synchronization::SyncReceiver for InjectingReceiver {
    fn authorize_session(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
    ) -> Result<crate::synchronization::SessionGrant, crate::synchronization::Refusal> {
        self.inner.authorize_session(peer, workspace, package)
    }

    fn protection_bundle_for(
        &self,
        workspace: WorkspaceId,
        package: &str,
        recipient: PublicIdentity,
    ) -> Result<Option<crate::synchronization::ProtectionBundle>, ()> {
        self.inner
            .protection_bundle_for(workspace, package, recipient)
    }

    fn import_protection_bundle(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
        bundle: crate::synchronization::ProtectionBundle,
    ) -> Result<usize, ()> {
        self.inner
            .import_protection_bundle(peer, workspace, package, bundle)
    }

    fn authors(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
    ) -> Result<Vec<PublicIdentity>, ()> {
        self.inner.authors(peer, workspace)
    }

    fn operations(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
    ) -> Result<Vec<([u8; 32], p2panda_core::Operation<()>)>, ()> {
        let mut operations = self.inner.operations(peer, workspace, package)?;
        operations.extend(self.extra.iter().cloned());
        Ok(operations)
    }

    fn receive_operation(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
        operation: p2panda_core::Operation<()>,
    ) -> Result<crate::synchronization::OperationOutcome, crate::synchronization::FatalSyncError>
    {
        self.inner
            .receive_operation(peer, workspace, package, operation)
    }
}

/// A probe's Core seam: it authorizes its own dial, offers nothing, and counts
/// what the peer sent. It is a fake only for the probe's own side.
#[cfg(test)]
struct ProbeReceiver;

#[cfg(test)]
impl crate::synchronization::SyncReceiver for ProbeReceiver {
    fn authorize_session(
        &self,
        _peer: PublicIdentity,
        _workspace: WorkspaceId,
        _package: &str,
    ) -> Result<crate::synchronization::SessionGrant, crate::synchronization::Refusal> {
        Ok(crate::synchronization::SessionGrant {
            membership_version: [0; 32],
            content_key_id: None,
        })
    }

    fn protection_bundle_for(
        &self,
        _workspace: WorkspaceId,
        _package: &str,
        _recipient: PublicIdentity,
    ) -> Result<Option<crate::synchronization::ProtectionBundle>, ()> {
        Ok(None)
    }

    fn import_protection_bundle(
        &self,
        _peer: PublicIdentity,
        _workspace: WorkspaceId,
        _package: &str,
        _bundle: crate::synchronization::ProtectionBundle,
    ) -> Result<usize, ()> {
        Ok(0)
    }

    fn authors(
        &self,
        _peer: PublicIdentity,
        _workspace: WorkspaceId,
    ) -> Result<Vec<PublicIdentity>, ()> {
        Ok(Vec::new())
    }

    fn operations(
        &self,
        _peer: PublicIdentity,
        _workspace: WorkspaceId,
        _package: &str,
    ) -> Result<Vec<([u8; 32], p2panda_core::Operation<()>)>, ()> {
        Ok(Vec::new())
    }

    fn receive_operation(
        &self,
        _peer: PublicIdentity,
        _workspace: WorkspaceId,
        _package: &str,
        _operation: p2panda_core::Operation<()>,
    ) -> Result<crate::synchronization::OperationOutcome, crate::synchronization::FatalSyncError>
    {
        Ok(crate::synchronization::OperationOutcome::Applied)
    }
}

/// Holds the runtime state lock on a separate thread until released.
///
/// A test uses this to make "Core is busy" observable; production never
/// constructs one.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub struct RuntimeStateHold {
    release: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(any(test, feature = "test-hooks"))]
impl RuntimeStateHold {
    pub fn release(mut self) {
        self.release.take().unwrap().send(()).unwrap();
        self.thread.take().unwrap().join().unwrap();
    }
}

#[cfg(any(test, feature = "test-hooks"))]
impl Drop for RuntimeStateHold {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Imports a validated bootstrap with the state lock already held.
///
/// A free function so the whole import can run inside one blocking closure,
/// without a `&CoreRuntime` reference crossing the thread boundary.
fn import_validated_bootstrap_locked(
    mut state: std::sync::MutexGuard<'_, RuntimeState>,
    bootstrap: &crate::membership::projection::ValidatedBootstrap,
    deadline: Option<std::time::Instant>,
    fault: impl FnMut(crate::membership::projection::ImportBoundary) -> Result<(), StorageError>,
) -> Result<(WorkspaceSummary, MembershipSnapshot), crate::OnboardingError> {
    if state.poisoned {
        return Err(crate::OnboardingError::UncertainStorageOutcome);
    }
    if let Some(deadline) = deadline {
        CoreRuntime::ensure_join_deadline(deadline)?;
    }
    let prior_busy_timeout_ms = state
        .connection
        .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
        .map_err(|_| crate::OnboardingError::LocalConflict)?;
    let prior_busy_timeout_ms =
        u64::try_from(prior_busy_timeout_ms).map_err(|_| crate::OnboardingError::LocalConflict)?;
    if let Some(deadline) = deadline {
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .ok_or(crate::OnboardingError::Timeout)?;
        state
            .connection
            .busy_timeout(remaining)
            .map_err(|_| crate::OnboardingError::LocalConflict)?;
    }
    let result = crate::membership::projection::import_bootstrap_with_fault(
        &mut state.connection,
        bootstrap,
        deadline,
        fault,
    );
    if state
        .connection
        .busy_timeout(std::time::Duration::from_millis(prior_busy_timeout_ms))
        .is_err()
    {
        state.poisoned = true;
        return Err(crate::OnboardingError::UncertainStorageOutcome);
    }
    if !matches!(
        &result,
        Err(crate::OnboardingError::UncertainStorageOutcome)
    ) && result.is_err()
        && deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline)
    {
        return Err(crate::OnboardingError::Timeout);
    }
    match result {
        Ok(imported) => {
            let snapshot = imported.projection.snapshot();
            state
                .memberships
                .insert(snapshot.workspace_id(), imported.projection);
            if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
                state.poisoned = true;
                return Err(crate::OnboardingError::UncertainStorageOutcome);
            }
            Ok((imported.summary, snapshot))
        }
        Err(error) => {
            if error == crate::OnboardingError::UncertainStorageOutcome {
                state.poisoned = true;
            }
            Err(error)
        }
    }
}

pub(crate) fn respond_to_join(
    state: &Arc<Mutex<RuntimeState>>,
    clock: &Arc<dyn Clock>,
    authenticated_peer: PublicIdentity,
    claim: crate::invitation::VerifiedInvitationClaim,
) -> Result<Vec<u8>, crate::OnboardingError> {
    if claim.invitee != authenticated_peer {
        return Err(crate::OnboardingError::Authentication);
    }
    let mut state = state
        .lock()
        .map_err(|_| crate::OnboardingError::UncertainStorageOutcome)?;
    if state.poisoned {
        return Err(crate::OnboardingError::UncertainStorageOutcome);
    }
    let now_unix_ms = clock.now_unix_ms();
    let applied = {
        let RuntimeState {
            connection,
            memberships,
            ..
        } = &mut *state;
        crate::membership::projection::maintain_quarantine(connection, memberships, now_unix_ms)
    }
    .map_err(|error| match error {
        MembershipError::Storage(StorageError::UncertainOutcome) => {
            state.poisoned = true;
            crate::OnboardingError::UncertainStorageOutcome
        }
        _ => crate::OnboardingError::InvalidInput,
    })?;
    for applied in applied {
        if let Some(transition) = CoreRuntime::transition_from_applied(applied) {
            replication::after_membership_accepted(&mut state, &transition);
        }
    }
    let RuntimeState {
        connection,
        secret_key,
        memberships,
        poisoned,
        ..
    } = &mut *state;
    match crate::membership::projection::redeem_invitation(
        connection,
        memberships,
        secret_key,
        claim,
        authenticated_peer,
        now_unix_ms,
    ) {
        Ok(export) => {
            if export.applied.accepted
                && let Some(transition) = CoreRuntime::transition_from_applied(export.applied)
            {
                replication::after_membership_accepted(&mut state, &transition);
            }
            Ok(export.response)
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
            Err(map_invitation_onboarding_error(error))
        }
    }
}

/// Reports a refused package synchronization session as the capability error
/// the caller can act on, instead of as a generic unavailability.
fn map_log_session_error(
    error: crate::synchronization::LogSessionError,
) -> crate::SynchronizationError {
    match error {
        crate::synchronization::LogSessionError::Refused(
            crate::synchronization::Refusal::NotAuthorized,
        ) => crate::SynchronizationError::NotCurrentMember,
        crate::synchronization::LogSessionError::Refused(
            crate::synchronization::Refusal::KeyMismatch,
        ) => crate::SynchronizationError::WrongScope,
        _ => crate::SynchronizationError::RuntimeUnavailable,
    }
}

pub(crate) fn map_invitation_onboarding_error(error: InvitationError) -> crate::OnboardingError {
    match error {
        InvitationError::Expired => crate::OnboardingError::Expired,
        InvitationError::AlreadyUsed => crate::OnboardingError::AlreadyUsed,
        InvitationError::Storage(StorageError::UncertainOutcome)
        | InvitationError::Membership(MembershipError::Storage(StorageError::UncertainOutcome)) => {
            crate::OnboardingError::UncertainStorageOutcome
        }
        InvitationError::WorkspaceNotFound => crate::OnboardingError::LocalConflict,
        InvitationError::Membership(MembershipError::MemberNotFound) => {
            crate::OnboardingError::AdmissionNoLongerCurrent
        }
        InvitationError::Membership(MembershipError::DecodeLimit) => {
            crate::OnboardingError::Overload
        }
        InvitationError::NotManager
        | InvitationError::WrongIssuer
        | InvitationError::InvalidTokenSignature
        | InvitationError::InvalidClaimSignature
        | InvitationError::ClaimTargetMismatch => crate::OnboardingError::Authentication,
        _ => crate::OnboardingError::InvalidInput,
    }
}

impl Drop for CoreRuntime {
    fn drop(&mut self) {
        self.transport.start_abnormal_shutdown();
    }
}

#[path = "runtime_replication.rs"]
mod replication;

#[cfg(test)]
#[path = "runtime_tests/mod.rs"]
mod runtime_tests;

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

    use iroh_base::SecretKey;

    use super::*;
    use crate::invitation::{sign_claim, sign_token};
    use crate::membership::{AdmissionProof, MembershipAction, MembershipEnvelope};
    use crate::{RecordEvent, RecordId};

    struct RecordingClock {
        calls: Arc<AtomicU64>,
    }

    impl Clock for RecordingClock {
        fn now_unix_ms(&self) -> i64 {
            self.calls.fetch_add(1, Ordering::Relaxed);
            1_234
        }
    }

    async fn wait_for_record(
        runtime: &CoreRuntime,
        workspace: WorkspaceId,
        package: &str,
        record_id: RecordId,
    ) -> RecordEvent {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Ok(mut subscription) = runtime.subscribe_records(workspace, package, None) {
                while let Some(event) = subscription
                    .next_event_timeout(std::time::Duration::ZERO)
                    .unwrap()
                {
                    if event.record_id() == record_id {
                        return event;
                    }
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "record did not arrive before the test deadline"
            );
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    #[tokio::test]
    async fn production_log_sync_grants_scope_converges_and_survives_restart() {
        let root = std::env::temp_dir().join(format!(
            "teamarness-production-log-sync-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let a_dir = root.join("a");
        let b_dir = root.join("b");
        let clock = || {
            Box::new(RecordingClock {
                calls: Arc::new(AtomicU64::new(0)),
            }) as Box<dyn Clock>
        };
        let a_config = CoreRuntimeConfig::new(&a_dir);
        let b_config = CoreRuntimeConfig::new(&b_dir);
        let a = CoreRuntime::open_with_clock_and_readiness(a_config.clone(), clock(), true)
            .await
            .unwrap();
        let workspace = a.create_workspace("Production sync").unwrap();
        let package = "teamarness.test";
        let invitation = a
            .create_workspace_invitation_for_test(workspace.id(), 1, 60_000)
            .unwrap();
        let b = CoreRuntime::open_with_clock_and_readiness(b_config.clone(), clock(), true)
            .await
            .unwrap();
        b.join_workspace(&invitation).await.unwrap();

        let first = a
            .append_record(workspace.id(), package, "message.post", b"first")
            .unwrap();
        let received = wait_for_record(&b, workspace.id(), package, first.record_id()).await;
        assert_eq!(received.record_id(), first.record_id());

        let a_local = a
            .append_record(workspace.id(), package, "message.post", b"a-local")
            .unwrap();
        let received_a_local =
            wait_for_record(&b, workspace.id(), package, a_local.record_id()).await;
        assert_eq!(received_a_local.record_id(), a_local.record_id());

        let b_local = b
            .append_record(workspace.id(), package, "message.post", b"b-local")
            .unwrap();
        let received_b_local =
            wait_for_record(&a, workspace.id(), package, b_local.record_id()).await;
        let mut a_records = a.subscribe_records(workspace.id(), package, None).unwrap();
        let mut a_seen = std::collections::BTreeSet::new();
        while let Some(event) = a_records
            .next_event_timeout(std::time::Duration::ZERO)
            .unwrap()
        {
            a_seen.insert((event.record_id(), event.order_key()));
        }
        assert_eq!(received_b_local.record_id(), b_local.record_id());
        let mut b_records = b.subscribe_records(workspace.id(), package, None).unwrap();
        let mut b_seen = std::collections::BTreeSet::new();
        while let Some(event) = b_records
            .next_event_timeout(std::time::Duration::ZERO)
            .unwrap()
        {
            b_seen.insert((event.record_id(), event.order_key()));
        }
        for expected in [
            (first.record_id(), first.order_key()),
            (a_local.record_id(), a_local.order_key()),
            (b_local.record_id(), b_local.order_key()),
        ] {
            assert!(a_seen.contains(&expected));
            assert!(b_seen.contains(&expected));
        }

        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
        drop(a);
        drop(b);

        let a = CoreRuntime::open_with_clock_and_readiness(a_config, clock(), true)
            .await
            .unwrap();
        let b = CoreRuntime::open_with_clock_and_readiness(b_config, clock(), true)
            .await
            .unwrap();
        let after_restart = a
            .append_record(workspace.id(), package, "message.post", b"after-restart")
            .unwrap();
        a.replicate_records_to_address_for_test(
            workspace.id(),
            package,
            b.transport.endpoint_address(),
            b.public_identity(),
        )
        .await
        .unwrap();
        let received_after_restart =
            wait_for_record(&b, workspace.id(), package, after_restart.record_id()).await;
        assert_eq!(
            received_after_restart.record_id(),
            after_restart.record_id()
        );

        let mut b_records = b.subscribe_records(workspace.id(), package, None).unwrap();
        let mut b_seen = std::collections::BTreeSet::new();
        while let Some(event) = b_records
            .next_event_timeout(std::time::Duration::ZERO)
            .unwrap()
        {
            assert!(b_seen.insert((event.record_id(), event.order_key())));
        }
        assert_eq!(b_seen.len(), 4);
        assert!(b_seen.contains(&(after_restart.record_id(), after_restart.order_key())));

        b.state
            .lock()
            .unwrap()
            .connection
            .execute_batch(
                "CREATE TEMP TRIGGER fail_remote_record_insert
                 BEFORE INSERT ON immutable_records
                 BEGIN SELECT RAISE(ABORT, 'injected remote apply failure'); END;",
            )
            .unwrap();
        let after_failure = a
            .append_record(workspace.id(), package, "message.post", b"after-failure")
            .unwrap();
        // The injected row failure is one peer's conflict, not a local fault:
        // the session finishes, the record is simply not applied yet.
        a.replicate_records_to_address_for_test(
            workspace.id(),
            package,
            b.transport.endpoint_address(),
            b.public_identity(),
        )
        .await
        .unwrap();
        let mut b_records = b.subscribe_records(workspace.id(), package, None).unwrap();
        let mut b_seen = std::collections::BTreeSet::new();
        while let Some(event) = b_records
            .next_event_timeout(std::time::Duration::ZERO)
            .unwrap()
        {
            b_seen.insert(event.record_id());
        }
        assert!(!b_seen.contains(&after_failure.record_id()));
        b.state
            .lock()
            .unwrap()
            .connection
            .execute_batch("DROP TRIGGER fail_remote_record_insert;")
            .unwrap();
        a.replicate_records_to_address_for_test(
            workspace.id(),
            package,
            b.transport.endpoint_address(),
            b.public_identity(),
        )
        .await
        .unwrap();
        let received_after_failure =
            wait_for_record(&b, workspace.id(), package, after_failure.record_id()).await;
        assert_eq!(
            received_after_failure.record_id(),
            after_failure.record_id()
        );

        let mut b_records = b.subscribe_records(workspace.id(), package, None).unwrap();
        let mut b_seen = std::collections::BTreeSet::new();
        while let Some(event) = b_records
            .next_event_timeout(std::time::Duration::ZERO)
            .unwrap()
        {
            assert!(b_seen.insert((event.record_id(), event.order_key())));
        }
        assert_eq!(b_seen.len(), 5);

        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
        drop(a);
        drop(b);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn current_peer_forwards_record_from_historical_member() {
        let root = std::env::temp_dir().join(format!(
            "teamarness-historical-record-forwarding-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let alice = CoreRuntime::open_with_clock_and_readiness(
            CoreRuntimeConfig::new(root.join("alice")),
            Box::new(SystemClock),
            true,
        )
        .await
        .unwrap();
        let workspace = alice.create_workspace("Historical forwarding").unwrap();
        let bob_invitation = alice
            .create_workspace_invitation(workspace.id())
            .await
            .unwrap();
        let bob = CoreRuntime::open_with_clock_and_readiness(
            CoreRuntimeConfig::new(root.join("bob")),
            Box::new(SystemClock),
            true,
        )
        .await
        .unwrap();
        bob.join_workspace(&bob_invitation).await.unwrap();

        let package = "teamarness.test";
        alice
            .append_record(workspace.id(), package, "message.post", b"seed")
            .unwrap();
        let (bob_origin, bob_grant) = alice
            .export_protection_bundle_for_test(workspace.id(), package, bob.public_identity())
            .unwrap();
        bob.import_protection_grant(workspace.id(), package, &bob_origin, &bob_grant)
            .unwrap();
        let historical = bob
            .append_record(workspace.id(), package, "message.post", b"historical")
            .unwrap();
        // Bob's own operation is read while Bob is still a current Member: the
        // gate now refuses a revoked Installation on its own sessions too.
        let bob_receiver = RuntimeSyncReceiver {
            state: Arc::downgrade(&bob.state),
            clock: Arc::new(SystemClock),
            demand: Arc::clone(&bob.membership_demand),
        };
        let operation = crate::synchronization::SyncReceiver::operations(
            &bob_receiver,
            alice.public_identity(),
            workspace.id(),
            package,
        )
        .unwrap()
        .into_iter()
        .map(|(_, operation)| operation)
        .find(|operation| {
            operation.header.verifying_key.as_bytes() == bob.public_identity().as_bytes()
        })
        .unwrap();
        alice
            .revoke_member(workspace.id(), bob.public_identity())
            .unwrap();

        let carol_invitation = alice
            .create_workspace_invitation(workspace.id())
            .await
            .unwrap();
        let carol = CoreRuntime::open_with_clock_and_readiness(
            CoreRuntimeConfig::new(root.join("carol")),
            Box::new(SystemClock),
            true,
        )
        .await
        .unwrap();
        carol.join_workspace(&carol_invitation).await.unwrap();
        let (carol_origin, carol_grant) = alice
            .export_protection_bundle_for_test(workspace.id(), package, carol.public_identity())
            .unwrap();
        carol
            .import_protection_grant(workspace.id(), package, &carol_origin, &carol_grant)
            .unwrap();

        let carol_receiver = RuntimeSyncReceiver {
            state: Arc::downgrade(&carol.state),
            clock: Arc::new(SystemClock),
            demand: Arc::clone(&carol.membership_demand),
        };
        crate::synchronization::SyncReceiver::receive_operation(
            &carol_receiver,
            alice.public_identity(),
            workspace.id(),
            package,
            operation,
        )
        .expect("a current peer may forward a historically eligible author's record");

        let mut subscription = carol
            .subscribe_records(workspace.id(), package, None)
            .unwrap();
        let mut seen = std::collections::BTreeSet::new();
        while let Some(event) = subscription
            .next_event_timeout(std::time::Duration::ZERO)
            .unwrap()
        {
            seen.insert(event.record_id());
        }
        assert!(seen.contains(&historical.record_id()));

        alice.shutdown().await.unwrap();
        bob.shutdown().await.unwrap();
        carol.shutdown().await.unwrap();
        drop(alice);
        drop(bob);
        drop(carol);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn invalid_remote_sequence_isolated_before_journal_commit() {
        let root = std::env::temp_dir().join(format!(
            "teamarness-invalid-remote-operation-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let a = CoreRuntime::open_with_clock_and_readiness(
            CoreRuntimeConfig::new(root.join("a")),
            Box::new(SystemClock),
            true,
        )
        .await
        .unwrap();
        let workspace = a.create_workspace("Invalid remote").unwrap();
        let invitation = a
            .create_workspace_invitation_for_test(
                workspace.id(),
                1,
                SystemClock.now_unix_ms().saturating_add(60_000),
            )
            .unwrap();
        let b = CoreRuntime::open_with_clock_and_readiness(
            CoreRuntimeConfig::new(root.join("b")),
            Box::new(SystemClock),
            true,
        )
        .await
        .unwrap();
        b.join_workspace(&invitation).await.unwrap();
        let package = "teamarness.test";
        a.append_record(workspace.id(), package, "message.post", b"seed")
            .unwrap();
        let (origin, grant) = a
            .export_protection_bundle_for_test(workspace.id(), package, b.public_identity())
            .unwrap();
        b.import_protection_grant(workspace.id(), package, &origin, &grant)
            .unwrap();
        let record = b
            .append_record(workspace.id(), package, "message.post", b"remote")
            .unwrap();
        let secret = b.state.lock().unwrap().secret_key.to_bytes();
        let key = p2panda_core::SigningKey::from_bytes(&secret);
        let operation = p2panda_core::Operation::from_parts(
            p2panda_core::Header::<()>::builder()
                .body(record.sealed_record())
                .seq_num(1)
                .backlink(Some(p2panda_core::Hash::from_bytes([7; 32])))
                .build(&key, ()),
            Some(p2panda_core::Body::from_bytes(record.sealed_record())),
        );
        let before = {
            let state = a.state.lock().unwrap();
            (
                state
                    .connection
                    .query_row::<i64, _, _>(
                        "SELECT COUNT(*) FROM immutable_operations",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap(),
                state
                    .connection
                    .query_row::<i64, _, _>("SELECT COUNT(*) FROM p2panda_operations", [], |row| {
                        row.get(0)
                    })
                    .unwrap(),
            )
        };
        let receiver = RuntimeSyncReceiver {
            state: Arc::downgrade(&a.state),
            clock: Arc::new(SystemClock),
            demand: Arc::clone(&a.membership_demand),
        };
        // One unacceptable operation is classified, not fatal: the session
        // continues and the author is skipped for the rest of it.
        assert_eq!(
            crate::synchronization::SyncReceiver::receive_operation(
                &receiver,
                b.public_identity(),
                workspace.id(),
                package,
                operation,
            ),
            Ok(crate::synchronization::OperationOutcome::Rejected(
                crate::synchronization::RejectReason::Conflict
            ))
        );
        let after = {
            let state = a.state.lock().unwrap();
            (
                state
                    .connection
                    .query_row::<i64, _, _>(
                        "SELECT COUNT(*) FROM immutable_operations",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap(),
                state
                    .connection
                    .query_row::<i64, _, _>("SELECT COUNT(*) FROM p2panda_operations", [], |row| {
                        row.get(0)
                    })
                    .unwrap(),
            )
        };
        assert_eq!(before, after);
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
        drop(a);
        drop(b);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn test_clock_is_injected_without_entering_the_public_configuration() {
        let calls = Arc::new(AtomicU64::new(0));
        let directory =
            std::env::temp_dir().join(format!("teamarness-runtime-clock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);

        let runtime = CoreRuntime::open_with_clock(
            CoreRuntimeConfig::new(&directory),
            Box::new(RecordingClock {
                calls: Arc::clone(&calls),
            }),
        )
        .await
        .unwrap();

        assert_eq!(calls.load(Ordering::Relaxed), 1);
        runtime.shutdown().await.unwrap();
        drop(runtime);
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn dropping_a_membership_subscription_deregisters_its_listener() {
        let directory = std::env::temp_dir().join(format!(
            "teamarness-runtime-subscription-drop-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        let runtime = CoreRuntime::open_with_clock(
            CoreRuntimeConfig::new(&directory),
            Box::new(RecordingClock {
                calls: Arc::new(AtomicU64::new(0)),
            }),
        )
        .await
        .unwrap();
        let workspace = runtime.create_workspace("Subscription drop").unwrap();

        let subscription = runtime
            .subscribe_membership_bounded(workspace.id(), MembershipEpoch::new(0), 1)
            .unwrap();
        assert_eq!(
            runtime
                .state
                .lock()
                .unwrap()
                .subscribers
                .get(&workspace.id())
                .map_or(0, Vec::len),
            1
        );
        drop(subscription);
        assert_eq!(
            runtime
                .state
                .lock()
                .unwrap()
                .subscribers
                .get(&workspace.id())
                .map_or(0, Vec::len),
            0
        );

        runtime.shutdown().await.unwrap();
        drop(runtime);
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn endpoint_identity_persists_and_invitation_lifetime_starts_after_readiness() {
        let directory = std::env::temp_dir().join(format!(
            "teamarness-runtime-transport-identity-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        let now = Arc::new(AtomicI64::new(42_000));
        let config = CoreRuntimeConfig::new(&directory);
        let runtime = CoreRuntime::open_with_clock(
            config.clone(),
            Box::new(AdjustableClock(Arc::clone(&now))),
        )
        .await
        .unwrap();
        assert_eq!(
            runtime.transport.endpoint_id(),
            runtime.public_identity.as_public_key()
        );
        let workspace = runtime.create_workspace("Transport identity").unwrap();
        let invitation = runtime
            .create_workspace_invitation(workspace.id())
            .await
            .unwrap();
        let verified = invitation.verified().unwrap();
        assert_eq!(verified.token.issued_at_unix_ms, 42_000);
        assert_eq!(verified.token.expires_at_unix_ms, 42_000 + 15 * 60 * 1_000);
        runtime.shutdown().await.unwrap();
        drop(runtime);

        let reopened =
            CoreRuntime::open_with_clock(config, Box::new(AdjustableClock(Arc::clone(&now))))
                .await
                .unwrap();
        assert_eq!(
            reopened.transport.endpoint_id(),
            reopened.public_identity.as_public_key()
        );
        reopened.shutdown().await.unwrap();
        drop(reopened);
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn shutdown_waits_for_invitation_creation_after_readiness() {
        let directory = std::env::temp_dir().join(format!(
            "teamarness-runtime-invitation-shutdown-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        let runtime = Arc::new(
            CoreRuntime::open_with_clock(
                CoreRuntimeConfig::new(&directory),
                Box::new(AdjustableClock(Arc::new(AtomicI64::new(42_000)))),
            )
            .await
            .unwrap(),
        );
        let workspace = runtime.create_workspace("Invitation shutdown").unwrap();

        runtime.transport.hold_next_invitation_after_readiness();
        let inviting_runtime = Arc::clone(&runtime);
        let invitation = tokio::spawn(async move {
            inviting_runtime
                .create_workspace_invitation(workspace.id())
                .await
        });
        runtime.transport.wait_for_invitation_readiness_hold().await;
        assert_eq!(runtime.transport.active_operations(), 1);

        let closing_runtime = Arc::clone(&runtime);
        let mut shutdown = tokio::spawn(async move { closing_runtime.shutdown().await });
        tokio::select! {
            result = &mut shutdown => {
                panic!("shutdown completed before invitation creation: {result:?}");
            }
            () = runtime.transport.wait_for_idle_waiter() => {}
        }

        runtime.transport.release_invitation_readiness_hold();
        invitation.await.unwrap().unwrap();
        shutdown.await.unwrap().unwrap();
        drop(runtime);
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn readiness_failure_does_not_sign_or_start_an_invitation_lifetime() {
        let directory = std::env::temp_dir().join(format!(
            "teamarness-runtime-readiness-clock-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        let calls = Arc::new(AtomicU64::new(0));
        let runtime = CoreRuntime::open_with_clock_and_readiness(
            CoreRuntimeConfig::new(&directory),
            Box::new(RecordingClock {
                calls: Arc::clone(&calls),
            }),
            false,
        )
        .await
        .unwrap();
        let workspace = runtime.create_workspace("No readiness").unwrap();
        let before = calls.load(Ordering::Relaxed);
        assert_eq!(
            runtime
                .create_workspace_invitation(workspace.id())
                .await
                .unwrap_err(),
            crate::OnboardingError::Readiness
        );
        assert_eq!(calls.load(Ordering::Relaxed), before);
        runtime.shutdown().await.unwrap();
        drop(runtime);
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn stopping_transport_rejects_onboarding_but_preserves_local_commands() {
        let directory = std::env::temp_dir().join(format!(
            "teamarness-runtime-local-during-shutdown-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        let runtime = Arc::new(
            CoreRuntime::open_with_clock(
                CoreRuntimeConfig::new(&directory),
                Box::new(AdjustableClock(Arc::new(AtomicI64::new(7_000)))),
            )
            .await
            .unwrap(),
        );
        let workspace = runtime.create_workspace("Local during shutdown").unwrap();
        let active = runtime.transport.begin_outbound().await.unwrap();
        let closing = Arc::clone(&runtime);
        let shutdown = tokio::spawn(async move { closing.shutdown().await });
        while runtime.transport.lifecycle_state() == 0 {
            tokio::task::yield_now().await;
        }

        assert_eq!(runtime.display_name(), "This device");
        assert_eq!(runtime.list_workspaces().unwrap(), vec![workspace.clone()]);
        assert_eq!(
            runtime
                .membership_snapshot(workspace.id())
                .unwrap()
                .members()
                .len(),
            1
        );
        assert_eq!(
            runtime
                .create_workspace_invitation(workspace.id())
                .await
                .unwrap_err(),
            crate::OnboardingError::Shutdown
        );
        assert!(matches!(
            runtime.transport.begin_outbound().await,
            Err(crate::OnboardingError::Shutdown)
        ));
        drop(active);
        shutdown.await.unwrap().unwrap();
        runtime.shutdown().await.unwrap();
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

    #[tokio::test]
    async fn uncertain_invitation_commit_poisons_runtime_until_authenticated_reopen() {
        let issuer_directory = std::env::temp_dir().join(format!(
            "teamarness-runtime-invitation-uncertain-{}",
            std::process::id()
        ));
        let invitee_directory = std::env::temp_dir().join(format!(
            "teamarness-runtime-invitation-claimant-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&issuer_directory);
        let _ = fs::remove_dir_all(&invitee_directory);
        let config = CoreRuntimeConfig::new(&issuer_directory);
        let issuer = CoreRuntime::open(config.clone()).await.unwrap();
        let invitee = CoreRuntime::open(CoreRuntimeConfig::new(&invitee_directory))
            .await
            .unwrap();
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
        issuer.shutdown().await.unwrap();
        drop(issuer);

        let reopened = CoreRuntime::open(config).await.unwrap();
        let snapshot = reopened.membership_snapshot(workspace.id()).unwrap();
        assert_eq!(snapshot.epoch(), MembershipEpoch::new(1));
        assert_eq!(snapshot.members().len(), 2);
        let recovered = reopened.redeem_invitation(&claim).unwrap();
        assert_eq!(recovered.epoch(), MembershipEpoch::new(1));

        reopened.shutdown().await.unwrap();
        drop(reopened);
        invitee.shutdown().await.unwrap();
        drop(invitee);
        fs::remove_dir_all(issuer_directory).unwrap();
        fs::remove_dir_all(invitee_directory).unwrap();
    }

    #[tokio::test]
    async fn canonical_path_admits_revokes_and_rejects_invalid_authority_without_advancing_state() {
        let directory = std::env::temp_dir().join(format!(
            "teamarness-runtime-membership-validation-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        let runtime = CoreRuntime::open(CoreRuntimeConfig::new(&directory))
            .await
            .unwrap();
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

        runtime.shutdown().await.unwrap();
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

    #[tokio::test]
    async fn every_reachable_topological_order_converges_for_single_manager_branches() {
        let baseline = std::env::temp_dir().join(format!(
            "teamarness-runtime-convergence-baseline-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&baseline);
        let runtime = CoreRuntime::open(CoreRuntimeConfig::new(&baseline))
            .await
            .unwrap();
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
        runtime.shutdown().await.unwrap();
        drop(runtime);

        let orders = topological_orders(&[vec![], vec![], vec![], vec![0], vec![2]]);
        assert!(orders.len() > 1);
        let mut expected = None;
        for (index, order) in orders.into_iter().enumerate() {
            let directory = std::env::temp_dir().join(format!(
                "teamarness-runtime-convergence-{}-{index}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&directory);
            fs::create_dir_all(&directory).unwrap();
            fs::copy(
                baseline.join(crate::storage::DATABASE_FILE_NAME),
                directory.join(crate::storage::DATABASE_FILE_NAME),
            )
            .unwrap();
            let branch = CoreRuntime::open(CoreRuntimeConfig::new(&directory))
                .await
                .unwrap();
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
            branch.shutdown().await.unwrap();
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

    #[tokio::test]
    async fn quarantine_retries_dependency_first_and_never_advances_twice() {
        let directory = std::env::temp_dir().join(format!(
            "teamarness-runtime-quarantine-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        let now = Arc::new(AtomicI64::new(1_000));
        let config = CoreRuntimeConfig::new(&directory);
        let runtime = CoreRuntime::open_with_clock(
            config.clone(),
            Box::new(AdjustableClock(Arc::clone(&now))),
        )
        .await
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

        runtime.shutdown().await.unwrap();
        drop(runtime);
        now.store(1_002, Ordering::Relaxed);
        let recovered = CoreRuntime::open_with_clock(
            config.clone(),
            Box::new(AdjustableClock(Arc::clone(&now))),
        )
        .await
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
        recovered.shutdown().await.unwrap();
        drop(recovered);

        let reopened =
            CoreRuntime::open_with_clock(config, Box::new(AdjustableClock(Arc::clone(&now))))
                .await
                .unwrap();
        assert_eq!(
            reopened.membership_snapshot(workspace.id()).unwrap(),
            eligible
        );

        reopened.shutdown().await.unwrap();
        drop(reopened);
        fs::remove_dir_all(directory).unwrap();
    }
}
