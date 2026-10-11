# 0022 — Workspace Person names are Core-owned, not a Profile Package

**Status:** accepted (2026-10-11)
**Refines:** ADR 0003 (Core, Shell and Package ownership), ADR 0004 (device-scoped membership), ADR 0008/0009 (Core synchronization), ADR 0018 (Package capability versioning).

## Context

Conversations displays message authors as Installation identifiers. People need a changeable name per Workspace, available in the Shell and every Package. A later membership workstream will group multiple device identities under one Workspace-scoped human identity. Making names a Profile Package would force a basic identity presentation field to depend on Package lifecycle and cross-Package data sharing; putting them in membership operations would make a rename look like an authorization change. The Shell must not become a profile implementation.

## Decision

Give every Workspace Person a stable opaque identifier derived from their Workspace's immutable creator genesis or first admission operation, **not** from their Installation public key or name. Today one admitted Installation maps to one Workspace Person. Canonical membership still authorizes Installations; a Person and its name are presentation data, not independent membership authority. A later multi-device workstream may attach more Installations to this same Person identifier but must design device attachment and whole-Person removal separately.

Core owns only the common, Workspace-scoped **name**: signed, validated changes, durable projection, and authenticated replication to current Workspace peers. Reuse Core's bounded protected record machinery under a reserved internal scope, unavailable as a Package grant; do not add a membership action, a general profile data store, or a new cross-Package protocol to ship names. Names have their own change cursor and do not alter `MembershipVersion` or membership epoch. Existing Package record-synchronization revocation limits still apply; names do not create a new confidentiality claim.

Expose a separate versioned name-lookup/change Semantic Capability through the Package Host and SDK, alongside the existing membership capability. The Shell only collects a name in its existing Workspace create/join flow, offers a small self-rename control, and displays names in its existing member list. Conversations consumes the same generic capability as any Package to label signed author identities; its messages continue to store author identities, never name strings as authority. A later optional Profile Package may own richer fields and UI, but cannot be required to make Workspace identity usable.

## Rejected alternatives

- A full Profile Package and Package-to-Package dependency for names: larger infrastructure and optional lifecycle for a ubiquitous identity field.
- Installation Display Name propagation: wrong scope and wrong long-term identifier.
- Membership operation for each rename: changes authorization state and protocol for presentation-only data.
- Shell-owned profile state: concentrates product data and synchronization in a deliberately thin host.
- Conversations-specific Core or Shell path: makes other Packages repeat the problem.

## Consequences

- This Core module is intentionally narrow: one name, not arbitrary profile metadata. Additive `teamarness.person-names` capability does not change the existing membership or Process Protocol contracts.
- The internal reserved scope needs explicit exclusion from Package admission/grants and supported external Core append paths; automatic Core subscription and durable key demand must work without any Package Instance. If the existing record engine cannot meet these invariants without disproportionate changes, revisit Design rather than treating this ADR as permission for an unreviewed protocol.
- Names can be temporarily unavailable while offline or waiting for a key; a deterministic identity fallback remains. Duplicate names are valid and never obscure the underlying author identity.

## Evidence

- `context/ontology.md`; `context/roadmap.md` (`Member names`, Phase 5).
- `context/.decisions/0003-core-runtime-vertical-packages.md`, `0004-p2panda-membership-dag.md`, `0008-immutable-record-capability-contract.md`, `0009-core-synchronization-foundation.md`, `0018-package-seam-versioning.md`.
- Workstream `member-names`: corrected accepted Scope `scope/20261011T082817Z_member-names-conversations.md`, research `research/20261006T225309Z_name-seams.md`, and revised Design `design/20261011T085348Z_workspace-person-names.md`.
