# 0024 — Qualified immutable-record replay and Package generation append fence

**Status:** accepted (2026-10-23)
**Refines:** ADR 0018 (additive Semantic Capability minor versions), ADR 0008/0009 (immutable-record replay and synchronization), ADR 0019 (Package lifecycle).

## Context

A Package resuming from a stored cursor, or seeing `current` availability, cannot conclude that an absent immutable record was never committed. A replacement Package generation can also see a replay boundary while an old generation's admitted append is still queued. Documents needs a reliable absence proof for an identified edit before it can retry without duplication, while existing 1.0 consumers must retain their wire contract.

## Decision

The immutable-record-log Semantic Capability offers minor 1.1. A negotiated 1.1 from-start subscription emits a credited, ordered `replay_end` event only after all locally committed scoped records through a fixed Core high-water mark have been ingested and acknowledged. Its cursor and journal token identify the scanned prefix; `gapFree` is true only when that prefix is complete and readable. Post-subscription appends cannot postpone the marker. A resumed cursor, `current` availability, a failed or reset stream, a gap, or unopened sealed history never certifies absence. The Host sends no `replay_end` to 1.0 streams; a Package requiring the proof declares minor 1.1 and refuses a 1.0-only Host.

Before replacement activation or recovery can capture its high-water mark, the Package Host closes the prior process generation's append admission, cancels queued work, and drains closures that entered Core. A timeout leaves the Instance failed/unresolved rather than launching a successor that could mistake an old in-flight append for absence. The Host-wide limiter bounds capacity but does not act as the generation fence. Other Package Instances remain independent.

## Rejected alternatives

- **Use health or a stored cursor as absence proof.** Neither establishes a from-start, gap-free prefix or accounts for unopened records.
- **Add a record-ID registry or plaintext Package journal.** Neither is necessary for generic replay qualification and both broaden the Core/Package boundary.
- **Change the 1.0 event vocabulary in place.** Negotiated 1.0 consumers must not receive a marker they did not request.
- **Let old queued appends finish after new replay.** This makes a true-looking absence proof stale immediately.

## Consequences

- A Package may conservatively keep an edit unresolved until both qualified replay and its own projection/open checks succeed; Documents does so for unavailable history.
- The marker is a new event kind in the existing capability family, not a new availability or open status. Minor 1.0 remains supported; additive-minor and closed-vocabulary rules in ADR 0018 remain unchanged.
- Incomplete drain prevents replacement for that Instance. This is a safety boundary, not a guarantee that unacknowledged UI drafts survive process death.

## Evidence

- `contracts/package-capabilities/v1/immutable-record-log-1.1.schema.json`, `immutable-record-log.test.mjs`, `immutable-record-replay.golden.json` — negotiated marker shape and 1.0 compatibility.
- `crates/teamarness-core/src/synchronization/record_subscription.rs` and `crates/teamarness-core/src/runtime/records.rs` — bounded Core high-water and gap-aware replay.
- `crates/teamarness-package-host/src/capability.rs`, `src/instance.rs`, `src/supervisor.rs` — credited marker and generation admission/drain.
- `crates/teamarness-package-sdk/src/capabilities/record_log.rs` — typed qualified-from-start callback and invalidation.
- `packages/documents-package/backend/src/backend.rs` — Package-owned projection/open checks before absence classification.
- **Weaknesses.** A qualified local prefix does not prove remote network delivery or successful decryption of all sealed Package history; the Package must remain unresolved where its own records cannot be opened.
