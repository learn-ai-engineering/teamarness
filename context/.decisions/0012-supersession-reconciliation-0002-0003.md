# 0012 — Reconcile superseded clauses in ADR 0002 and ADR 0003

**Status:** accepted (2026-09-17)
**Supersedes:** ADR 0002 (clauses listed below), ADR 0003 (clauses listed below)

## Context

ADR 0001 rule 2 says an accepted decision is never edited; a contradicted clause is superseded by a later decision that cites it.

ADRs 0006, 0007, and 0008 changed how Package Backends run, which language their contract uses, who owns synchronization transport, and who holds content keys. ADR 0006 and ADR 0007 name some of the ADR 0003 sections they replace. They do not cite the ADR 0002 clauses on TypeScript contracts and the Tauri bridge. ADR 0008 moved raw content-key custody to the Core Runtime without any supersession text. A reader of ADR 0002 or ADR 0003 alone would therefore find statements that are no longer true, with nothing pointing to their replacement.

## Decision

The following clauses are superseded. Line references are to the ADR files as accepted.

| Clause | Replacement | Authority |
|---|---|---|
| ADR 0002, line 15: "The package SDK and semantic capability contracts are TypeScript" | Package UIs are TypeScript and use `@teamarness/package-ui-sdk`. Package Backends implement the language-neutral Package Process Protocol; Teamarness ships a Rust Backend SDK. Semantic Capability contracts are language-neutral JSON Schema under `contracts/`. | ADR 0007 |
| ADR 0002, line 17: "The runtime-to-package bridge uses Tauri's invoke/event system", and line 51: "The boundary is the Tauri invoke/event bridge" | The Package Backend boundary is the Package Process Protocol. The Tauri invoke/event bridge remains the boundary between the Shell and Package UIs. How Package UIs reach that bridge is under review in the architecture-review remediation plan phase 5; this decision does not settle it. | ADR 0006, ADR 0007 |
| ADR 0002, line 38: "Package authors write TypeScript" | Package UI authors write TypeScript. Package Backend authors may use any language that implements the Package Process Protocol; Rust has supported tooling. | ADR 0007 |
| ADR 0003, line 31, under "Core runtime does NOT own": "Package cryptography, keys, or transport" | The Core Runtime owns reusable synchronization transport and raw content-key custody, and performs generic record protection and grant mechanics. Packages own their domain records, schemas, projections, and policy. | ADR 0007, ADR 0008 |
| ADR 0003, line 40, under "Package owns": "Package cryptographic keys and key rotation" | Content-key custody moves to the Core Runtime. Packages never receive raw keys. **Whether and when content keys rotate is not decided here.** Version 1 has no rotation (ADR 0008); the rotation wording is owned by the `TEAM.SYS.MEM` requirements. | ADR 0008 |
| ADR 0003, line 73: "Conversations package uses Iroh for peer-to-peer message delivery … The package maintains its own allow-list derived from canonical membership." | The Conversations Package replicates its records through the `teamarness.synchronization.immutable-record-log` Synchronization Capability. The Core Runtime gates append and synchronization on Canonical Membership. | ADR 0007, ADR 0008 |

### Explicitly not superseded

- ADR 0003, line 95: "The Tauri webview boundary provides process-level isolation for package UIs." This clause is under review in the architecture-review remediation plan phase 5, which will supersede or reaffirm it.
- ADR 0002, line 14: the system tray, native menus, and auto-updater. They remain intended but are unscheduled.
- Everything else in ADR 0002 and ADR 0003, including Rust and Tauri for the shell and runtime, Iroh for peer-to-peer connectivity, product-domain ownership by Packages, and the narrow Core Runtime authority.

## Rejected alternatives

### Edit ADR 0002 and ADR 0003 in place

Rejected. It violates ADR 0001 rule 2 and would erase the record of what was originally decided.

### Wait for plan phase 5 and reconcile everything at once

Rejected. The clauses above are already settled by accepted ADRs 0006, 0007, and 0008 and do not depend on the Package UI isolation work. Waiting would leave known contradictions in the decision log for several phases.

## Consequences

- The decision index (`context/.decisions/README.md`) marks ADR 0002 and ADR 0003 as superseded in part by this decision.
- Readers of ADR 0002 and ADR 0003 resolve the listed clauses through this decision and the index.
- The Shell and Package requirements already describe the replacement ownership; no requirement changes are needed for these clauses.
- The Package UI isolation clause of ADR 0003 stays open until plan phase 5 records its outcome.
- Content-key rotation remains an open requirement question, not a decision made here.

## Evidence

- `contracts/package-capabilities/v1/`: language-neutral JSON Schema Semantic Capability contracts, including `immutable-record-log.schema.json`.
- `contracts/package-process/v1/`: the language-neutral Package Process Protocol.
- `crates/teamarness-package-sdk/`: the Rust Backend SDK.
- `packages/teamarness-package-ui-sdk/`: the TypeScript Package UI SDK, `@teamarness/package-ui-sdk`.
- `crates/teamarness-core/src/synchronization/`: Core-owned synchronization transport, record protection, and content-key custody.
- `packages/conversations-package/backend/Cargo.toml`: the Conversations Backend depends on no Teamarness crate other than the Backend SDK and the Package Process Protocol; it has no Iroh dependency.
