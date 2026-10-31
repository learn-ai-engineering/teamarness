# DELTA-016 — The Agent harness uses a bundled catalog and its own enablement

**Status:** resolved (2026-10-30) by workstream `agent-port-discovery`, Host `6073dd0` and paired standalone Agent `7cf7358`. Don approved local Host-first integration; no push or release is authorized. The original divergence below is retained as history.

## Resolution

The Agent uses package-ports 2.0 runtime summaries and complete metadata, with Host-owned durable selections. Its built-in Conversations participation and generic model tools use the same enforced access. Canonical SDK vendor provenance, final Agent build and all nine real Agent/Host integration cases pass independent review and audit. A source-built major-3 Provider absent from the Agent's catalog and consumed declarations supplies a guide, exact schemas and successful query/command calls without an Agent rebuild.

Don confirms the rebuilt Agent, Conversations guide retrieval, enable/disable, rebinding/unbinding and retained choices after app restart. He accepts automated evidence for Provider stop/recover because the desktop UI has no such control. This is not qualification of per-Agent restart with unresolved commands.

Documents still has no guide. Its future chat/revise check and Comment Ports remain under **Documents Agent tools and Comments** in the [roadmap](../roadmap.md), not an unresolved runtime-catalog divergence. Exhibits file tooling also remains separate.

Evidence: workstream `agent-port-discovery`, `verify/20261027T012416Z_{review,validation}.md` and `verify/20261030T234629Z_validation.md` in the resolver-selected MyFlow store.

## Original divergence

[ADR 0035](../.decisions/0035-agent-controlled-port-discovery.md) puts runtime discovery and enablement in the Host:

- The harness learns a contract's Ports, schemas and `runtime.guide` through `teamarness.package-ports` 2.0 metadata.
- The Host stores the Steward's selections for each harness and enforces them.
- A bound Agent needs no `consumedPorts` declaration to call a guide-bearing contract.

The standalone Agent Package's first tools slice (workstream `agent-harness`, repository `teamarness-local-packages/agent`, branch `proper_harness`) does none of this. Instead it does the following:

- **Bundled catalog.** It compiles in a snapshot of the Conversations, Documents and Exhibits `ports.json` files from Teamarness `d25a42d`, checked by digest. A new agent-facing Package, or a new guide minor, appears only after a catalog refresh and an Agent rebuild.
- **Manifest declarations.** It declares Documents 1.1 and Exhibits 1.0 as optional `consumedPorts`. It asks the Host only whether those declared contracts are available, so it lists a contract that isn't installed as "Unavailable".
- **Local enablement.** It stores the Steward's choices in its own `packages.json` and enforces them only inside the harness. The Host doesn't see or enforce them.
- **Old capability major.** It uses `teamarness.package-ports` 1.x through the vendored SDK. Under ADR 0035 the 1.x declaration fails negotiation once 2.0 ships, so the Agent must be rebuilt with its callers.

Only Exhibits has a `runtime.guide` at `d25a42d`, so Exhibits is the only Package the Agent offers today.

## Original impact

- A Steward can't use a newly installed agent-capable Package without a new Agent build. This is the opposite of the developer's goal: an agent-capable Package should appear, be switched on, and work at once from the metadata the Host provides.
- Enablement is a harness-side policy, not a Host guarantee.
- After ADR 0035 integrates, the current Agent build no longer negotiates Ports until it is migrated.

## Original follow-up

The original proposed Agent slice was:

- Replace `BundledCatalog` with Host discovery behind the Agent's existing `Catalog` seam, using package-ports 2.0 summary and detail reads.
- Move enablement to the Host-owned selections. Retire `packages.json`, the bundled `backend/catalog/`, `refresh-catalog.mjs` and the optional `consumedPorts` entries.
- Show only the contracts that discovery returns, plus retained unavailable selections, as ADR 0035 defines.
- Run the Documents chat check that `agent-harness` deferred, once Documents publishes a guide, and list Conversations once its guide ships.

The closure criterion was discovering and enabling a guide-bearing Package that the Agent was not built with, without a rebuild. The real unknown-Provider integration case now satisfies it.

## Tracking

- ADR 0035, ADR 0026, ADR 0030, [TEAM-DQ10](../open-questions.md), and `context/roadmap.md` (Agent Package tools and guides).
- Workstream `agent-harness`: plan `plan/20261025T064413Z_agent-harness-plan.md` (Explicit Exclusions), and Verify `verify/20261025T082709Z_agent-harness-validation.md`.
