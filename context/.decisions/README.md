# Decision log

Accepted architectural decisions for Teamarness. Each decision is numbered and immutable (ADR 0001, rule 2): an accepted decision is never edited, only superseded by a later decision that cites the clauses it replaces.

This index is the one place that lists every decision's status, its supersessions, and where its evidence is summarised. When an ADR's own header or evidence differs in style, this index is authoritative for status and supersession.

## Index

| Number | Title | Status (date) | Supersedes | Superseded in part by | Evidence summary | Notes |
|---|---|---|---|---|---|---|
| [0001](0001-vrs-intent-tree.md) | VRS intent tree as the context authority | accepted (2026-09-12) | – | – | In the ADR | – |
| [0002](0002-rust-tauri-iroh.md) | Rust, Tauri, and Iroh for the full stack | accepted (2026-09-12) | – | 0012 (lines 15, 17, 38, 51: TypeScript contracts, Tauri bridge, Package author language); 0015 (line 17: Package UI mounting; line 51: Package UI bridge access) | None recorded | Tray, native menus, and auto-updater are intended but unscheduled |
| [0003](0003-core-runtime-vertical-packages.md) | Core runtime with vertical packages architecture | accepted (2026-09-12) | – | 0004 (linear membership chain); 0006 (in-process Package Backends); 0007 (Package transport and host-proxied storage); 0012 (lines 31, 40: key custody and transport; line 73: Conversations transport); 0015 (lines 48, 95: Package UI mounting and isolation) | None recorded | – |
| [0004](0004-p2panda-membership-dag.md) | p2panda causal DAG for canonical membership | accepted (2026-09-12) | 0003 (part) | – | In the ADR | "Phase 4" key-work labels clarified by 0011; membership synchronization no longer deferred by 0013 |
| [0005](0005-iroh-workspace-onboarding.md) | Iroh workspace onboarding with recoverable atomic import | accepted (2026-09-13) | – | – | 0011 | Cited review records cannot be recovered; refined by 0013 |
| [0006](0006-rust-sidecar-package-backends.md) | Rust sidecar Package Backends | accepted (2026-09-13) | 0003 (part) | 0007 (Rust-only Backends) | 0011 | – |
| [0007](0007-language-neutral-package-host-capabilities.md) | Language-neutral Package host capabilities | accepted (2026-09-13) | 0003 (part), 0006 (part) | – | 0011 | Refined by 0018 (seam versioning) |
| [0008](0008-immutable-record-capability-contract.md) | Immutable record capability contract and protection boundary | accepted (2026-09-15) | – | 0014 (Decision, the sentence naming four private Core seams with deterministic adapters for compatibility tests) | 0011 | The immutable-record decision; "Phase 1" label clarified by 0011; refined by 0013 (grant origin, recipient-bound controls), 0017 (custody wording) and 0018 (seam versioning) |
| [0009](0009-core-synchronization-foundation.md) | Core synchronization foundation | accepted (2026-09-15) | – | 0014 (Consequences, first bullet: restart validation of every projection row and sealed envelope before capability calls are accepted) | In the ADR | Status-line phase label clarified by 0011; refined by 0013 and 0017 (custody wording) |
| [0010](0010-vendored-package-authoring.md) | Vendored Package authoring foundation | accepted (2026-09-15) | – | – | None recorded | Formerly `0008-vendored-package-authoring.md`; renumbered by 0011; refined by 0016 (validator crate, provenance) |
| [0011](0011-decision-log-conventions.md) | Decision log numbering, format, phase labels, and evidence | accepted (2026-09-17) | – | – | In the ADR | – |
| [0012](0012-supersession-reconciliation-0002-0003.md) | Reconcile superseded clauses in ADR 0002 and ADR 0003 | accepted (2026-09-17) | 0002 (part), 0003 (part) | – | In the ADR | Does not decide content-key rotation; Package UI isolation decided by 0015 |
| [0013](0013-membership-replication-sync-authorization.md) | Membership replication and synchronization authorization | accepted (2026-09-18) | – | – | In the ADR | Refines 0008 (grant origin) and 0009 (parking, session isolation); membership sync no longer deferred (0004); evidence path moved by plan phase 8 |
| [0014](0014-synchronization-bounds-async-boundary.md) | Synchronization resource bounds, async boundary, and availability events | accepted (2026-09-18) | 0008 (part), 0009 (part) | – | In the ADR | Refines 0013 (session bounds, scheduler) |
| [0015](0015-package-ui-sandboxed-frames.md) | Package UIs run in sandboxed frames | accepted (2026-09-18) | 0002 (part), 0003 (part) | – | In the ADR | Realm, DOM, storage and IPC isolation; not process or hang isolation |
| [0016](0016-package-error-fidelity-and-author-validation.md) | Package error fidelity and author validation | accepted (2026-09-19) | – | – | In the ADR | Refines 0010 (validator crate, provenance); Package-owned errors, one admission rule set, SDK machinery, contract-typed UI requests |
| [0017](0017-key-custody-and-protection-grants.md) | Version 1 key custody and recipient-bound protection grants | accepted (2026-09-19) | – | – | In the ADR | Refines 0008/0009 custody wording; grants are ADR 0013 |
| [0018](0018-package-seam-versioning.md) | Package seam versioning | accepted (2026-09-19) | – | – | In the ADR | Refines 0007 and 0008 (seam versioning); three version lines, family-plus-version negotiation, additive families, intended deprecation window |

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
