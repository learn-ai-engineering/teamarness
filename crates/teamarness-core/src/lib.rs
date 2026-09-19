//! Teamarness's durable identity and canonical membership runtime.
//!
//! # Dependency boundary
//!
//! Iroh types appear in the private `network`, `onboarding`, `membership_sync`
//! and `synchronization` modules, and in `runtime` internals. The p2panda
//! crates are used by `membership` and `synchronization`. `rusqlite` stays
//! behind `storage` and the private `RuntimeState`. The two Ed25519 stacks
//! those dependencies carry meet only in `synchronization::key_bridge`.
//!
//! No dependency type appears in a public signature, which
//! `tests/public_api.rs` enforces with compile-fail fixtures. Canonical
//! membership bytes do not depend on serde-derived p2panda representations.
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
mod diag;
mod encoding;
mod error;
mod runtime;

// These boundaries are intentionally private: nothing outside the crate can
// name what is behind them.
mod identity;
mod invitation;
mod membership;
mod membership_sync;
mod network;
mod onboarding;
mod storage;
mod subscription;
mod synchronization;
mod workspace;

pub use error::{
    CoreError, IdentityError, InvitationError, MembershipError, OnboardingError, ShutdownError,
    StorageError, SubscriptionError, ValueParseError, WorkspaceError,
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
