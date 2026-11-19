# 0044 — Person-authorized dynamic Package Ports

**Status:** accepted (2026-11-15)
**Supersedes:** ADR 0035 (the decision to offer package-ports 2.0 only)
**Refines:** ADR 0018 (major-version coexistence), ADR 0026 (Port metadata and routing), ADR 0030 (Host-derived actors), ADR 0036 (tier-aware discovery)

## Context

A Personal Package can expose selected Workspace Ports to an external coding agent. The external agent runs independently of any Agent Package installed in Teamarness. Calls through this gateway act on behalf of the Installation's current Workspace Person, not a Workspace Agent.

Newly installed Providers must be discoverable without rebuilding the gateway. Manifest-declared Port consumption permits Person-authored calls but requires the Consumer to name its dependencies in advance. Dynamic Agent access discovers unknown guide-bearing contracts but requires a current Agent Binding. Neither interface alone supplies Person-authorized automatic discovery.

Port routing, schema validation, bounded metadata, lifecycle, and actor derivation belong to the Host. External bearer tokens, listener configuration, and tool restrictions belong to the gateway Package. Native Package trust remains a supported-contract boundary, not containment of hostile code.

## Decision

### 1. Add package-ports 3.0 and retain 2.0

The Host supports `teamarness.package-ports` 3.0 alongside unchanged 2.0. Adding a Person access mode extends a closed vocabulary and therefore requires a new major under ADR 0018. The earlier pre-release exception that removed major 1 does not authorize removing major 2.

Negotiation selects the requested family and major, then the highest common minor. A grant's negotiated major determines its wire validation. A 2.0 grant does not accept 3.0 fields or acquire Person discovery authority. Existing 2.0 Consumers need no rebuild. Keep 2.0 for at least the ADR 0018 compatibility window; its removal is separate work. Provide the corresponding author and catalog compatibility diagnostics.

Both versions use the same internal broker and metadata machinery after version-specific validation. Do not duplicate dispatch implementations. Provider Port Contract versions, Package Process 1.0, and the UI Host version do not change because of this extension.

### 2. Person access is a precondition, not an actor claim

Major 3 provides `declared`, `agent`, and `person` access views. Selection requests explicitly identify the Agent or Person namespace. Existing declared and Agent behavior is preserved.

Person access requires an unbound calling Package Instance and a current local Workspace Person. The Host derives that Person through Core. A bound caller cannot request Person access to override its binding. Agent access still requires a current Agent Binding and never falls back to Person authority.

The capability grant fixes the caller Package, Workspace, Installation, and process generation. Requests cannot choose an actor, another Workspace, or a Provider. The Core signing rule and key custody do not change. No Person Binding or new Principal kind is introduced.

### 3. Discovery and selections remain generic

Person discovery lists admitted, routable contracts visible to the caller's admission tier, including contracts without a conforming runtime guide and retained unavailable selections. Guide-less contracts carry an eligibility explanation but cannot be enabled through Person dynamic access. Reuse the existing `runtime.guide` convention and compiler classification.

Major-3 summaries identify the offered contract version, Provider identity and display metadata, guide eligibility, current availability, saved selected major, and complete metadata digest. This permits a UI to group several contracts from one Package without treating contract identity as Package Identity. Do not expose filesystem paths, loader URLs, or graph-rejected authoring candidates. Major-2 metadata remains unchanged.

Preserve the existing bounded summaries, chunked metadata, reachable schema definitions, query/stream/event distinctions, and tier policy. Team Consumers see Team Providers only; Personal Consumers can see both tiers. Discovery does not start Providers or enable access.

Person selections are durable, default-empty, and separate from existing Agent selections. Key them by Installation, Workspace, caller Package Identity, and contract, with the selected major. Do not import Agent selections as Person authorization. A missing or incompatible Provider remains unavailable rather than silently upgrading the choice.

Major-3 discovery also supplies a scope-bound opaque route revision. A Consumer can require that revision when describing, enabling, or invoking a contract. The Host checks it against the current Port snapshot before dispatch. A mismatch returns `port_catalog_changed`. The revision is an optimistic precondition, not a caller-selected route or a durable grant.

### 4. Preserve authorization and lifecycle ownership

Person access uses the existing broker's commands, ordinary queries, live queries, and events. Authorization, durable selections, and binding transitions share the existing operation gate and process-generation leases. Actual work retains ownership after observer cancellation. Storage uncertainty never becomes a successful grant.

Person subscriptions record their access basis and actor. Disable, a transition to an Agent binding, or loss of current local membership ends affected Person subscriptions. Provider exit or replacement and Consumer generation retirement retain their existing termination semantics. Existing Agent binding-loss behavior is unchanged.

Already-authorized operations retain their actor and truthful uncertainty. No command is replayed. A read-only external product must enforce its own restriction; the generic Host mode is not a read-only gateway interface.

### 5. Keep external policy in the Package

The External Gateway is a standalone Personal Package, with one loopback endpoint per enabled Workspace. It consumes Person dynamic access and does not install, bind, or configure an internal Agent.

The Package owns bearer-token authentication, explicit Package and tool selection, HTTP limits, onboarding, and listener lifecycle. Host contract enablement is necessary but not sufficient for an external call. The gateway independently refuses disabled tools, commands, streams, and excluded contracts. Its initial external surface is read-only; Conversations is excluded. Tokens share one configured tool scope and are individually regenerable and revocable.

The gateway forwards Port calls rather than signing records itself. A future write would be attributed to the local Person under existing rules. A token label or gateway log is not signed evidence that a record came through the gateway; additional provenance would require a separate decision.

## Rejected alternatives

- **Manifest-curated Provider lists:** new Packages would require gateway manifest changes and re-admission instead of appearing automatically.
- **A dedicated Workspace Agent:** makes a Person-operated external gateway depend on an identity the user does not want to create or bind.
- **Relax major-2 Agent checks:** silently changes existing authority assumptions and permits unintended Person fallback.
- **Add a Person enum value in minor 2.1:** violates ADR 0018's closed-vocabulary rule.
- **A gateway-specific capability family:** duplicates the existing Port shape and puts one product's identity into generic Host behavior.
- **Replace major 2 immediately:** forces unrelated Consumer migrations without a supported compatibility window.
- **Host-owned HTTP and token configuration:** couples external-agent product behavior to the Host when a native Package can own it.

## Consequences

The capability registry must support multiple majors per family. The Host, wire schemas, SDK, authoring diagnostics, and tests must distinguish 2.0 from 3.0 explicitly while sharing routing implementation. Person selections need their own durable namespace. No Provider adapter or Core identity migration is required.

A gateway requiring 3.0 fails clearly on an older Host. Existing Agent Packages remain on 2.0 unless separately migrated. Gateway SDK vendoring must record a clean Host revision that implements the new interface.

Authorization tests must cover current Person membership, bound-caller refusal, separate selection namespaces, default denial, cross-Workspace isolation, route-revision races, storage cancellation, and stream revocation. Real-Host tests must run 2.0 and 3.0 Consumers together and discover an unknown live-installed Personal Provider without rebuilding the Consumer.

This decision is accepted architecture, not evidence that 3.0 is implemented or verified. External writes, MCP, LAN access, and additional signed provenance remain outside the initial gateway delivery.

## Evidence

- `crates/teamarness-package-host/src/capability.rs`: current family registry, grants, negotiation, metadata/selection dispatch, and binding checks. The inspected registry stores one descriptor per family and requires an extension for simultaneous majors.
- `crates/teamarness-package-host/src/{port_access,port_metadata,broker}.rs`: actor derivation, dynamic access, complete metadata, tier-aware snapshots, validation, operation ownership, and revocation. Current dynamic access is Agent-only.
- `crates/teamarness-package-host/src/host_store.rs` and `migrations/0003_agent_port_selections.sql`: existing Agent selection storage, which this decision does not reinterpret.
- `crates/teamarness-core/src/runtime/workspaces.rs`: current Person lookup through canonical membership, distinct from a token or payload claim.
- `crates/teamarness-package-sdk/src/ports.rs` and `contracts/package-capabilities/v1/package-ports-2.0.schema.json`: current closed `declared`/`agent` vocabulary and bounded metadata client.
- `tests/system/tests/agent_port_discovery.rs` and `crates/teamarness-package-host/tests/package_ports_{access,calls,events,metadata}.rs`: existing qualification seams to extend. Their presence is not a passing result for the new mode.
- Packages requirements `TEAM.SYS.PKG-R02`, `R05`, `R11`, `R13`, `R14`, and `R19`: lifecycle, storage, native networking, scoped Ports, and settings UI precedents.

Evidence is targeted source inspection at `3a5f6545e1e8ca9125b53b2aa38ee5fa77ddc688`. No implementation or new test result supports the proposed mode yet. Scope assessment suggests an extension of existing mechanisms, not a replacement architecture; effort and end-to-end behavior remain to be qualified.

### Provenance (non-versioned)

Workstream `external-gateway` in the resolver-selected MyFlow store: accepted alignment `scope/20261114T234340Z_alignment.md`, research `research/20261115T000554Z_person-discovery-impact.md`, and design `design/20261115T001127Z_external-gateway.md`. Don accepted the design and requested this ADR on 2026-11-15.
