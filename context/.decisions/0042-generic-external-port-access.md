# 0042 — Generic external access to Person-selected Package Ports

**Status:** accepted (2026-11-16)
**Supersedes:** ADR 0041 (Decision §5, initial read-only, stream, and Conversations restrictions; Consequences, external writes outside the initial delivery)
**Refines:** ADR 0016 (external Provider-error projection), ADR 0026 (external live-delivery adapter)

## Context

An external coding agent needs to create and maintain Workspace content, not only read it. A Person chooses the exact operations that the External Gateway exposes. New guided Providers must work without Package-specific gateway changes, including Providers that offer streaming queries or events.

The Host already supplies Person-authorized commands, ordinary queries, streaming queries, and events through package-ports 3.0. It owns actor derivation, contract enablement, schema validation, route preconditions, Provider fan-out, and subscription leases. The standalone gateway owns loopback HTTP, bearer tokens, exact-tool selection, and external connection lifetime.

All gateway tokens share one configured tool scope. Calls act as the local Workspace Person, not a Workspace Agent. A token label is not signed domain provenance. The gateway's HTTP interface is unreleased, and its existing read-only manifest uses a closed schema.

## Decision

### 1. Expose every supported shape through explicit selection

The gateway supports ordinary queries, commands, streaming queries, and events from any admitted, visible Provider with a conforming runtime guide. No Package or contract name is permanently excluded. In particular, Conversations is selectable, not enabled by default.

Keep the runtime-guide prerequisite, separate Person selections, Host-derived authority, and package-ports 3.0 alongside unchanged 2.0. No new capability version, Agent Binding, Provider adapter, or signing authority is introduced.

Select exact tools against their current Provider, major, and canonical schema fingerprint. Added tools stay off. Automatically include the required guide when any valid tool is selected. Guides are untrusted data, never authority. Existing persisted reads and token verifiers retain their representation. Upgrade alone enables no commands or subscriptions.

Every active token gains each subsequently selected tool. Settings distinguish reads, writes, streaming queries, and events, and state that writes act as the local Person. Explicit tool enablement is consent to this shared-scope expansion. Package enablement alone does not select tools.

### 2. Extend the unreleased HTTP v1 contract in place

Preserve existing ordinary-call request and result envelopes. `POST /v1/call` also accepts selected commands, with dispatch kind derived from metadata rather than an external claim. Streaming shapes use a separate `POST /v1/subscribe` route.

Revise the closed manifest and related schemas together with documentation and tests. The manifest no longer claims the product is read-only. Capability support does not imply that a tool is enabled. Existing clients that require the exact old closed schemas must update. Do not maintain a second read-only HTTP version for this unreleased product.

This decision concerns the Package-owned external HTTP contract only. It does not relax ADR 0018's Semantic Capability versioning rules.

### 3. Own each subscription through one NDJSON connection

A bearer-authenticated POST opens one selected event or streaming query. Streaming queries carry the Provider request. Event requests carry gateway `request:null`, which is not forwarded to the event method.

After successful Host subscription establishment, the connection carries compact newline-delimited JSON ready, value, heartbeat, and terminal frames. Values preserve the original schema-valid JSON. The gateway neither interprets all events as invalidations nor generically coalesces them. The ready frame permits clients to subscribe before initial reads only after the both-leg confirmation in §4.

Delivery is live-only and non-durable. Disconnect ends ownership. Reconnect is explicit and requires fresh authorization and Provider-specific reconciliation. There are no replay IDs, detached subscription handles, durable acknowledgements, automatic reconnects, or automatic model wakeups.

Use eight stream slots, separate from the existing twelve ordinary SDK-call slots and within the existing 32-connection ceiling. Bound output and use existing SDK credit rather than creating another fan-out broker. Use a stream-specific five-second pending-output write limit and a 15-second heartbeat, not a five-second lifetime for quiet streams. Stream slots cover pending openings too; an abandoned opening that later succeeds must close its subscription.

The gateway policy owner registers token generation and exact-tool authority for each stream. Token regeneration/revocation, tool or Package disablement, unavailable authority, and lifecycle stop terminate affected streams even when idle. Successful policy changes prevent later local value admission. Already-admitted bounded frames and socket bytes may drain. No socket or cleanup wait holds policy ownership.

### 4. Confirm listening across both process legs

Process 1.1 adds optional `stream_open.requireReady:true` and correlated
`stream_ready` containing only kind and the existing stream ID. Both forms are
version-gated. Existing process 1.0 peers and unconfirmed APIs keep their behavior.
Admission and launch select the highest common minor in Host range 1.0–1.1.

A Provider handler registers its listener and initializes change-filter state
before returning `EventSubscription`; the SDK acknowledges only after that
return, before forwarding values. The Host confirms Provider setup, then installs
the Consumer route, lease, and event subscriber before acknowledging the Consumer.
Two confirmed event subscribers reuse one confirmed upstream. Legacy 1.0 Providers
and unconfirmed upstreams cannot satisfy this request and refuse clearly with
`capability_version_unsupported`, without losing ordinary supported operations.

The shared classifier enforces version, direction, requested readiness, ordering,
and existing bounded tombstones. Ready consumes no sequence or credit and carries
no Person authority. A close before ready is an opening refusal. The Host uses
one 20-second setup budget including activation, with remaining-time nested waits
and cleanup for cancellation, timeout, process loss, and late completion. Readiness
means registered and listening at setup, not durable history or permanent service.

Keep enqueue-only SDK methods and add explicit confirmed methods. Rebuild source
Providers with compatible SDK bytes and process ranges. Refresh the gateway once
from a generated canonical snapshot of committed upstream source, preserving
four-tree checks and truthful revision/digest provenance. This bounded correction
resolves the original design's missing establishment seam; it does not authorize
a new broker, capability major, Port Contract shape, or general SDK updater.

### 5. Preserve mutation and error semantics

Pass Provider requests, version fields, results, and errors without a guessed cross-Provider version abstraction. Keep the external error envelope and preserve complete Provider-owned details within the Host's 4 KiB/depth-8 bounds. Drop over-budget details as a whole, not by Package-specific field allowlists. Do not forward unrelated Host diagnostics.

Provider details remain untrusted domain data, as successful results do. The gateway does not add credentials, request headers, or private diagnostics. It cannot generically guarantee that a trusted native Provider never returns sensitive content.

Preserve authoritative SDK, Host, and Provider certainty verdicts. Gateway-owned failures after a command may have dispatched report an uncertain outcome and prohibit retry. Losing a response can follow a committed write. No invocation is automatically replayed after timeout, stale route, disconnect, or restart.

The final local gateway admission remains the call revocation seam. Already-admitted commands may finish after a later disable/revoke and may have lasting effects. Do not add enqueue barriers, cancellation, rollback, exactly-once execution, or settlement tracking.

## Rejected alternatives

- **Keep external access read-only or exclude Conversations:** prevents the accepted participation workflow and introduces arbitrary domain restrictions.
- **Provider-specific gateway tools or version/error adapters:** undermine dynamic discovery and lose Provider-owned semantics.
- **SSE:** viable, but native EventSource cannot issue this authenticated POST. NDJSON uses one JSON framing model for the known coding clients.
- **WebSockets or detached long-poll subscriptions:** add session, multiplexing, expiry, or abandoned-handle state without a demonstrated need.
- **HTTP v2 alongside exact read-only v1:** adds compatibility routing and qualification for an unreleased external interface. The developer accepts updating strict clients instead.
- **Per-token roles or automatic token rotation:** conflict with chosen shared scope or add credential churn without replacing explicit exact-tool consent.
- **Durable event replay or write idempotency ledger:** stronger guarantees than the underlying live stream and Provider contracts require.
- **A new Host subscription broker:** duplicates existing validated, credit-controlled fan-out and authority leases.

## Consequences

The standalone Package changes catalog dispatch, policy, HTTP bodies, UI contracts and labels, and external schemas/docs. It retains configVersion 1, bearer-token format, artifact shape, and declared capabilities. The bounded Host/SDK/process-1.1 handshake and one-time canonical gateway vendor refresh are required. No broader Host redesign is authorized.

Gateway stream tests must cover sparse events, bounded capacity, disconnect, slow clients, idle permission loss, and cleanup while ordinary calls remain usable. Command tests must distinguish certain pre-dispatch refusal, authoritative Provider refusal, and uncertain post-admission gateway failure. Real-Host qualification must use actual Backlog, Documents, and Exhibits commands, a real Provider event, and an unknown guided Provider including a streaming query.

Provider limitations remain visible: existing Epics/Collections are prerequisites, Documents create/revise are separate commands, and Exhibit Drafts remain Installation-local. No new Provider CRUD is supplied by the gateway. CLI distribution, credential provisioning, full UI redesign, model-wake integration, MCP, and LAN access remain separate work.

This is architecture intent, not passing implementation or manual evidence. No merge, push, publication, normal-profile installation, or waiver of the initial read-only milestone's manual gates follows from this decision.

## Evidence

- `crates/teamarness-package-sdk/src/ports_v3.rs` exposes `command`, `query`, `live_query`, and `subscribe_event` with Person access and route preconditions. `capability.rs` supplies `recv_with_credit`, explicit close, and drop cleanup.
- `crates/teamarness-package-host/src/{port_access,capability,broker,event_subscriptions}.rs` own Person authorization, dispatch, validation, and bounded subscription delivery. `errors.rs` fixes Provider-error details at 4,096 bytes and depth 8.
- `context/02-system/05-packages/requirements.md`, especially R13–R14, requires truthful command uncertainty and bounded live-only event delivery. ADRs 0026 and 0041 place generic authority in the Host and external policy in the Package.
- `packages/{backlog-package,documents-package,exhibits-package,conversations-package}/ports.json` describe the real Provider commands, queries, and events. Their runtime guides own workflow and reconciliation semantics.
- Targeted inspection at Host `d0cc7c4cceb37bf64aa09342fb6c9abaf26644dc` and standalone gateway `eac9d8c0c34e2c6a1729fe54a460485bb6c8b592` found read-only/Conversations filters in the gateway, not a missing Host dispatch shape. Gateway config already records exact tool names and kind-sensitive fingerprints. Its HTTP code currently buffers complete JSON responses and uses a whole-response write deadline, so subscriptions require a separate body/write-lifetime path.
- **Weaknesses:** gateway source belongs to a separate repository and is not reproduced here. No new tests, builds, runtime cost measurement, outside-agent workflow, or notification-harness integration ran during this design. Eight stream slots and heartbeat/write intervals are bounded design choices, not measured throughput guarantees. Existing upstream unhealthy-Provider stream debt in DELTA-018 is not resolved here.

### Provenance (non-versioned)

Workstream `external-gateway-writes` in the resolver-selected MyFlow store: accepted Scope alignment and guide addendum, targeted generic-Port research, and `design/20261116T075440Z_generic-port-access.md`. Don chose NDJSON, in-place HTTP v1 evolution, and a new ADR on 2026-11-16. He then accepted the complete design and ADR as the input to Plan. On 2026-11-16 he accepted the corrected both-leg handshake design and replacement plan with “Accept and implement.”
