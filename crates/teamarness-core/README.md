# teamarness-core

`teamarness-core` is the durable identity, canonical-membership, Workspace onboarding, and bounded immutable-record replication library. Applications use the public `CoreRuntime` seam; the crate owns installation key custody, SQLite persistence, authenticated membership replay, one shared Iroh Endpoint/Router, recovery, and transition publication.

## Core Runtime API

Open one runtime for an installation-private data directory:

```rust
use teamarness_core::{CoreRuntime, CoreRuntimeConfig, MembershipEpoch};

let runtime = CoreRuntime::open(CoreRuntimeConfig::new("private-data")).await?;
let identity = runtime.public_identity();
let workspace = runtime.create_workspace("Team")?;
let snapshot = runtime.membership_snapshot(workspace.id())?;
let transitions = runtime.subscribe_membership(workspace.id(), MembershipEpoch::new(0))?;
runtime.shutdown().await?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

The public API supports:

- asynchronously opening an installation, authenticating storage, recovering membership and synchronization state, and starting one shared Iroh Endpoint/Router from the persisted Installation key;
- domain-separated signing and public signature verification without secret access;
- creating and listing Workspaces;
- creating one opaque, fixed-lifetime `WorkspaceInvitation` as the creator-manager;
- joining an invited Workspace through authenticated Iroh transport and returning owned `WorkspaceSummary` and `MembershipSnapshot` values only after atomic local import;
- recovering the same committed admission when response delivery is interrupted, including after invitation expiry while that Installation remains current;
- reading the current membership snapshot and content-derived `MembershipVersion`;
- requesting a finite bounded immutable-record reconciliation with a current peer through the shared Core network; the production path uses a private P2Panda operation projection and LogSync, while the public capability remains semantic and package-neutral;
- revoking an admitted member;
- querying declared-time membership history; and
- catching up and following durable post-genesis transitions after a local `MembershipEpoch` cursor. Each transition reports the `MembershipChange` it made — the admitted or revoked Installation — and the Installation that authored it, identically for durable catch-up and live delivery.

Invitation creation waits at most five seconds for production default-relay readiness and starts the fixed 15-minute invitation lifetime only after readiness succeeds. The returned `WorkspaceInvitation` is an opaque, sensitive bearer value: applications may parse, display, copy, and submit its text, but must not interpret its private wire representation.

A successful onboarding response contains the issuer's complete accepted membership state at one consistency boundary. Version 1 limits that bootstrap to 8 MiB and 1,024 canonical operations. The same Installation can recover its one committed admission after interrupted response delivery, including after the invitation expires, provided it is still a current member. Another Installation cannot reuse the invitation. Onboarding is bounded and one-shot; continuous membership replication is a separate protocol.

Runtime shutdown rejects new onboarding transport work, leaves local identity, Workspace, and membership commands available, and must be awaited. `Drop` is an abnormal best-effort cleanup fallback only.

Low-level token issuance, claim creation, and redemption were intentionally removed from the public interface; they are private onboarding mechanics. Installation secrets, raw key bytes, SQLite connections, Iroh endpoint, router, connection, stream, and ticket handles, bootstrap and canonical-operation values, membership adapters, mutable p2panda `GroupCrdt` state, fault hooks, and transition publishers are not public.

## Declared-time warning

`CoreRuntime::was_member` reports authenticated **author-declared event time**. Admission is inclusive and revocation is exclusive at their signed authored milliseconds. Late or backdated operations can revise an earlier answer after synchronization. Never use this historical query to authorize a write; security-sensitive work must use current membership and its `MembershipVersion`.

## Generic synchronization foundation

The immutable-record capability is generic: Packages provide bounded canonical bytes and purposes, while Core owns signing, sealed persistence, membership gates, duplicate handling, ordering, cursors, peer reconciliation, and secret custody. Each accepted sealed record has a private authenticated P2Panda projection for production LogSync. Teamarness semantic rows and the projection commit together; an invalid or out-of-order remote operation is rejected before it can leave a partial journal row. Duplicate delivery is idempotent.

Synchronization wakes are durable and coalesced. A private Core worker schedules at most four finite sessions, resumes work after restart, and is awaited before the shared Router and Endpoint shut down. No P2Panda, Iroh, SQLite, endpoint, or raw key type crosses the Package boundary.

Every synchronization resource is bounded, and `synchronization/limits.rs` is the single place that names a bound. A responder accepts at most eight concurrent sessions and two per peer, before it reads anything, and waits at most five seconds for the opening message. After the handshake a frame is at most 1 MiB, and one session exchanges at most 2,048 operations, 8 MiB of operation bytes, and 64 other frames in each direction. Outgoing operations are read from storage in pages within a send budget and written with transport backpressure; a session that stops at its bound says so, and a later session continues. A record subscription holds no queue in Core: every commit advances a per-scope journal head, and a subscriber reads pages of at most 32 committed records after its own position. Scheduling is per peer and durable per (scope, peer): backoff starts only after a failure and only for that peer, at most four sessions run and one per peer, and shutdown waits at most two seconds. A record or scope that fails authentication is quarantined and skipped instead of failing `CoreRuntime::open`, and restart validates only what was committed since the previous validated start. A subscription reports `waiting_for_history_access`, `offline`, `catching_up`, or `current` with a pending record count and stable warning codes.

Canonical membership replicates over `teamarness/membership-sync/1`, independent of whether any Package has records to synchronize: one finite session per Workspace and peer reconciles sorted operation-ID sets and exchanges the canonical envelopes each side lacks, in causal order. Package synchronization uses `teamarness/package-sync/2` and serves nothing — not even the Workspace author list — until both this Installation and the authenticated remote peer are current Members, and accepting a revocation cancels live sessions with the revoked Installation. A received operation whose author is not yet known, or whose scope has no Content Key yet, is parked within fixed per-scope and per-peer bounds and applied when that state arrives. A scope's first Content Key is accepted only with the Workspace creator's signed origin. Protection controls are sealed to their recipient Installation and signed through the domain-separated signing interface.

## Database ownership and recovery

A runtime owns one `core.sqlite3` database beneath its private data directory. That database contains the installation identity, workspace creation state, immutable canonical membership envelopes, invitation spends, derived snapshots, membership versions, epochs, and transition evidence. Consumers must not open or mutate it while a runtime is active.

Workspace creation records generated identity and signed genesis material once, then rolls a committed pending request forward on `CoreRuntime::open`. Invitation redemption atomically spends the nonce and commits the authenticated admission, derived state, epoch, and transition. Startup authenticates canonical operations, reconstructs projections, repairs safe derived-state drift, and rejects missing or contradictory canonical evidence. If a commit outcome is uncertain, the active runtime is poisoned and must be dropped and reopened before another command.

## Explicit exclusions

The Core Runtime does not provide:

- unbounded synchronization sessions: production LogSync sessions are finite and bounded to 4,096 operations, 64 MiB, and 30 seconds;
- distributed invitation-spend coordination across managers or databases;
- a second-manager path, promotion, demotion, arbitrary access levels, nested groups, or re-admission after revocation;
- multi-manager conflict fixtures (mutual manager removal and dependent transitive invalidation are deferred until a supported second-manager path exists);
- human-scoped identity behavior;
- package lifecycle, package storage, package transport, package keys, revocation-driven package-key rotation, or cryptographic protection of future package content from revoked Installations (version 1 gates synchronization only);
- a Tauri shell, package UI, or product-domain packages; or
- authorization based on declared historical timestamps.

Version 1 has one permanent creator-manager. Invitations admit installations with fixed member/write access, and the issuer is the authority that redeems its token.
