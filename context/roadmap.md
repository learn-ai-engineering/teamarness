# Resonance — Roadmap

## Status

Active. This file records the expected sequence of major work and the reason for that order.

## Current baseline

The VRS intent tree and accepted architecture decisions are established. Phase 2, the desktop shell and Workspace onboarding, is complete and is the current implementation baseline. Phase 3, Package Foundation, is next.

## Phases

### Phase 0 — Context Foundation (complete)

Established the VRS intent tree, ontology, initial decisions, and open questions before product code. This phase produced the `context/` tree that guides all subsequent work.

**Outcome:** A complete, internally consistent intent layer that answers: what is Resonance, what terms does it use, what decisions have been made, what questions remain open, and in what order will we build it.

### Phase 1 — Core Runtime (complete)

The minimum viable runtime: installation identity (an Ed25519 keypair compatible with an Iroh `EndpointId`), workspace creation, a signed append-only causal membership DAG, single-use invitations, and revocation with durable membership transitions. No UI. No packages. No endpoint binding or network transport.

**Outcome:** A Rust library that generates an identity, creates a workspace, admits a member by invitation, revokes a member, and maintains canonical membership with local epoch cursors and content-derived membership versions. Tests exercise membership, invitations, recovery, tamper rejection, and concurrent operations.

**Depends on:** Phase 0.

### Phase 2 - Desktop Shell and Workspace Onboarding (complete)

Completed on 2026-09-13. This phase added the Tauri desktop shell, bound the runtime's Iroh endpoint, and completed Workspace onboarding between two Installations. One Installation creates a Workspace and a single-use Workspace Invitation. A second Installation pastes the invitation, connects to the inviter through Iroh, joins, persists the Workspace locally, and displays the same member set after explicit refresh. Package hosting, Workspace leave, member removal controls, manager administration, and continuous membership synchronization remain deferred.

**Outcome:** Two independently stored desktop app instances can create and join one workspace through a copied invitation, then show the same workspace identity and membership.

**Depends on:** Phase 1.

### Phase 3 - Package Foundation

The Tauri shell adds package mounting, workspace-level theming, and bounded package failure handling. The TypeScript package SDK covers manifest parsing, semantic capability declarations, package-scoped storage, and the standard lifecycle (`activate`, `suspend`, `recover`, `dispose`). A bundled trusted reference package exercises the foundation end to end. The Tauri invoke and event bridge carries typed, secret-free capability contracts.

**Outcome:** The desktop shell mounts a bundled package, activates it, and tears it down. The reference package proves the SDK and host contract work.

**Depends on:** Phase 2.

### Phase 4 - Conversations Package

The conversations domain as the first real package: channels, signed Markdown messages, Iroh-based P2P transport between package instances, local SQLite persistence, offline queueing, unread state, and channel lifecycle. Channel membership bounded by canonical workspace membership. The package maintains an Iroh `EndpointId` allow-list derived from membership epochs.

**Outcome:** Two peers on the same network exchange encrypted messages in a `#general` channel through the conversations package UI.

**Depends on:** Phase 3.

### Phase 5 - Membership Management and Hardening

Workspace leave, manager assignment and transfer, member removal controls, revocation-driven key rotation, epoch transitions visible to packages, package-scoped membership with core eligibility checks, and forward-only history for newly admitted members. Multi-device support lets one human identity have multiple Installation identities, with device-aware membership operations. This phase also resolves whether recognizable remote-member names are local aliases or signed shared Installation or human profile data.

**Outcome:** Workspace managers and members can complete the deferred membership workflows. A removed member cannot read or post future messages, a newly admitted member sees only post-admission content, and packages rotate keys after membership transitions.

**Depends on:** Phase 4.

### Phase 6 - Repository Integration

Repository registration, local Git reading, and repository-backed package views. Resonance does not replicate repository content. Iroh is not used for Git transport. Git remains the sole repository transport.

**Outcome:** A user registers a local clone and a package renders repository-derived context.

**Depends on:** Phase 3.

## Maintenance rule

Update this roadmap when a phase starts, closes, changes order, or changes direction. Update subsystem requirements and specs when behavior or architecture intent changes. Record hard-to-reverse trade-off decisions in `context/.decisions/`, unresolved design questions in `context/open-questions.md`, and known implementation divergence in `context/.delta/`.
