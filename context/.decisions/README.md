# Decision log

Accepted architectural decisions for Teamarness. Each decision is numbered and immutable (ADR 0001, rule 2): an accepted decision is never edited, only superseded by a later decision that cites the clauses it replaces.

This index is the one place that lists every decision's status, its supersessions, and where its evidence is summarised. When an ADR's own header or evidence differs in style, this index is authoritative for status and supersession.

## Index

| Number | Title | Status (date) | Supersedes | Superseded in part by | Evidence summary | Notes |
|---|---|---|---|---|---|---|
| [0001](0001-vrs-intent-tree.md) | VRS intent tree as the context authority | accepted (2026-09-12) | – | – | In the ADR | – |
| [0002](0002-rust-tauri-iroh.md) | Rust, Tauri, and Iroh for the full stack | accepted (2026-09-12) | – | 0012 (lines 15, 17, 38, 51: TypeScript contracts, Tauri bridge, Package author language) | None recorded | Tray, native menus, and auto-updater are intended but unscheduled |
| [0003](0003-core-runtime-vertical-packages.md) | Core runtime with vertical packages architecture | accepted (2026-09-12) | – | 0004 (linear membership chain); 0006 (in-process Package Backends); 0007 (Package transport and host-proxied storage); 0012 (lines 31, 40: key custody and transport; line 73: Conversations transport) | None recorded | Line 95 (Package UI isolation) is under review in architecture-review remediation plan phase 5 and is not superseded |
| [0004](0004-p2panda-membership-dag.md) | p2panda causal DAG for canonical membership | accepted (2026-09-12) | 0003 (part) | – | In the ADR | "Phase 4" key-work labels clarified by 0011 |
| [0005](0005-iroh-workspace-onboarding.md) | Iroh workspace onboarding with recoverable atomic import | accepted (2026-09-13) | – | – | 0011 | Cited review records cannot be recovered |
| [0006](0006-rust-sidecar-package-backends.md) | Rust sidecar Package Backends | accepted (2026-09-13) | 0003 (part) | 0007 (Rust-only Backends) | 0011 | – |
| [0007](0007-language-neutral-package-host-capabilities.md) | Language-neutral Package host capabilities | accepted (2026-09-13) | 0003 (part), 0006 (part) | – | 0011 | – |
| [0008](0008-immutable-record-capability-contract.md) | Immutable record capability contract and protection boundary | accepted (2026-09-15) | – | – | 0011 | The immutable-record decision; "Phase 1" label clarified by 0011 |
| [0009](0009-core-synchronization-foundation.md) | Core synchronization foundation | accepted (2026-09-15) | – | – | In the ADR | Status-line phase label clarified by 0011 |
| [0010](0010-vendored-package-authoring.md) | Vendored Package authoring foundation | accepted (2026-09-15) | – | – | None recorded | Formerly `0008-vendored-package-authoring.md`; renumbered by 0011 |
| [0011](0011-decision-log-conventions.md) | Decision log numbering, format, phase labels, and evidence | accepted (2026-09-17) | – | – | In the ADR | – |
| [0012](0012-supersession-reconciliation-0002-0003.md) | Reconcile superseded clauses in ADR 0002 and ADR 0003 | accepted (2026-09-17) | 0002 (part), 0003 (part) | – | In the ADR | Does not decide content-key rotation or Package UI isolation |

### Former file names

| Former file | Current file |
|---|---|
| `0008-vendored-package-authoring.md` | `0010-vendored-package-authoring.md` |

## Numbering rule

The number is unique. Before adding an ADR, run:

```sh
ls context/.decisions | grep -E '^[0-9]{4}-' | cut -c1-4 | sort | uniq -d
```

The output must be empty. Add the new decision to the index in the same change.

## Phase labels

In an ADR, "Phase N" means a phase in `context/roadmap.md`. A step inside a workstream plan is a "workstream step", never a "Phase" (ADR 0011).

## Template

ADRs numbered `0011` and later use this template. Earlier ADRs keep their original format.

```markdown
# NNNN — Title

**Status:** accepted (YYYY-MM-DD)
**Supersedes:** ADR NNNN (clauses or sections replaced)   <- optional

## Context

Why a decision is needed now, and the facts that constrain it.

## Decision

What is decided, written as settled intent.

## Rejected alternatives

Each option considered and why it was not chosen.

## Consequences

What changes, what now must be true, and what is left open.

## Evidence

Self-sufficient versioned text: tracked repository paths, measured or
researched facts, and the known weaknesses of that evidence.

### Provenance (non-versioned)   <- optional

Local working artifacts (for example files under `.myflow`). Never
required to understand or check the decision.
```
