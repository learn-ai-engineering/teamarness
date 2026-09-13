# Resonance — Open Questions

Design uncertainties that need resolution before or during implementation. Each entry records the question, the decision point it blocks, and what would constitute an answer.

---

## RES-DQ1 — Membership log design: signature chain vs CRDT

**Status:** Resolved (2026-09-12, research).

**Decision:** Adopt p2panda-auth's DAG-based authorization CRDT. Each workspace is a p2panda auth `Group`. Membership operations (admit, revoke) are group operations with the "strong removal" resolver. The "Iroh + p2panda" platform provides a coherent stack: Iroh for transport/peer identity, p2panda for authorization/access control.

**Rationale:** p2panda-auth solves hard distributed-systems problems (concurrency resolution, duelling admins, transitive invalidation) that a custom hash-chained log would need to eventually build. The auth crate is deliberately separate from encryption, so we can adopt auth now and evaluate p2panda-encryption later. See `context/.decisions/0004-p2panda-membership-dag.md`.

---

## RES-DQ2 — Package backend execution model

**Status:** Resolved (2026-09-12, design).

**Decision:** Trusted bundled package backends run as Rust modules in the main process with crate-level isolation. Package UIs run in Tauri webviews. Raw Core Runtime and Iroh handles remain private behind typed capability contracts. Isolation for future untrusted member-loaded packages remains deferred until that loading model is designed.

**Rationale:** Phase 3 begins with trusted bundled packages, so separate backend processes would add IPC and serialization without protecting against untrusted code in the current Scope. ADR 0003 establishes the in-process package boundary and keeps future stronger isolation possible. See `context/.decisions/0003-core-runtime-vertical-packages.md`.

---

## RES-DQ3 — Storage ownership and isolation

**Blocks:** Phase 3-4 implementation

**Question:** How should package storage be isolated from core storage and from other packages?

**Options:**
- A: Single SQLite database with schema-namespaced tables per package. Simple, one connection, but no storage isolation.
- B: Separate SQLite database per package per workspace. Strong isolation, multiple connections.
- C: Package-owned storage abstraction where the runtime provides a namespaced handle. Packages don't know the underlying database layout.

**Considerations:**
- Option A is what Resonance V1 used and was flagged as a coupling problem.
- Option B aligns with the declared direction that packages own their persistence.
- Option C adds an abstraction layer but provides the cleanest seam for future storage backends.

---

## RES-DQ4 — Device identity vs human identity

**Status:** Resolved (2026-09-12, design).

**Decision:** Phase 1 treats installations as first-class members. The internal p2panda adapter uses tagged installation and workspace identifiers and keeps nested groups private, leaving room for a later human group of installation identities without changing installation keys.

**Rationale:** Iroh identifies installations, and Phase 1 has no device-management or human-key protocol. Pretending that a device key identifies a human would make the first schema misleading. The p2panda group model leaves a later grouping path, but human identity remains separate Phase 5 design work. See `context/.decisions/0004-p2panda-membership-dag.md`.

---

## RES-DQ5 — Membership log durability

**Status:** Resolved (2026-09-12, design).

**Decision:** Store all Phase 1 core state in one SQLite database under the runtime's private data directory. Store each canonical signed operation as immutable bytes with indexed dependency edges. Use database transactions for workspace creation, invitation spending, admission, epoch events, and membership-version updates.

**Rationale:** The important seam is larger than the operation log. Identity, workspace recovery, invitation use, and membership transitions must commit together. One core database gives those changes a real transaction boundary while append-only guards preserve the membership record. Package databases remain separate. See `context/.decisions/0004-p2panda-membership-dag.md`.

---

## RES-DQ6 — Tauri version and tooling

**Status:** Resolved (2026-09-13, research).

**Decision:** Use Tauri 2.11 with exact package pins, vanilla TypeScript, Vite, Vitest, and npm workspaces. Commit the Rust and npm lockfiles. Re-check patch versions if implementation begins after the research date, while preserving Tauri minor-version compatibility.

**Rationale:** The official Tauri release index and package registries confirmed compatible current versions and support the repository's Node.js 24 and Rust 1.96 minimums. The shell and reference package do not need a frontend framework. See `.myflow/workstreams/shell-package-lifecycle/research/20260912T221751Z_tauri-package-execution-storage-and-core-boundary.md`.

---

## RES-DQ7 - Workspace join protocol

**Status:** Resolved (2026-09-13, design).

**Decision:** Keep onboarding behind high-level Core Runtime operations. Wrap the existing signed invitation in a bounded `WorkspaceInvitation` with an Iroh endpoint ticket. Bind both authenticated peers to the token and claim, commit admission before response delivery, validate and import the complete bootstrap atomically, and report local join only after that import. An exact retry recovers the original admission before expiry checks, then exports the issuer's complete current accepted set at one consistency boundary without creating another admission.

**Rationale:** The issuer and joiner have independent databases, so retry must bridge two commit points rather than pretend they share a transaction. Current-state join completion preserves the matching-membership requirement if another admission or revocation commits before retry. The operation remains bounded and claim-specific; continuous membership synchronization is a separate protocol. See `context/.decisions/0005-iroh-workspace-onboarding.md` and `.myflow/workstreams/shell-package-lifecycle/design/20260913T002055Z_workspace-onboarding-protocol.md`.

---

## RES-DQ8 - Recognizable remote-member names

**Blocks:** Phase 5 member presentation, human identity, and multi-device design

**Question:** When a Workspace displays a remote Member, should its recognizable name be a local alias, a signed Installation Display Name shared with peers, a future human profile name, or a combination with explicit precedence?

**Current state:** The Core Runtime stores a validated Installation Display Name as local metadata. Canonical membership identifies Members by Public Identity and does not distribute names. The Shell therefore falls back to long public keys for remote Members.

**What must be resolved:** Define who owns and signs a name, whether it is local or shared, its privacy boundary, rename and conflict behavior, offline convergence, multi-device presentation, and the fallback when no trusted name exists. A display name or alias must never become membership or authorization authority.

**Intended destination:** Phase 5, Membership Management and Hardening. Do not add name propagation to Phase 2 closeout.
