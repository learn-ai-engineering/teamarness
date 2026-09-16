# 0009 — Core synchronization foundation

## Status

Accepted for the generic immutable-record capability (Phase 5, 2026-09-15).

## Decision

Core uses a hybrid durable journal for `teamarness.synchronization.immutable-record-log` 1.0. Teamarness semantic rows remain authoritative for `RecordId`, `OrderKey`, sealed envelopes, delivery cursors, membership evidence, and package scope. Each accepted record also has a private P2Panda operation projection containing the authenticated header, payload hash, backlink, author sequence, and true height used by bounded production LogSync.

A production LogSync receive is validated before it can mutate the Teamarness journal. The Teamarness operation and its P2Panda projection are committed in one SQLite transaction. Sequence, backlink, scope, signature, ciphertext binding, and content-key checks reject malformed or out-of-order operations without leaving partial accepted rows. Duplicate operation delivery is idempotent.

One Core-owned synchronization worker consumes durable wakes, schedules at most four finite peer sessions, and is awaited before the shared Router and Endpoint shut down. P2Panda and Iroh types remain private implementation details; Packages receive only the semantic append, subscribe, and open capability.

## Consequences

- Restart validates the P2Panda projection and sealed envelopes before capability calls are accepted.
- A finite session may require continuation; durable wake generations preserve that work without unbounded transport.
- Core retains raw content-key custody and the version-1 one-key/no-rotation limitation. Package persistence and diagnostics remain ciphertext-only.
- A current connected Member may attest and forward a record signed by any identity in accepted Workspace Membership history. Version 1 does not bind records to a Membership Version or trusted authored time, so Core cannot independently prove that a forwarded record from a later-revoked author predates revocation. Strong temporal revocation requires the Phase 5 key rotation and record-version work; Phase 4 relies on the current forwarder's authorization while preserving the original author's signature.
- Invalid remote operations are isolated from valid peers and cannot poison the accepted Teamarness journal through a partial write.
- The shared Network remains the sole Endpoint/Router owner. Downstream Host, SDK, and Package phases must not expose or depend on P2Panda/Iroh types.

## Evidence

- `crates/teamarness-core/src/synchronization/operation_store.rs`
- `crates/teamarness-core/src/synchronization/mod.rs`
- `crates/teamarness-core/src/synchronization/worker.rs`
- `crates/teamarness-core/src/synchronization/network_handler.rs`
- Core production partition/restart convergence and projection-recovery tests
