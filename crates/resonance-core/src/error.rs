use std::error::Error;
use std::fmt;

/// An error returned while parsing a public domain value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueParseError {
    value_name: &'static str,
}

impl ValueParseError {
    pub(crate) const fn new(value_name: &'static str) -> Self {
        Self { value_name }
    }
}

impl fmt::Display for ValueParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid {}", self.value_name)
    }
}

impl Error for ValueParseError {}

macro_rules! error_family {
    ($name:ident, $summary:literal) => {
        #[doc = $summary]
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[non_exhaustive]
        pub enum $name {
            /// The operation is part of the public contract but is not available in this scaffold.
            RuntimeUnavailable,
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("the core runtime is not available in the repository scaffold")
            }
        }

        impl Error for $name {}
    };
}

error_family!(ConfigurationError, "A runtime configuration error.");

/// A durable core storage or recovery error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StorageError {
    /// The private data directory is not a directory.
    InvalidDataDirectory,
    /// A filesystem operation failed.
    Io,
    /// Restrictive filesystem permissions could not be applied.
    Permission,
    /// SQLite could not open or operate on the core database.
    Database,
    /// An ordered migration could not be applied.
    Migration,
    /// The database schema is newer than this runtime supports.
    UnsupportedSchema,
    /// Durable installation identity records failed authentication.
    CorruptIdentity,
    /// Durable workspace trust anchors or canonical genesis records disagree.
    CorruptWorkspace,
    /// SQLite may have committed, so the active runtime cannot safely continue.
    UncertainOutcome,
    /// The active runtime is poisoned and must be reopened.
    RuntimePoisoned,
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidDataDirectory => "the private data path is not a directory",
            Self::Io => "private storage filesystem operation failed",
            Self::Permission => "private storage permissions could not be restricted",
            Self::Database => "core database operation failed",
            Self::Migration => "core database migration failed",
            Self::UnsupportedSchema => "core database schema is newer than this runtime",
            Self::CorruptIdentity => "durable installation identity is corrupt",
            Self::CorruptWorkspace => "durable workspace trust anchor is corrupt",
            Self::UncertainOutcome => "database commit outcome is uncertain",
            Self::RuntimePoisoned => "the runtime must be reopened before another command",
        };
        formatter.write_str(message)
    }
}

impl Error for StorageError {}

/// An installation identity or signing error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdentityError {
    /// A display name violated the documented length or character rules.
    InvalidDisplayName,
    /// A signing context violated the documented domain-tag rules.
    InvalidSigningContext,
    /// Signature verification failed.
    InvalidSignature,
    /// A payload cannot be represented by the versioned signing envelope.
    PayloadTooLarge,
    /// Durable storage failed while changing identity metadata.
    Storage(StorageError),
}

impl fmt::Display for IdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDisplayName => formatter.write_str("invalid installation display name"),
            Self::InvalidSigningContext => formatter.write_str("invalid signing context"),
            Self::InvalidSignature => formatter.write_str("invalid signature"),
            Self::PayloadTooLarge => formatter.write_str("signing payload is too large"),
            Self::Storage(error) => error.fmt(formatter),
        }
    }
}

impl Error for IdentityError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            _ => None,
        }
    }
}

/// A workspace command or query error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WorkspaceError {
    /// A workspace name violated the documented length or character rules.
    InvalidName,
    /// A workspace identity or genesis conflicts with durable state.
    Conflict,
    /// A ready workspace trust anchor or pending request is corrupt.
    CorruptTrustAnchor,
    /// Durable storage failed while processing the workspace.
    Storage(StorageError),
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName => formatter.write_str("invalid workspace name"),
            Self::Conflict => formatter.write_str("workspace identity or genesis conflicts"),
            Self::CorruptTrustAnchor => formatter.write_str("workspace trust anchor is corrupt"),
            Self::Storage(error) => error.fmt(formatter),
        }
    }
}

impl Error for WorkspaceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            _ => None,
        }
    }
}

/// A canonical membership authentication error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MembershipError {
    /// The command is reserved for a later implementation phase.
    RuntimeUnavailable,
    /// The canonical format version is unsupported.
    UnsupportedVersion,
    /// Bytes do not describe a well-formed Phase 1 action.
    MalformedAction,
    /// An installation or workspace identifier appeared in the wrong role.
    InvalidRole,
    /// Dependencies were not encoded in strictly increasing order.
    DependenciesNotCanonical,
    /// The encoded operation ID does not match the canonical unsigned bytes.
    ContentHashMismatch,
    /// The operation author's signature is invalid.
    InvalidSignature,
    /// A claim does not bind the member admitted by the action.
    AdmissionTargetMismatch,
    /// A canonical field or complete envelope exceeds its version-1 bound.
    DecodeLimit,
    /// The requested workspace does not exist.
    WorkspaceNotFound,
    /// The envelope names a different workspace than its trust anchor.
    WorkspaceMismatch,
    /// A dependency is unavailable.
    MissingDependency,
    /// A dependency belongs to another workspace.
    CrossWorkspaceDependency,
    /// A dependency does not descend from the workspace genesis.
    NonDescendantDependency,
    /// Authored time is earlier than a dependency's authored time.
    NonMonotonicTimestamp,
    /// Authored time exceeds the accepted five-minute future bound.
    FutureTimestamp,
    /// A create operation conflicts with the immutable genesis.
    ConflictingGenesis,
    /// The operation author is not the workspace's sole version-1 manager.
    NotManager,
    /// The admission token and claim do not authorize this operation.
    InvalidAdmissionProof,
    /// Version 1 forbids admission after a causal revocation of the same installation.
    CausalReadmission,
    /// The requested member is not currently active.
    MemberNotFound,
    /// Version 1 keeps the creator as the sole manager.
    CannotRevokeManager,
    /// p2panda rejected the authenticated operation.
    AuthorizationRejected,
    /// Durable storage failed while processing canonical membership.
    Storage(StorageError),
}

impl fmt::Display for MembershipError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::RuntimeUnavailable => "the membership runtime is not available",
            Self::UnsupportedVersion => "unsupported membership operation version",
            Self::MalformedAction => "malformed membership action",
            Self::InvalidRole => "membership identifier has the wrong role",
            Self::DependenciesNotCanonical => "membership dependencies are not canonical",
            Self::ContentHashMismatch => "membership operation content hash does not match",
            Self::InvalidSignature => "membership operation signature is invalid",
            Self::AdmissionTargetMismatch => "admission proof target does not match",
            Self::DecodeLimit => "membership operation exceeds a decode bound",
            Self::WorkspaceNotFound => "workspace was not found",
            Self::WorkspaceMismatch => "membership operation belongs to another workspace",
            Self::MissingDependency => "membership dependency is unavailable",
            Self::CrossWorkspaceDependency => "membership dependency belongs to another workspace",
            Self::NonDescendantDependency => "membership dependency does not descend from genesis",
            Self::NonMonotonicTimestamp => "membership timestamp is not causally monotonic",
            Self::FutureTimestamp => "membership operation timestamp is too far in the future",
            Self::ConflictingGenesis => "membership operation conflicts with workspace genesis",
            Self::NotManager => "membership operation author is not the workspace manager",
            Self::InvalidAdmissionProof => "admission proof does not authorize this operation",
            Self::CausalReadmission => "causal re-admission is not supported in version 1",
            Self::MemberNotFound => "workspace member is not currently active",
            Self::CannotRevokeManager => "the version-1 creator manager cannot be revoked",
            Self::AuthorizationRejected => "membership operation was rejected by authorization",
            Self::Storage(error) => return error.fmt(formatter),
        })
    }
}

impl Error for MembershipError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            _ => None,
        }
    }
}

/// An invitation token or claim authentication error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InvitationError {
    /// The command is reserved for a later implementation phase.
    RuntimeUnavailable,
    /// The canonical format version is unsupported.
    UnsupportedVersion,
    /// Bytes do not describe a well-formed token or claim.
    Malformed,
    /// An installation or workspace identifier appeared in the wrong role.
    InvalidRole,
    /// The token ID does not match its canonical unsigned bytes.
    ContentHashMismatch,
    /// The invitation token signature is invalid.
    InvalidTokenSignature,
    /// The invitee claim signature is invalid.
    InvalidClaimSignature,
    /// The claim is not bound to the expected invitee.
    ClaimTargetMismatch,
    /// The requested workspace does not exist on the issuing installation.
    WorkspaceNotFound,
    /// The local installation did not issue the token.
    WrongIssuer,
    /// The local installation is not a current workspace manager.
    NotManager,
    /// The invitation has expired or was issued with no usable lifetime.
    Expired,
    /// The invitation nonce has already been consumed.
    AlreadyUsed,
    /// Durable storage failed while processing the invitation.
    Storage(StorageError),
    /// The authenticated admission could not be accepted.
    Membership(MembershipError),
    /// A canonical field or complete value exceeds its version-1 bound.
    DecodeLimit,
}

impl fmt::Display for InvitationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::RuntimeUnavailable => "the invitation runtime is not available",
            Self::UnsupportedVersion => "unsupported invitation format version",
            Self::Malformed => "malformed invitation value",
            Self::InvalidRole => "invitation identifier has the wrong role",
            Self::ContentHashMismatch => "invitation token content hash does not match",
            Self::InvalidTokenSignature => "invitation token signature is invalid",
            Self::InvalidClaimSignature => "invitation claim signature is invalid",
            Self::ClaimTargetMismatch => "invitation claim target does not match",
            Self::WorkspaceNotFound => "invitation workspace was not found",
            Self::WrongIssuer => "invitation must be redeemed by its issuing installation",
            Self::NotManager => "invitation issuer is not a current workspace manager",
            Self::Expired => "invitation has expired",
            Self::AlreadyUsed => "invitation has already been used",
            Self::Storage(error) => return error.fmt(formatter),
            Self::Membership(error) => return error.fmt(formatter),
            Self::DecodeLimit => "invitation value exceeds a decode bound",
        })
    }
}

impl Error for InvitationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            Self::Membership(error) => Some(error),
            _ => None,
        }
    }
}

/// A stable error returned by Workspace onboarding operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OnboardingError {
    /// The transfer value or protocol input is malformed or unsupported.
    InvalidInput,
    /// An invitation, claim, or authenticated peer binding is invalid.
    Authentication,
    /// The unused invitation has expired.
    Expired,
    /// Another Installation has already used the invitation.
    AlreadyUsed,
    /// Public endpoint addressing did not become ready in time.
    Readiness,
    /// A bounded onboarding operation exceeded its time budget.
    Timeout,
    /// The runtime has reached an onboarding concurrency or size limit.
    Overload,
    /// A received Workspace bootstrap is invalid or incomplete.
    InvalidBootstrap,
    /// Durable local Workspace state conflicts with the received Workspace.
    LocalConflict,
    /// The recovered admission no longer grants current membership.
    AdmissionNoLongerCurrent,
    /// Storage may have committed and the runtime must be reopened.
    UncertainStorageOutcome,
    /// Runtime shutdown has started, so no new onboarding work is accepted.
    Shutdown,
}

impl OnboardingError {
    /// Reports whether retrying the same operation without changing its input may succeed.
    #[must_use]
    pub const fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::Readiness | Self::Timeout | Self::Overload | Self::Shutdown
        )
    }
}

impl fmt::Display for OnboardingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidInput => "invalid workspace onboarding input",
            Self::Authentication => "workspace onboarding authentication failed",
            Self::Expired => "workspace invitation has expired",
            Self::AlreadyUsed => "workspace invitation was used by another installation",
            Self::Readiness => "workspace invitation addressing is not ready",
            Self::Timeout => "workspace onboarding timed out",
            Self::Overload => "workspace onboarding is overloaded",
            Self::InvalidBootstrap => "workspace bootstrap is invalid",
            Self::LocalConflict => "local workspace state conflicts with the invitation",
            Self::AdmissionNoLongerCurrent => "the recovered admission is no longer current",
            Self::UncertainStorageOutcome => "workspace onboarding storage outcome is uncertain",
            Self::Shutdown => "workspace onboarding is unavailable during shutdown",
        })
    }
}

impl Error for OnboardingError {}

/// An error while registering or receiving membership transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SubscriptionError {
    /// The requested workspace does not exist.
    WorkspaceNotFound,
    /// The supplied cursor is ahead of durable workspace state.
    CursorAhead,
    /// Durable transition storage could not be read.
    Storage(StorageError),
}

impl fmt::Display for SubscriptionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkspaceNotFound => formatter.write_str("subscription workspace was not found"),
            Self::CursorAhead => {
                formatter.write_str("subscription cursor is ahead of durable state")
            }
            Self::Storage(error) => error.fmt(formatter),
        }
    }
}

impl Error for SubscriptionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            _ => None,
        }
    }
}

/// An error returned while opening the core runtime.
#[derive(Debug)]
#[non_exhaustive]
pub enum CoreError {
    /// Runtime configuration could not be accepted.
    Configuration(ConfigurationError),
    /// Installation identity initialization failed.
    Identity(IdentityError),
    /// Durable storage initialization or recovery failed.
    Storage(StorageError),
}

impl fmt::Display for CoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(error) => error.fmt(formatter),
            Self::Identity(error) => error.fmt(formatter),
            Self::Storage(error) => error.fmt(formatter),
        }
    }
}

impl From<StorageError> for CoreError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl Error for CoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Configuration(error) => Some(error),
            Self::Identity(error) => Some(error),
            Self::Storage(error) => Some(error),
        }
    }
}
