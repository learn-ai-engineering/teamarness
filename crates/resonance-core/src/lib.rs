//! Resonance's durable identity and canonical membership runtime.
//!
//! # Dependency boundary
//!
//! `iroh-base` supplies only installation key types in Phase 1; the full Iroh
//! endpoint crate is intentionally absent. `p2panda-auth` and `p2panda-core`
//! are isolated behind the private membership adapter, and `rusqlite` is
//! isolated behind private storage modules. Canonical membership bytes do not
//! depend on serde-derived p2panda representations.
//!
//! # Visibility boundary
//!
//! Callers use [`CoreRuntime`] and the domain values re-exported here. Identity
//! secrets, raw key bytes, SQLite connections, transport handles, p2panda
//! adapters, mutable CRDT state, recovery hooks, and notification publishers
//! are private implementation details. New public items must describe a
//! semantic core capability rather than expose one of those dependencies.

#![forbid(unsafe_code)]

mod error;
mod runtime;

// These boundaries are intentionally private. Later phases deepen them without
// changing which implementation details callers can name.
mod identity {}
mod invitation {}
mod membership {}
mod storage {}
mod subscription {}
mod workspace {}

pub use error::{
    ConfigurationError, CoreError, IdentityError, InvitationError, MembershipError, StorageError,
    SubscriptionError, ValueParseError, WorkspaceError,
};
pub use runtime::{
    CoreRuntime, CoreRuntimeConfig, InvitationClaim, InvitationToken, MemberSnapshot,
    MembershipEpoch, MembershipOperationId, MembershipRole, MembershipSnapshot,
    MembershipSnapshotDigest, MembershipSubscription, MembershipTransition, MembershipVersion,
    PublicIdentity, PublicSignature, SigningContext, WorkspaceId, WorkspaceSummary,
};
