# 0026 — Host-brokered Package ports

**Status:** accepted (2026-10-21)
**Refines:** ADR 0007 (the Package Host boundary), ADR 0018 (Package seam versioning), ADR 0019 (bounded protocol adapters)

## Context

Independent Packages need to offer commands, queries and live events to other Packages. Existing Semantic Capabilities let a Package consume Host-owned behavior, but do not route to another Package. Direct linking would couple Package identities and bypass the Host's Workspace and process-generation authority. A new process frame would also change the language-neutral Package Process Protocol even though its existing request and stream frames are bidirectional.

Existing Packages activate eagerly to perform background work. A universal lazy startup policy would break that behavior. Per-process request and stream ceilings are shared with lifecycle and UI traffic, so simply forwarding requests or making every subscriber wait for the slowest one is not safe.

## Decision

A Package declares versioned **Port Contracts** independently of Package Identity. A Provider's command, query and event schemas live in a bounded, digest-verified contract file referenced by its Package Manifest. A Consumer declares exact ports and compatible versions in its manifest. The Package Host resolves at most one Provider per contract per Installation at Package Admission and issues a generation-bound `teamarness.package-ports` grant restricted to the Consumer's declared ports. Required missing or incompatible ports refuse the Consumer; optional absent ports remain discoverably unavailable. An existing graph-feasible Provider remains the incumbent when a second Provider is registered; recency and discovery order do not replace it. The Host resolves competing claims and required dependencies as a bounded graph. If an unusually dense contested component exceeds the search ceiling, the Host reports `port_graph_search_limit` for that component rather than persisting a provisional replacement; unrelated Packages continue. Registration changes are reconciled on the next Host open, not by live grant rebinding.

The Host brokers every call and explicit subscription to a Provider Instance in the caller's Workspace over existing Package Process Protocol 1.0 requests and streams. It validates payloads both ways, supplies actor context from the caller's Instance binding, and does not trust a payload's acting-Principal claim. The Host does not add Package-specific routes to the Shell. Commands may mutate; queries promise read-only behavior by Provider contract rather than Host inspection. Live queries are consumer-initiated streams; events are Provider-originated, live-only, at-most-once streams. One upstream event stream per active Provider event port fans out through bounded, independent subscriber queues. Overflow ends only that subscriber; it does not delay the others or imply replay.

Startup remains eager by default for existing background Packages. A Package may opt into on-demand startup; its first call, explicit subscription, UI mount or explicit activation starts it. Event declaration does not subscribe or wake inactive Consumers. A dispatched command whose Provider exits, stalls or produces an invalid response may have taken effect: the broker reports an outcome-unknown verdict and never automatically retries it.

Host-derived actor context does not delegate a Principal's signing key to the Provider. Under ADR 0023 a Workspace Agent key is bound to one Package Instance; this decision does not enable a separate Conversations Provider to sign records as that Agent. The Conversations 1.0 Port Contract offers a Person-only `message.post` command, a read-only `message.page` query, and a live coalescible `messages.changed` invalidation. Conversations checks the Host-derived Installation Person against its own Installation before a post. Core signs the Message; the actor context never selects a signer. A missed invalidation requires a fresh page query. Delegated Agent-signed writes require a later authority decision.

The Host and SDK MAY expose opt-in, bounded, payload-free local Port observations. The Host relates process-scoped Consumer `p:` IDs to Provider `h:` IDs and records pre-dispatch rejection without inventing a Provider ID. A shared event upstream relates one `h:` ID to separate Consumer process scopes and `p:` stream IDs. A slow receiver reports lag and cannot delay a Port. SDK Provider observations use the existing `h:` ID. These observations cannot prove command success, survive a process crash, or serve as a cross-installation trace. The Conversations developer mode can render safe fields into the Host's existing private rotated stderr files. No Shell diagnostic contract or telemetry store follows from this decision.

## Rejected alternatives

- **Direct Package links or Package Identity as the consumed contract.** They bypass broker authority and make implementation substitution require changing Consumers.
- **Inline manifest schemas or a new process frame kind.** The former bloats a bounded manifest; the latter adds protocol churn despite existing bidirectional frames.
- **Universal lazy activation or automatic event-driven Consumer wakeup.** These break current background synchronization or add a new durable wake obligation to live-only events.
- **Live registration rescans.** They would replace admission graphs and grants under running generations; v1 preserves the Host-open reconciliation point.
- **One shared backpressured subscriber queue.** One slow Consumer would stall all others. Independent bounded queues localize the failure.

## Consequences

The Package Host becomes responsible for a compiled installation wiring graph, exact scoped grants, bounded routing, schema validation and per-subscriber overflow. Package authors gain a manifest-derived full catalog and a separate grant-filtered runtime view. The Rust SDK gains generic adapters, but the documented frame route remains language-neutral. Existing capability families, UI contracts and Package Process Protocol major remain unchanged. A newly registered Provider requires reopening the Host. A test-only SDK Consumer qualifies Conversations across two profiles; it does not ship as a Package. Agent-bound runtime and authority belong to a separate Agent Package workstream. An in-app diagnostics view needs separate Observability scope.

## Evidence

- Workstream `package-interface`: `scope/2026-10-21_18-30-37_package-interface-alignment.md`, `research/20261021T054236Z_host-routing-and-admission-bounds.md`, `design/20261021T060446Z_package-ports-broker.md` (in the resolver-selected workstream store).
- `crates/teamarness-package-host/src/{admission,capability,instance,protocol}.rs`, `host/lifecycle.rs`; `crates/teamarness-package-sdk/src/{dispatcher,lib}.rs`; `contracts/package-process/v1/README.md`.
- `packages/conversations-package/ports.json`, `backend/src/backend.rs`; `tests/system/tests/conversations_ports_proof.rs`; `crates/teamarness-package-host/src/port_observation.rs`; `crates/teamarness-package-sdk/src/port_observation.rs`. The source-built system Consumer verifies signed Person authorship, live invalidation and recovery. Host and SDK observations are optional, process-local diagnostics, not durable delivery evidence.
