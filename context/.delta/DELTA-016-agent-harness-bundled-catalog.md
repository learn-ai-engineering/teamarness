# DELTA-016 — The Agent harness uses a bundled catalog and its own enablement

**Status:** open (2026-10-26). Owner: the next standalone Agent Package slice, after workstream `agent-port-discovery` lands ADR 0035. Accepted by the developer as a v1 step for the `agent-harness` workstream.

## Divergence

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

## Impact

- A Steward can't use a newly installed agent-capable Package without a new Agent build. This is the opposite of the developer's goal: an agent-capable Package should appear, be switched on, and work at once from the metadata the Host provides.
- Enablement is a harness-side policy, not a Host guarantee.
- After ADR 0035 integrates, the current Agent build no longer negotiates Ports until it is migrated.

## Follow-up

The next Agent slice, after `agent-port-discovery` integrates:

- Replace `BundledCatalog` with Host discovery behind the Agent's existing `Catalog` seam, using package-ports 2.0 summary and detail reads.
- Move enablement to the Host-owned selections. Retire `packages.json`, the bundled `backend/catalog/`, `refresh-catalog.mjs` and the optional `consumedPorts` entries.
- Show only the contracts that discovery returns, plus retained unavailable selections, as ADR 0035 defines.
- Run the Documents chat check that `agent-harness` deferred, once Documents publishes a guide, and list Conversations once its guide ships.

Close this delta when the Agent discovers and enables a guide-bearing Package that it was not built with, with no rebuild.

## Tracking

- ADR 0035, ADR 0026, ADR 0030, [TEAM-DQ10](../open-questions.md), and `context/roadmap.md` (Agent Package tools and guides).
- Workstream `agent-harness`: plan `plan/20261025T064413Z_agent-harness-plan.md` (Explicit Exclusions), and Verify `verify/20261025T082709Z_agent-harness-validation.md`.
