# DELTA-007 — Synchronization bounds, backpressure, and state events are incomplete

**Status:** closed (2026-09-18)

Resolved by ADR 0014 (architecture-review workstream, remediation plan phase 4 — synchronization resource bounds and async correctness).

## Divergence

`TEAM.SYS.TRANS-R03` and `TEAM.SYS.PKG-R10` describe bounded sessions, backpressure, and a subscription stream. In fact:

- no global memory bound or backpressure exists on the sync protocols;
- the bounded scheduler components are test-only;
- blocking SQLite and mutex work runs on async worker threads;
- the capability-call timeout cannot fire;
- `state` and `warning` subscription events are never emitted, so Packages infer availability.

## Impact

Memory can grow without limit under load or from unauthenticated peers, and timeouts do not bound Package calls.

## Resolution

Core bounds every session and frame, reads outgoing operations lazily within a send budget, holds no per-subscriber queue, runs storage and cryptographic work on blocking threads, schedules per peer with durable progress, quarantines a corrupt record or scope instead of failing to open, and reports availability and warning events to Packages.

The bounds live in `crates/teamarness-core/src/synchronization/limits.rs`. The session and delivery paths are `frames.rs`, `exchange.rs`, `log_view.rs`, and `record_subscription.rs`; availability is `health.rs`; scheduling is `scheduler.rs`; isolation is `quarantine.rs`; and the asynchronous boundary is `crates/teamarness-core/src/blocking.rs`.

## Tracking

- Review findings SYN-07, SEC-03, SYN-11, CQ-01/SYN-10/SEC-09, CQ-03, CQ-04, SYN-08
- `TEAM.SYS.TRANS-R03`, `TEAM.SYS.PKG-R10`, `TEAM.SYS.CONV-R14`
