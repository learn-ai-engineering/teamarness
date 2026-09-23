# 0021 — Fair class selection under the synchronization cap

**Status:** accepted (2026-09-23)
**Refines:** ADR 0014 (synchronization resource bounds and scheduling), ADR 0020 (subscription-driven Content Key acquisition, TRANS-R08).

## Context

The synchronization scheduler keeps a bounded in-memory job set (`max_scheduled_jobs`, currently 4,096) that a durable load rebuilds on each reload. The load lists forced membership, then ordinary membership, then held-record scopes, then pending key demand (`worker.rs::load_due_jobs`), applying the cap while enumerating records — before the demand loop runs. Held-record jobs that fail are not consumed, per-peer backoff only delays dialing, so the same held jobs can fill the cap on every reload and the pending key-demand loop is never reached. Independent review confirmed this as a P1 (`SF-001`): a reachable key holder can remain uncontacted indefinitely, violating TRANS-R08's prompt, bounded demand scheduling.

## Decision

Non-membership work is selected at the durable-load boundary with **fair-queueing alternation across two classes** — held-record jobs and pending key-demand jobs — after membership precedence is applied in full:

- Per slot, the class with the fewer cumulative served slots is chosen; when only one class has due work, all remaining slots go to it (work-conserving). Served counters are normalized after each load so the difference stays bounded.
- Within a class, a **volatile resume cursor** (worker-owned, in-memory) continues the deterministic enumeration order (`record_wakes`, `key_demands`, then sorted per-workspace peers) with wrap-around cycling and at-most-once selection per load.
- The cursor resets on restart; demand rows are durable, so restart loses no demand and the deterministic bound applies per run. No schema change, no durable cursor.
- For a stable due set of D demand and R record units with m non-membership slots per reload, every demand unit is selected within at most `⌈2·max(D,R)/m⌉` reloads (records symmetric), so both classes make progress and repeated failures do not break the bound.
- Membership, cap, `SchedulerCore` dispatch, per-peer backoff, failing-peer slots, forced/in-flight retention, and demand consume/retire semantics are unchanged. Listing is orthogonal to consumption: an unconsumed pair stays one stable (scope, peer) row, and only a completed non-truncated session consumes it.

## Rejected alternatives

- **Fixed one-slot demand reservation** — starves every demand after the first and wastes slots.
- **Demand-first reversal** — starves held-record reconciliation, which also carries admission work.
- **Reorder inside `SchedulerCore`** — the cap is applied before dispatch; omitted classes are unrecoverable.
- **Index rotation without wrap** — tail units are never re-selected.
- **Durable cursor** — requires schema change; excluded by scope (no schema change; no across-restart cursor guarantee implied).
- **Bare SQL `LIMIT` paging** — permanently omits later rows; continuation-aware paging is a separate P2 (`SM-001`), recorded not fixed.
- **Library-provided scheduler** — the team owns the scheduler per ADR 0014; a library swap is a larger stack change and was declined by the developer (their note recorded in the workstream manifest), with the localized design accepted instead.

## Consequences

- Total scheduled jobs remain within the existing cap; membership stays first; held-record reconciliation still progresses.
- Pending key demand is always listed within a bounded number of reloads, satisfying TRANS-R08's "promptly under the existing job bounds" clause under saturation.
- Restart keeps demand durable; only the in-memory cursor resets.
- The unbounded wake-enumeration scan (`SM-001`) is outside this decision and remains a recorded P2.
- Trace comments in the changed code link the seam to TRANS-R08 and ADR 0020.

## Evidence

- `crates/teamarness-core/src/synchronization/worker.rs` — `load_due_jobs` cap-before-demand defect and test fixture seam.
- `crates/teamarness-core/src/synchronization/scheduler.rs` — `replace_due`/`next_dispatch` boundary (unchanged).
- `crates/teamarness-core/src/synchronization/limits.rs` — injectable bounds.
- `crates/teamarness-core/migrations/0013_scope_key_demand.sql` — durable, deduplicated demand.
- Workstream `content-key-demand-fairness`: alignment, focused research, accepted design `design/20260923T094321Z_fair-selection-under-cap.md`, and the original `content-key-stall` review finding `SF-001`.
