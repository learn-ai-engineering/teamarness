# 0008 — Immutable record capability contract and protection boundary

**Status:** accepted (2026-09-15)

## Context

Conversations needs offline immutable records without making the Core Runtime
know Conversations domain operations. The Package Host already has implicit,
generation-scoped capability grants. P2Panda Core, Sync, and Encryption 0.7.1
provide useful internal primitives, but their operation, transport, and secret
types must not become a Package contract. P2Panda Spaces and Net overlap
Teamarness membership and shared Iroh endpoint ownership.

## Decision

Teamarness provides `teamarness.synchronization.immutable-record-log` 1.0 with
only `append`, `subscribe`, and `open`. Package Identity, Workspace,
Installation, capability version, and process generation are implicit in the
grant. `append` accepts a visible ASCII purpose and bounded canonical bytes,
commits before success, and is content-idempotent. `subscribe` is an at-least-
once, credit-controlled stream from an Installation-local cursor. `open` accepts
1–32 sealed envelopes and at most 512 KiB decoded input.

Core keeps `RecordId`, causal `OrderKey`, and local `Cursor` separate. Record
identity is an authenticated BLAKE3 digest; order is versioned height plus
identity; cursor is delivery progress and is not an order offset. The sealed
envelope is a Teamarness-owned versioned binary encoding and contains no Rust or
P2Panda representation.

The four private Core seams are `ReplicatedRecordStore`, `ReplicaEngine`,
`PeerSessionTransport`, and `RecordProtection`. Production adapters may use
P2Panda Core, Sync, and Encryption; deterministic adapters are used for
compatibility tests. Phase 1's bounded Encryption gate selects the settled
fallback: P2Panda Encryption's direct XChaCha20-Poly1305 primitive plus
Teamarness-owned creator/current-holder grant projection. The complete group
state machine remains deferred until its persistence adapters can be proven. Canonical Membership remains the existing Core authority.
P2Panda Spaces and Net are not dependencies. Version 1 uses creator-only first
key bootstrap, current-holder grants of one content key, and does not call
protection remove, update, deletion, or rotation. Revocation closes future
append and synchronization gates but does not claim to remove retained old key
material in this version.

Packages own canonical domain bytes, validation, projections, policy, sealed
caches, and opaque indexes. The Package Process contract carries semantic
values only and never exposes Iroh, P2Panda, SQLite, private-key, or raw-key
types.

## Consequences

The capability schema and golden fixtures are closed and language-neutral.
Package projections must insert by `OrderKey` and commit their Core cursor in
their own transaction; a late earlier-sorting record cannot be skipped. Core
can change its replication and storage adapters without changing Package code.
A future phase may add durable journal and network implementations behind these
seams, but it must preserve this contract.

## Evidence

- `contracts/package-capabilities/v1/immutable-record-log.schema.json`
- `contracts/package-capabilities/v1/immutable-record.golden.json`
- `crates/teamarness-core/src/synchronization/`
- `.myflow/workstreams/conversations-package/design/20260915T051922Z_conversations-package.md`
