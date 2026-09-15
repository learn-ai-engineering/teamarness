# Teamarness — Roadmap

## Status

Active. This file records the expected sequence of major work and the reason for that order.

## Current baseline

The VRS intent tree and accepted architecture decisions are established. Package Foundation implementation is the current baseline; external Package authoring remains foundation-only while SDK publication policy is deferred. Phase 4, Conversations Package, is in progress.

## Phases

### Phase 0 — Context Foundation (complete)

Established the VRS intent tree, ontology, initial decisions, and open questions before product code. This phase produced the `context/` tree that guides all subsequent work.

**Outcome:** A complete, internally consistent intent layer that answers: what is Teamarness, what terms does it use, what decisions have been made, what questions remain open, and in what order will we build it.

### Phase 1 — Core Runtime (complete)

The minimum viable runtime: installation identity (an Ed25519 keypair compatible with an Iroh `EndpointId`), workspace creation, a signed append-only causal membership DAG, single-use invitations, and revocation with durable membership transitions. No UI. No packages. No endpoint binding or network transport.

**Outcome:** A Rust library that generates an identity, creates a workspace, admits a member by invitation, revokes a member, and maintains canonical membership with local epoch cursors and content-derived membership versions. Tests exercise membership, invitations, recovery, tamper rejection, and concurrent operations.

**Depends on:** Phase 0.

### Phase 2 - Desktop Shell and Workspace Onboarding (complete)

Completed on 2026-09-13. This phase added the Tauri desktop shell, bound the runtime's Iroh endpoint, and completed Workspace onboarding between two Installations. One Installation creates a Workspace and a single-use Workspace Invitation. A second Installation pastes the invitation, connects to the inviter through Iroh, joins, persists the Workspace locally, and displays the same member set after explicit refresh. Package hosting, Workspace leave, member removal controls, manager administration, and continuous membership synchronization remain deferred.

**Outcome:** Two independently stored desktop app instances can create and join one workspace through a copied invitation, then show the same workspace identity and membership.

**Depends on:** Phase 1.

### Phase 3 - Package Foundation (complete)

Completed on 2026-09-14. The Tauri Shell now mounts Package UIs, applies Workspace-level theming, and contains bounded Package failures. Phase 3 defined a language-neutral Package Process Protocol and shipped Rust Backend and TypeScript UI SDKs. Each Package Instance receives a durable Package Data Directory and runs independently of UI visibility. One trusted Rust reference Package exercises the foundation from both an application-bundled Package Source and an explicitly registered installation-local Package Source. Phase 3 also established versioned Semantic Capability negotiation so later phases can add generic Package-scoped synchronization without changing lifecycle or UI contracts. It did not implement a synchronization engine.

**Outcome:** The desktop shell discovers, validates, mounts, activates, recovers, and tears down the same reference package from bundled and configured local sources through one host contract. The documented path from a personal development package to a package shipped with Teamarness does not require package integration code to be rewritten.

**Depends on:** Phase 2.

**External authoring condition:** production Package authoring requires published, versioned Rust and TypeScript SDK artifacts plus a supported generated-project upgrade policy. Vendored foundation projects are not upgraded in place, and no updater is provided.

### Phase 4 - Conversations Package (in progress)

Started on 2026-09-14. The Conversations domain is the first real Package. It owns Channels, signed Markdown Messages, Channel Membership, conversation keys, local schemas and projections, offline behavior, unread state, and UI. It consumes Canonical Membership as an eligibility ceiling. Phase 4 adds the first generic Package-scoped Synchronization Capability for the reusable Iroh transport and replication machinery required by Conversations.

**Outcome:** Two peers on the same network exchange encrypted messages in a `#general` channel through the conversations package UI.

**Depends on:** Phase 3.

### Phase 5 - Membership Management and Hardening

Workspace leave, manager assignment and transfer, member removal controls, revocation-driven key rotation, epoch transitions visible to packages, package-scoped membership with core eligibility checks, and forward-only history for newly admitted members. Multi-device support lets one human identity have multiple Installation identities, with device-aware membership operations. This phase also resolves whether recognizable remote-member names are local aliases or signed shared Installation or human profile data.

**Outcome:** Workspace managers and members can complete the deferred membership workflows. A removed member cannot read or post future messages, a newly admitted member sees only post-admission content, and packages rotate keys after membership transitions.

**Depends on:** Phase 4.

### Phase 6 - Repository Integration

Repository registration, local Git reading, and repository-backed package views. Teamarness does not replicate repository content. Iroh is not used for Git transport. Git remains the sole repository transport.

**Outcome:** A user registers a local clone and a package renders repository-derived context.

**Depends on:** Phase 3.

## Maintenance rule

Update this roadmap when a phase starts, closes, changes order, or changes direction. Update subsystem requirements and specs when behavior or architecture intent changes. Record hard-to-reverse trade-off decisions in `context/.decisions/`, unresolved design questions in `context/open-questions.md`, and known implementation divergence in `context/.delta/`.
