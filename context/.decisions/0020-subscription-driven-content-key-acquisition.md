# 0020 — Subscription-driven Content Key acquisition

**Status:** accepted (2026-09-22)
**Refines:** ADR 0008 (first-key bootstrap trigger), ADR 0009 and ADR 0014 (durable synchronization work)

## Context

A Package Instance opens `teamarness.synchronization.immutable-record-log` during activation. The Package can need history before it appends a record. A newly admitted Member does not yet have a `sync_scopes` row, so the synchronization worker cannot schedule that Member's scope. The creator can also have no scope to grant when no Package record exists. This state occurs in Conversations because `#general` exists without a record.

The Content Key remains Core-owned. A non-creator cannot create the first key, and a remote request cannot authorize the Workspace creator to create one. Key acquisition must preserve the creator origin and the recipient-bound Protection Grant rules from ADR 0013.

## Decision

Opening the immutable-record subscription declares local interest in the Package-and-Workspace synchronization scope.

When the local Installation is the Workspace creator, the declaration creates the genuine scope, its first Content Key, its durable wake, and the creator's signed Content Key Origin in one transaction. It creates no Package record.

When the local Installation is another current Member and has no Content Key for the scope, the declaration records one durable pending key demand. A failed append records the same demand as a fallback. Repeated declarations and blocked appends coalesce into the same row.

The synchronization worker enumerates pending key demands beside held scopes. Demand jobs use the existing authenticated record-sync session, current-Member peer set, concurrency limits, retry policy, and peer backoff. A completed session without a key records per-peer attempt progress but does not satisfy the demand. Idle reconciliation can try the demand again without a tight redial loop.

Only an authenticated Protection Grant import satisfies the demand. Grant import creates the genuine `sync_scopes` state and deletes the demand in one transaction. A non-creator never creates a scope key locally, and remote demand never causes creator bootstrap. The creator's own local subscription is the only automatic first-key trigger.

The immutable-record capability remains version 1.0. This decision changes the behavior behind `subscribe`; it adds no field, event, error code, or vocabulary value.

## Rejected alternatives

### Keep first-key creation on the creator's first append

Rejected. An empty synchronized Package scope has no holder, so a Member cannot acquire history access or append the first Package record.

### Store a permanent interest row after key acquisition

Rejected. `sync_scopes` already records the satisfied state. A permanent row would need Package disable and uninstall lifecycle rules that do not exist. Future Content Key rotation can reuse the same declaration-to-demand transition when a required generation is missing.

### Store a key-less placeholder in `sync_scopes`

Rejected. A `sync_scopes` row means that Core holds a real journal identity and Content Key. Weakening that invariant would spread optional-key handling across record storage, protection, validation, and scheduling.

### Keep demand only in memory or infer it from failed appends

Rejected. In-memory work is lost on restart, and append-driven acquisition makes a user mutation the normal synchronization trigger.

### Delete demand after any completed session

Rejected. A reachable peer may not hold the key. Only verified grant import proves that the demand is satisfied.

## Consequences

- Subscription has a deliberate durable side effect, but only for a current Member and only for a Package that opens the synchronization capability.
- The creator can hold and grant a Content Key before the first Package record exists.
- A dedicated pending-demand table and per-peer progress table keep key-less state out of `sync_scopes` and `sync_peer_progress`.
- The scheduler needs a distinct internal demand job so progress cannot be written through the held-scope foreign key.
- Local revocation removes pending demands for that Workspace. The existing membership gate prevents a revoked Package Instance from recreating them.
- The pending state contains Package Identity, Workspace ID, scope digest, and scheduler generations. It contains no key or Package record content.
- The ontology definition of Content Key Generation and the Transport requirements must name subscription-driven bootstrap and prompt missing-key acquisition.

## Evidence

- `crates/teamarness-core/src/runtime/records.rs` — `subscribe_records` opens a generic subscription without declaring synchronization work; `append_record` is the current Package mutation path.
- `crates/teamarness-core/src/synchronization/mod.rs` — `scope_digest`, `ensure_scope`, append bootstrap, Protection Grant import, and creator-origin verification.
- `crates/teamarness-core/src/synchronization/worker.rs` — held-scope job enumeration currently starts from `sync_scopes` and `durable_wakes`.
- `crates/teamarness-core/src/synchronization/scheduler.rs` — bounded jobs, per-peer backoff, and completion semantics.
- `crates/teamarness-core/src/runtime/replication.rs` — accepted membership transitions already wake held scopes and provide the cleanup seam for local revocation.
- `context/.decisions/0013-membership-replication-sync-authorization.md` — current-holder grants, creator origin, authenticated membership gates, and symmetric grant delivery.
- `context/.decisions/0014-synchronization-bounds-async-boundary.md` — scheduler bounds, idle reconciliation, and availability states.
- **Weaknesses:** the existing test suite does not isolate holder-online acquisition without explicit replication or forced sweeps. Implementation must add a condition-based regression with a deadline below the 60-second record-idle interval.

### Provenance (non-versioned)

- MyFlow workstream `content-key-stall`, including `scope/20260922T194235Z_content-key-stall.md`, `research/20260922T195106Z_scheduler-and-bootstrap-diagnosis.md`, and the Design artifact.
