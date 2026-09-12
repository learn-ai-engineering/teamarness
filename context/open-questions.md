# Resonance — Open Questions

Design uncertainties that need resolution before or during implementation. Each entry records the question, the decision point it blocks, and what would constitute an answer.

---

## RES-DQ1 — Membership log design: signature chain vs CRDT

**Status:** Resolved (2026-09-12, research).

**Decision:** Adopt p2panda-auth's DAG-based authorization CRDT. Each workspace is a p2panda auth `Group`. Membership operations (admit, revoke) are group operations with the "strong removal" resolver. The "Iroh + p2panda" platform provides a coherent stack: Iroh for transport/peer identity, p2panda for authorization/access control.

**Rationale:** p2panda-auth solves hard distributed-systems problems (concurrency resolution, duelling admins, transitive invalidation) that a custom hash-chained log would need to eventually build. The auth crate is deliberately separate from encryption, so we can adopt auth now and evaluate p2panda-encryption later. See `context/.decisions/0004-p2panda-membership-dag.md`.

---

## RES-DQ2 — Package backend execution model

**Blocks:** `02-system/05-packages/` spec, Phase 2 implementation

**Question:** How should package backends execute relative to the core runtime? In the same Rust process with module-level isolation, or in separate processes?

**Options:**
- A: Same-process Rust modules. Packages are crates or modules within the same binary. Crate-level visibility controls isolation. Simplest, no IPC overhead.
- B: Separate processes with IPC (Tauri sidecars or subprocesses). Strongest isolation, but adds IPC complexity and serialization overhead.
- C: Staged: bundled packages run in-process (trusted); member-loaded packages run in separate webviews or processes (untrusted).

**Considerations:**
- Phase 2 starts with bundled (trusted) packages only. Same-process isolation via crate visibility is sufficient.
- The long-term direction includes member-loaded packages that need stronger isolation.
- Option C matches the existing Resonance design: a shared main webview is not a sandbox; untrusted packages need separate webviews.

---

## RES-DQ3 — Storage ownership and isolation

**Blocks:** Phase 2-3 implementation

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

**Rationale:** Iroh identifies installations, and Phase 1 has no device-management or human-key protocol. Pretending that a device key identifies a human would make the first schema misleading. The p2panda group model leaves a later grouping path, but human identity remains separate Phase 4 design work. See `context/.decisions/0004-p2panda-membership-dag.md`.

---

## RES-DQ5 — Membership log durability

**Status:** Resolved (2026-09-12, design).

**Decision:** Store all Phase 1 core state in one SQLite database under the runtime's private data directory. Store each canonical signed operation as immutable bytes with indexed dependency edges. Use database transactions for workspace creation, invitation spending, admission, epoch events, and membership-version updates.

**Rationale:** The important seam is larger than the operation log. Identity, workspace recovery, invitation use, and membership transitions must commit together. One core database gives those changes a real transaction boundary while append-only guards preserve the membership record. Package databases remain separate. See `context/.decisions/0004-p2panda-membership-dag.md`.

---

## RES-DQ6 — Tauri version and tooling

**Blocks:** Phase 2 implementation

**Question:** Which version of Tauri and which build tooling should we use?

**Options:**
- A: Tauri v2 stable with its built-in tooling. Current production version, well-documented, active ecosystem.
- B: Tauri v2 with a specific frontend framework (Vite, etc.) for package UI development.

**Considerations:**
- Tauri v2 is the current stable major version with significant improvements over v1.
- Package UIs are TypeScript web content; they can use any framework or none. The shell should be framework-agnostic.
- The choice of frontend tooling for the shell itself (Vite, etc.) is separate from what packages use.