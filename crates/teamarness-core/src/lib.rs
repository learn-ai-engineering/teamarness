//! Teamarness's durable identity and canonical membership runtime.
//!
//! # Dependency boundary
//!
//! Iroh endpoint and ticket crates are isolated behind the private onboarding
//! boundary. `p2panda-auth` and `p2panda-core` are isolated behind the private
//! membership adapter, and `rusqlite` is isolated behind private storage
//! modules. Canonical membership bytes do not depend on serde-derived p2panda
//! representations.
//!
//! # Visibility boundary
//!
//! Callers use [`CoreRuntime`] and the domain values re-exported here. Identity
//! secrets, raw key bytes, SQLite connections, transport handles, p2panda
//! adapters, mutable CRDT state, recovery hooks, and notification publishers
//! are private implementation details. New public items must describe a
//! semantic core capability rather than expose one of those dependencies.

#![forbid(unsafe_code)]

mod blocking;
mod error;
mod runtime;

// These boundaries are intentionally private. Later phases deepen them without
// changing which implementation details callers can name.
mod identity;
// These authenticated private values are constructed by the workspace and
// redemption workflows in later phases.
#[allow(dead_code)]
mod invitation;
mod membership;
mod membership_sync;
#[allow(dead_code)]
mod network;
#[allow(dead_code)]
mod onboarding;
mod storage;
mod subscription;
#[allow(dead_code)]
mod synchronization;
mod workspace;

pub use error::{
    ConfigurationError, CoreError, IdentityError, InvitationError, MembershipError,
    OnboardingError, StorageError, SubscriptionError, ValueParseError, WorkspaceError,
};
pub use membership::MembershipOperationId;
pub use onboarding::WorkspaceInvitation;
/// Test observation hooks. See the `test-hooks` feature.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub use runtime::RuntimeStateHold;
pub use runtime::{
    CoreRuntime, CoreRuntimeConfig, MemberSnapshot, MembershipChange, MembershipEpoch,
    MembershipRole, MembershipSnapshot, MembershipSnapshotDigest, MembershipSubscription,
    MembershipTransition, MembershipVersion, PublicIdentity, PublicSignature, SigningContext,
    WorkspaceId, WorkspaceSummary,
};
/// How many records subscriptions have decoded for one scope since the process
/// began. See the `test-hooks` feature.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
#[must_use]
pub fn records_decoded_for_tests(workspace: WorkspaceId, package: &str) -> usize {
    synchronization::record_subscription::records_decoded_for_test(synchronization::scope_digest(
        package, workspace,
    ))
}

/// How many membership subscriptions have been opened for one Workspace since
/// the process began. See the `test-hooks` feature.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
#[must_use]
pub fn membership_subscriptions_opened_for_tests(workspace: WorkspaceId) -> usize {
    runtime::membership_subscriptions_opened(workspace)
}

pub use synchronization::{
    Cursor, ImmutableRecord, OpenedRecord, OrderKey, RecordAvailability, RecordEvent, RecordId,
    RecordPage, RecordSubscription, SyncAvailability, SyncWarning, SynchronizationError,
};
