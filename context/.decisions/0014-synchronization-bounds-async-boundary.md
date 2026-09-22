# 0014 — Synchronization resource bounds, async boundary, and availability events

**Status:** accepted (2026-09-18)
**Supersedes:** ADR 0009 (Consequences, first bullet: restart validation of every projection row and sealed envelope before capability calls are accepted); ADR 0008 (Decision, the sentence naming four private Core seams with deterministic adapters for compatibility tests)

## Context

After membership replication and synchronization authorization (ADR 0013), Core synchronization converged but was not bounded. Record subscriptions copied every committed record into unbounded per-subscriber queues and decrypted the whole backlog before returning. Each session loaded and cloned the whole scope log, buffered both directions in unbounded channels, and accepted frames up to 64 MiB. The package-sync acceptor had no concurrency limit.

Synchronous SQLite and cryptographic work ran on Tokio worker threads while holding the runtime mutex, so the Package capability-call deadline could not fire. The scheduler dialled every member every five seconds, slept before every attempt, dropped work when a lock was busy, and could miss a revocation while sleeping. Restart decrypted every record, and one bad row prevented the runtime from opening. Remote order keys were trusted as sent. Packages received no availability events, so a waiting, offline, or wedged subscription looked the same as a current one.

## Decision

### Async boundary

Async code enters Core state only through a blocking-thread boundary. The runtime keeps one synchronous state lock and a synchronous command API. Work done while holding the lock is bounded: one local append, one page of at most 32 subscription records, one page of at most 256 log rows, or one batch of at most 16 remote operations. No lock guard is held across an await.

Package capability calls run their Core work on blocking threads, limited to eight concurrent calls across all Packages, so the ten-second call deadline takes effect. `overloaded` and `deadline_exceeded` are retryable. An `append` that times out after starting reports an uncertain outcome; it is content-idempotent.

### Bounded synchronization sessions

A package synchronization responder accepts at most eight concurrent sessions and two per peer, before it reads anything, and waits at most five seconds for the opening message. After the handshake, a frame is at most 1 MiB. A session sends and receives at most 2,048 operations, 8 MiB of operation bytes, and 64 other frames in each direction. A peer that exceeds a bound is disconnected, and operations already applied remain. A session that stops at its send bound says so, and the scheduler continues in a later session.

Outgoing operations are read lazily from storage in pages and written with transport backpressure. Incoming frames are read continuously, within the session bounds, so two peers sending to each other cannot block each other.

### Record delivery

A record subscription holds no queue in Core. Every commit advances a per-scope journal head, and a subscriber reads committed records after its own position, in pages sized by stream credit. Live delivery therefore cannot diverge from the durable journal.

A cursor from another journal or ahead of the journal head is rejected. The Package Host closes the stream with `cursor_invalid`, and the Package resets its cursor and replays by record identity.

### Scheduling

The synchronization scheduler keeps durable progress per scope and peer, and per Workspace and peer for membership. It runs at most four sessions, at most one per peer, and at most two for peers that keep failing. Backoff starts only after a failure and applies only to that peer, from 500 milliseconds up to 60 seconds. Wakes coalesce. Record scopes are reconciled at least every 60 seconds and membership every 30 seconds. Revocation cancels live sessions, and a revoked peer is never dialled from backoff. Shutdown stops scheduling immediately and waits at most two seconds for sessions.

### Validation and quarantine

Restart validates each scope's binding and content-key identity, and every record committed since the previous validated restart. Every record is authenticated again when it is read by a subscriber or served to a peer. A record that fails is quarantined and skipped, and a scope that fails is quarantined and stops synchronizing. Neither prevents the runtime or other scopes from working.

A received record's order key must be version 1 and must embed the record's identity. Its height may exceed the receiver's highest height in that scope by at most 2^20.

### Availability events

Record subscriptions report `waiting_for_history_access`, `offline`, `catching_up`, or `current`, with a pending record count. These are derived from delivery position, content-key presence, quarantine, local membership, parked records, and the outcome of recent sessions with each peer. A scope that is quarantined, a local Installation that has been revoked, and a scope whose peers all failed all report `offline`; the reason is carried by the stable warning codes `record_quarantined`, `scope_quarantined`, and `membership_revoked`.

## Rejected alternatives

### A storage actor thread

Rejected for now. It would rewrite every state access in the runtime and the deadline-bounded onboarding import. The blocking boundary keeps that path open.

### Bounded per-subscriber channels

Rejected. They still copy payloads per subscriber, need a lag policy that duplicates the durable path, and can still miss delivery.

### Bounded queues in both directions of a session

Rejected. The upstream LogSync protocol does not read while it sends, so two peers with large backlogs could block each other.

### Keeping full restart validation

Rejected. Startup time grew with the whole history, and one bad row stopped the installation.

### Verifying order-key heights through parent frontiers

Deferred. It changes the sealed record format and belongs with record-version work in roadmap Phase 5.

## Consequences

- ADR 0009's worker limit of four finite sessions is kept and is now enforced by tested production code. The deterministic seam adapters named by ADR 0008 are removed, because production never used them; Core's replication and storage adapters remain private.
- An old tampered row is detected when it is next read or served, not at startup. Anyone who can modify the database can already read its keys (tracked separately).
- A Member can still place its own record up to 2^20 heights ahead of other records.
- Replaying received membership operations still replays the whole membership DAG per operation. This is deferred to roadmap Phase 5; realistic Workspaces are small, and the lock is released between operations.
- The capability contract stays version 1.0: state and warning events were already defined, and `cursor_invalid` is an additional stream close reason.

## Evidence

- `crates/teamarness-core/src/blocking.rs`: blocking boundary.
- `crates/teamarness-core/src/synchronization/limits.rs`: every bound.
- `crates/teamarness-core/src/synchronization/frames.rs`, `exchange.rs`, `log_view.rs`: bounded sessions.
- `crates/teamarness-core/src/synchronization/record_subscription.rs`, `health.rs`: delivery and availability.
- `crates/teamarness-core/src/synchronization/scheduler.rs`, `worker.rs`: scheduling.
- `crates/teamarness-core/src/synchronization/quarantine.rs` and `crates/teamarness-core/migrations/0011_sync_bounds_and_recovery.sql`: validation and quarantine.
- `crates/teamarness-package-host/src/capability.rs`: call boundary and stream events.
- Tests: duplex-based frame, budget, and no-deadlock tests; paused-time scheduler tests; `current_thread` tests proving work is off the executor; restart quarantine tests; a 5,000-record multi-installation catch-up; Host state-event tests.
- **Weaknesses:** in-process tests use loopback transport. The 2,048-operation bound assumes applies of at most about 5 ms each; slower disks need a lower bound. Worst-case memory under authenticated abuse is about 200 MiB across twelve sessions; it was computed, not measured.
