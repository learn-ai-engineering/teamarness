# 0035 — Agent-controlled runtime Port discovery and enablement

**Status:** accepted (2026-10-25)
**Refines:** ADR 0026 (Port metadata and routing), ADR 0030 (bound-Agent access)
**Exception:** ADR 0018 (previous-major transition window for this pre-release change only)

## Context

A generic Agent harness needs complete descriptions and schemas before it can construct tools. A list of declared Port names is insufficient, and a contract admitted at 1 MiB cannot fit the 256 KiB runtime value budget. Agent participation must also follow the Steward's selections, including when the harness declares the Provider's Ports. A manifest must not bypass an unchecked contract.

The harness is a Package, not a Shell feature. Providers already receive Host-derived actors through one broker. Provider-specific adapters or Provider-owned Agent settings would couple otherwise independent Packages. Several harness Packages may bind the same Agent, so settings keyed only by Agent Principal would silently share authority.

## Decision

### One current family and complete metadata

Use `teamarness.package-ports` **2.0 only**, with explicit `access` and requested contract `major` on invocations and subscriptions. Package Process stays 1.0; Provider contracts version independently. This pre-release transition explicitly excepts ADR 0018's intended previous-major window: no install base, published SDK, or historical settings require a compatibility dispatcher. Rebuild actual callers together. Old capability declarations fail negotiation clearly; no grant or settings migration runs.

The versioned wire contract is [package-ports-2.0.schema.json](../../contracts/package-capabilities/v1/package-ports-2.0.schema.json). The `v1` directory identifies the document collection, not the family version. Summary pages contain at most 32 rows and 32 KiB encoded JSON. Detail reads carry at most 64 KiB decoded bytes in padded base64. The complete compact UTF-8 projection is at most the admitted 1 MiB. SHA-256 names the exact bytes, not a reserialized equivalent. Schemas are never truncated.

Compile guide classification and metadata from final admitted Providers and actual declared routes, not rejected authoring-catalog candidates. A guide is a normal `runtime.guide` query with a closed empty-object request and a closed response requiring a bounded `markdown` string. Resolve local references and warn authors about nonconforming guides. Declared projections contain exactly declared Ports; Agent projections contain all contract Ports. Both include all reachable definitions and preserve ordinary-query, live-query, and event distinctions.

Opaque cursors bind caller, Workspace, access view, admission revision, and last sorted contract identity. No per-client snapshot is retained. Host replacement invalidates cursors; discovery never starts a Provider. Current Instance availability is sampled separately from immutable admitted metadata.

### Agent UI controls; Host owns enforcement

The Steward chooses contracts in the Agent Package UI. The harness configures only its own selections through its generation-bound capability grant. Requests cannot select another Package, Workspace, Provider, or actor.

Selections default to empty. Persist selected contract majors in the existing Installation-local Host store under Installation, Workspace, and harness Package Identity. Do not key them by Provider or Agent Principal, replicate them, infer them from manifests, or add a general configuration framework. Retain them across restart, rebind, unbind, and harness or Provider reinstall under the same identities. A major mismatch is unavailable, not an automatic upgrade. An unavailable saved choice can still be disabled.

Agent discovery requires a current binding and includes admitted eligible guide-bearing contracts plus unavailable retained selections. It exposes no never-enabled no-guide contract. Declared metadata remains available to unbound non-Agent consumers. Dynamic Agent calls require no fabricated `consumedPorts` declaration. A bound Agent using a guide-bearing contract requires enablement through either view; a declared no-guide contract remains ordinary composition. Unbound Agent access never falls back to Person authority.

### Shared dispatch and current Host lifecycle

A focused `port_access` authorization seam owns visibility, settings, and resolution over one immutable admission snapshot. Both views use the existing broker's validators, permits, isolation, overload, event fan-out, actor derivation, and uncertainty classifier. Calls and live queries retain their derived caller actor; shared event upstreams retain the Installation Person actor.

A per-harness operation gate serializes durable settings changes, authorization, and binding transitions. Actual work owns the gate and generation lease, including blocking storage work after waiter cancellation. Publish settings only after durable success. If completion is uncertain, reread durable state before new authorization; do not claim success. Do not hold the gate throughout a Provider command.

Subscription ownership includes Workspace, harness, contract, access basis, binding identity or revision, and generation. Disable closes affected subscriptions with `port_not_enabled`; unbind or Agent removal uses `agent_not_bound`; a changed binding uses `agent_binding_changed`. Already accepted bounded events may drain, but none is enqueued after revocation. Unrelated subscribers keep their upstream and permits. Local binding transitions notify directly; accepted membership loss reconciles idle streams too.

Provider exit, suspend, dispose, or generation replacement ends affected streams with `provider_exited`. Recover permits explicit reconnect, not revival or replay. Commands authorized earlier keep their actor and truthful uncertainty. Never automatically retry an uncertain command.

Registration replacement remains next-Host-open behavior under ADR 0026 and Packages R09/R13. This decision does not depend on packagement or adopt its future tier policy or live registration manager. No Core signing policy changes.

## Rejected alternatives

- **Provider-side or two-sided settings:** duplicate Agent management without strengthening the trusted-native-process boundary.
- **Global Agent selections:** unintentionally share authority between harnesses.
- **Manifest bypass:** contradicts the Steward's unchecked selection.
- **New Agent capability family or legacy dispatcher:** duplicate routing and misstate this authority break.
- **One response containing every schema:** exceeds the value budget for admitted contracts.
- **Dynamic manifest rewriting or per-Agent event upstreams:** confuse admission with settings and duplicate existing fan-out.
- **Packagement integration as a prerequisite:** explicitly excluded; qualify current lifecycle and changed registrations on reopen.

## Consequences and qualification

This is a supported-contract policy, not a native-code sandbox. A trusted harness can call its own enable method. Agent UI, model endpoints, external SDK vendoring, and Provider-specific tools remain upstream-owned.

Phase 1 qualifies declared metadata, explicit current requests, migrated callers, and default-denied bound-Agent guide access. Agent discovery, durable settings, dynamic access, and revocation remain later milestones; the final wire schema does not imply those handlers exist. Conversations guidance follows separately. Documents tools and Comment Ports remain follow-ups.

Evidence: contract `port_contract_rules` tests; Host `package_ports_metadata`, `package_ports_calls`, and `package_ports_events`; SDK `ports_metadata`; the source-built Conversations declared consumer and system proof. These tests do not establish model-provider schema acceptance or the Steward's manual Agent UI journey.
