# Teamarness — Open Questions

Design uncertainties that need resolution before or during implementation. Each entry records the question, the decision point it blocks, and what would constitute an answer.

---

## TEAM-DQ1 — Membership log design: signature chain vs CRDT

**Status:** Resolved (2026-09-12, research).

**Decision:** Adopt p2panda-auth's DAG-based authorization CRDT. Each workspace is a p2panda auth `Group`. Membership operations (admit, revoke) are group operations with the "strong removal" resolver. The "Iroh + p2panda" platform provides a coherent stack: Iroh for transport/peer identity, p2panda for authorization/access control.

**Rationale:** p2panda-auth solves hard distributed-systems problems (concurrency resolution, duelling admins, transitive invalidation) that a custom hash-chained log would need to eventually build. The auth crate is deliberately separate from encryption, so we can adopt auth now and evaluate p2panda-encryption later. See `context/.decisions/0004-p2panda-membership-dag.md`.

---

## TEAM-DQ2 — Package Backend execution model

**Status:** Resolved (2026-09-13, prototype and domain modeling).

**Decision:** Each Package Instance runs its Package Backend as an executable in a host-supervised Package Process. The Package Process Protocol is language-neutral. Phase 3 ships a Rust Backend SDK and Rust reference Package, not a Rust-only host contract. Application-bundled and installation-local Package Sources use the same Package Manifest, protocol, lifecycle, capabilities, and Package-owned code.

**Rationale:** The local Package path changed the trade-off behind ADR 0003's in-process choice. A three-candidate prototype found that native libraries conflict with ordinary macOS hardened-runtime library validation, while WebAssembly did not prove the future Conversations background-work model. A sidecar preserved ordinary native dependencies, loaded from both sources, and kept Package aborts and stalled calls outside the host process. ADR 0006 selected sidecars; ADR 0007 supersedes its Rust-only protocol restriction.

---

## TEAM-DQ3 — Storage ownership and isolation

**Status:** Resolved (2026-09-13, scope and domain modeling).

**Decision:** The Package Host assigns each Package Instance a durable Package Data Directory keyed by Installation, Workspace, and Package Identity. The Package owns the directory's contents and may choose SQLite, files, indexes, or another local representation. Source and version changes preserve the directory for the same Package Identity. No supported Teamarness contract exposes Core Runtime storage or another Package's directory.

**Rationale:** A trusted native Package needs direct control over its local representation. Proxying each storage operation through the host would add a second data API without creating a security boundary because the Package Process retains the current user's ambient filesystem authority. ADR 0007 records this boundary and its limits.

---

## TEAM-DQ4 — Device identity vs human identity

**Status:** Resolved (2026-09-12, design).

**Decision:** Phase 1 treats installations as first-class members. The internal p2panda adapter uses tagged installation and workspace identifiers and keeps nested groups private, leaving room for a later human group of installation identities without changing installation keys.

**Rationale:** Iroh identifies installations, and Phase 1 has no device-management or human-key protocol. Pretending that a device key identifies a human would make the first schema misleading. The p2panda group model leaves a later grouping path, but human identity remains separate Phase 5 design work. See `context/.decisions/0004-p2panda-membership-dag.md`.

---

## TEAM-DQ5 — Membership log durability

**Status:** Resolved (2026-09-12, design).

**Decision:** Store all Phase 1 core state in one SQLite database under the runtime's private data directory. Store each canonical signed operation as immutable bytes with indexed dependency edges. Use database transactions for workspace creation, invitation spending, admission, epoch events, and membership-version updates.

**Rationale:** The important seam is larger than the operation log. Identity, workspace recovery, invitation use, and membership transitions must commit together. One core database gives those changes a real transaction boundary while append-only guards preserve the membership record. Package databases remain separate. See `context/.decisions/0004-p2panda-membership-dag.md`.

---

## TEAM-DQ6 — Tauri version and tooling

**Status:** Resolved (2026-09-13, research).

**Decision:** Use Tauri 2.11 with exact package pins, vanilla TypeScript, Vite, Vitest, and npm workspaces. Commit the Rust and npm lockfiles. Re-check patch versions if implementation begins after the research date, while preserving Tauri minor-version compatibility.

**Rationale:** The official Tauri release index and package registries confirmed compatible current versions and support the repository's Node.js 24 and Rust 1.96 minimums. The shell and reference package do not need a frontend framework. See `.myflow/workstreams/shell-package-lifecycle/research/20260912T221751Z_tauri-package-execution-storage-and-core-boundary.md`.

---

## TEAM-DQ7 - Workspace join protocol

**Status:** Resolved (2026-09-13, design).

**Decision:** Keep onboarding behind high-level Core Runtime operations. Wrap the existing signed invitation in a bounded `WorkspaceInvitation` with an Iroh endpoint ticket. Bind both authenticated peers to the token and claim, commit admission before response delivery, validate and import the complete bootstrap atomically, and report local join only after that import. An exact retry recovers the original admission before expiry checks, then exports the issuer's complete current accepted set at one consistency boundary without creating another admission.

**Rationale:** The issuer and joiner have independent databases, so retry must bridge two commit points rather than pretend they share a transaction. Current-state join completion preserves the matching-membership requirement if another admission or revocation commits before retry. The operation remains bounded and claim-specific; continuous membership synchronization is a separate protocol. See `context/.decisions/0005-iroh-workspace-onboarding.md` and `.myflow/workstreams/shell-package-lifecycle/design/20260913T002055Z_workspace-onboarding-protocol.md`.

---

## TEAM-DQ8 - Recognizable remote-member names

**Blocks:** Member presentation in the Shell and Packages; Phase 5 human identity and multi-device design

**Question:** When a Workspace displays a remote Member, should its recognizable name be a local alias, a signed Installation Display Name shared with peers, a future human profile name, or a combination with explicit precedence?

**Current state:** The Core Runtime stores a validated Installation Display Name as local metadata. Canonical membership identifies Members by Public Identity and does not distribute names. The Shell therefore falls back to long public keys for remote Members.

**What must be resolved:** Define who owns and signs a name, whether it is local or shared, its privacy boundary, rename and conflict behavior, offline convergence, multi-device presentation, and the fallback when no trusted name exists. A display name or alias must never become membership or authorization authority.

**Direction (2026-09-17, developer):** Teams using Teamarness know each other, so each person gives a single name field (typically a first name, with no surname) when creating or joining a Workspace, and every other Member sees that name. Generated adjective-animal aliases are temporary and will be removed. Ownership, signing, rename, conflict, and fallback rules remain to be designed.

**Intended destination:** The Shell and Package UI polish workstream (`ui-polish-member-names`) resolves this for one Installation per person. Phase 5 extends it to multiple devices.
