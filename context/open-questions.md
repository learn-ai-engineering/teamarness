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

**Decision:** Phase 1 treats installations as first-class members. The internal p2panda adapter uses tagged installation and workspace identifiers and keeps nested groups private, leaving room for a later human group of installation identities without changing installation keys. ADR 0023 uses that room: a Workspace Person is a nested key group that holds its Installation key and its Agent Identities.

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

**Rationale:** The official Tauri release index and package registries confirmed compatible current versions and support the repository's Node.js 24 and Rust 1.96 minimums. The shell and reference package do not need a frontend framework. Research on 2026-09-12 found `tauri` 2.11.5, `tauri-build` 2.6.3, `@tauri-apps/api` 2.11.1, and `@tauri-apps/cli` 2.11.4 in the official release index. The Rust crate and the JavaScript API must share the 2.11 minor. Vite 8.3.0, TypeScript 7.0.2, and Vitest 5.0.0 accept Node.js 24. Tauri recommends the vanilla TypeScript template when no framework is needed, and committing lockfiles. No build was run at research time; later phases built and bundled with these pins.

---

## TEAM-DQ7 - Workspace join protocol

**Status:** Resolved (2026-09-13, design).

**Decision:** Keep onboarding behind high-level Core Runtime operations. Wrap the existing signed invitation in a bounded `WorkspaceInvitation` with an Iroh endpoint ticket. Bind both authenticated peers to the token and claim, commit admission before response delivery, validate and import the complete bootstrap atomically, and report local join only after that import. An exact retry recovers the original admission before expiry checks, then exports the issuer's complete current accepted set at one consistency boundary without creating another admission.

**Rationale:** The issuer and joiner have independent databases, so retry must bridge two commit points rather than pretend they share a transaction. Current-state join completion preserves the matching-membership requirement if another admission or revocation commits before retry. The operation remains bounded and claim-specific; continuous membership synchronization is a separate protocol (ADR 0013). See `context/.decisions/0005-iroh-workspace-onboarding.md`. Its evidence summary is in ADR 0011. Every membership mutation pre-checks that the complete bootstrap stays within 8 MiB and 1,024 operations, so an exact retry can always be exported.

---

## TEAM-DQ8 - Recognizable remote-member names

**Status:** decided for one Installation per Workspace Person in ADR 0022 and delivered; generalized to Workspace Principals by ADR 0023 (2026-10-21).

A Member chooses a Unicode Workspace Person Name at create/join and may rename only their own Person; a Steward names its Workspace Agents. Core signs and synchronizes names separately from Canonical Membership. Shell and Packages consume the same current projection through `teamarness.principals` 1.0, which carries each Principal's kind, Steward, current keys and name; absent, duplicate, or temporarily stale names retain a fingerprint and an inspectable full author identity. Names do not grant membership and are not Installation Display Names. Conversations relabels existing Messages on rename without modifying their signed author identities. Other Workspace names remain independent.

**Still open for Phase 5:** Attach multiple Installation identities to one Workspace Person's key group without changing its ID or name; specify authorized name writers and conflict rules for multiple devices. Historical at-message-time names and cryptographic secrecy after revocation are not delivered by this workstream.

---

## TEAM-DQ9 - Package Instance lifecycle on Workspace leave or local revocation

**Status:** open.

**Blocks:** Roadmap Phase 5 (Membership Management), which introduces Workspace leave and member removal.

**Question:** When the local Installation leaves a Workspace or is revoked from it, does the Package Host suspend that Workspace's Package Instances, dispose them, or retain them and their Package Data Directories?

**Current state:** `ReconcileWorkspaces` activates every admitted Package for every Workspace the Core Runtime lists, and never disposes an Instance. Nothing tells the Package Host that a Workspace has gone away, so a left or revoked Workspace keeps its Backends running until the application stops.

**Options:** the Package Host subscribes to a Core Runtime workspace-set or local-membership stream through a narrow port trait (ARC-06), or the Shell drives suspension and disposal as part of the leave command it already issues.

**Note:** the Core Runtime already gates every capability call on current membership, so this is a lifecycle and resource question, not an authorization one. A revoked Installation's Packages cannot read or write Workspace data; they merely keep running.

**Owner:** roadmap Phase 5.

---

## TEAM-DQ10 - How Packages offer agent-facing guidance for their Ports

**Status:** open (raised 2026-10-25).

**Blocks:** Agent tools for Documents and later Packages (roadmap: **Agent participation in Package Ports**).

**Question:** Where does an Agent learn how to use a Package's Ports well: what is possible, what to do, what not to do? Ports give an Agent the capability; each Package also knows best how that capability should be used.

**Current state:** Port Contracts (`ports.json`) declare exact shapes, with bounded descriptions of at most 512 characters. The standalone Agent Package is the harness for tools and skills, and it has tools only for Conversations. Documents Port Contract 1.1 accepts a bound Agent on `document.revise` (ADR 0033), but no Agent tool calls it yet.

**Options:**
- Each Package ships agent guidance (skills, tool descriptions, perhaps a small library) beside its Port Contract, and an Agent discovers it from the Package at run time.
- Packages publish guidance that a Steward imports and enables for an Agent, choosing from the skills available.
- The Agent Package owns per-Package tools.

The developer prefers loose coupling with high cohesion across the ecosystem, so that Agents engage with every Package naturally.

**Answer looks like:** a design (and an ADR if it adds a contract or capability) that says where guidance lives, how it is versioned with the Port Contract, how an Agent or Steward finds and enables it, and how it is bounded and trusted.

**Owner:** a new Scope, after a developer discussion.
