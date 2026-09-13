# resonance-core

`resonance-core` is the Phase 1 durable identity and canonical-membership library. Applications use the public `CoreRuntime` seam; the crate owns installation key custody, SQLite persistence, authenticated membership replay, recovery, and transition publication.

## Phase 1 API

Open one runtime for an installation-private data directory:

```rust
use resonance_core::{CoreRuntime, CoreRuntimeConfig, MembershipEpoch};

let runtime = CoreRuntime::open(CoreRuntimeConfig::new("private-data")).await?;
let identity = runtime.public_identity();
let workspace = runtime.create_workspace("Team")?;
let snapshot = runtime.membership_snapshot(workspace.id())?;
let transitions = runtime.subscribe_membership(workspace.id(), MembershipEpoch::new(0))?;
runtime.shutdown().await?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

The public API supports:

- asynchronously opening an installation, authenticating storage, recovering membership, and starting one Iroh onboarding endpoint from the persisted Installation key;
- domain-separated signing and public signature verification without secret access;
- creating and listing workspaces;
- issuing an expiring invitation as the creator-manager, claiming it on another installation, and redeeming it once on the issuer;
- reading the current membership snapshot and content-derived `MembershipVersion`;
- revoking an admitted member;
- querying declared-time membership history; and
- catching up and following durable post-genesis transitions after a local `MembershipEpoch` cursor.

Invitation creation waits at most five seconds for production default-relay readiness and starts the fixed 15-minute invitation lifetime only after readiness succeeds. Runtime shutdown rejects new onboarding transport work, leaves local identity, Workspace, and membership commands available, and must be awaited. `Drop` is an abnormal best-effort cleanup fallback only.

Installation secrets, raw key bytes, SQLite connections, Iroh endpoint and router handles, canonical-operation adapters, mutable p2panda `GroupCrdt` state, fault hooks, and transition publishers are not public.

## Declared-time warning

`CoreRuntime::was_member` reports authenticated **author-declared event time**. Admission is inclusive and revocation is exclusive at their signed authored milliseconds. Late or backdated operations can revise an earlier answer after synchronization. Never use this historical query to authorize a write; security-sensitive work must use current membership and its `MembershipVersion`.

## Database ownership and recovery

A runtime owns one `core.sqlite3` database beneath its private data directory. That database contains the installation identity, workspace creation state, immutable canonical membership envelopes, invitation spends, derived snapshots, membership versions, epochs, and transition evidence. Consumers must not open or mutate it while a runtime is active.

Workspace creation records generated identity and signed genesis material once, then rolls a committed pending request forward on `CoreRuntime::open`. Invitation redemption atomically spends the nonce and commits the authenticated admission, derived state, epoch, and transition. Startup authenticates canonical operations, reconstructs projections, repairs safe derived-state drift, and rejects missing or contradictory canonical evidence. If a commit outcome is uncertain, the active runtime is poisoned and must be dropped and reopened before another command.

## Explicit Phase 1 exclusions

Phase 1 does not provide:

- completed remote admission, Workspace bootstrap import, peer discovery, or membership synchronization;
- distributed invitation-spend coordination across managers or databases;
- a second-manager path, promotion, demotion, arbitrary access levels, nested groups, or re-admission after revocation;
- multi-manager conflict fixtures (mutual manager removal and dependent transitive invalidation are deferred until a supported second-manager path exists);
- human-scoped identity behavior;
- package lifecycle, package storage, package transport, package keys, revocation-driven package-key rotation, or enforcement over future package content;
- a Tauri shell, package UI, or product-domain packages; or
- authorization based on declared historical timestamps.

Version 1 has one permanent creator-manager. Invitations admit installations with fixed member/write access, and the issuer is the authority that redeems its token.
