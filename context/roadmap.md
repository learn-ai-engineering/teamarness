# Resonance — Roadmap

## Status

Active. This file records the expected sequence of major work and the reason for that order.

## Current baseline

The VRS intent tree and accepted architecture decisions are established. The Phase 1 Rust core runtime is complete and is the current implementation baseline. Phase 2, the shell and package lifecycle, is next and has not started.

## Phases

### Phase 0 — Context Foundation (complete)

Established the VRS intent tree, ontology, initial decisions, and open questions before product code. This phase produced the `context/` tree that guides all subsequent work.

**Outcome:** A complete, internally consistent intent layer that answers: what is Resonance, what terms does it use, what decisions have been made, what questions remain open, and in what order will we build it.

### Phase 1 — Core Runtime (complete)

The minimum viable runtime: installation identity (an Ed25519 keypair compatible with an Iroh `EndpointId`), workspace creation, a signed append-only causal membership DAG, single-use invitations, and revocation with durable membership transitions. No UI. No packages. No endpoint binding or network transport.

**Outcome:** A Rust library that generates an identity, creates a workspace, admits a member by invitation, revokes a member, and maintains canonical membership with local epoch cursors and content-derived membership versions. Tests exercise membership, invitations, recovery, tamper rejection, and concurrent operations.

**Depends on:** Phase 0.

### Phase 2 — Shell and Package Lifecycle (next)

The Tauri desktop shell with package mounting in webviews, workspace controls, and workspace-level theming. The package SDK in TypeScript with manifest parsing, semantic capability declaration, and the standard lifecycle (activate, suspend, dispose). A reference package that exercises the lifecycle end-to-end. The Tauri invoke/event bridge carries typed, secret-free capability contracts.

**Outcome:** A desktop window that mounts a package, activates it, and tears it down. The reference package proves the SDK works.

**Depends on:** Phase 1.

### Phase 3 — Conversations Package

The conversations domain as the first real package: channels, signed Markdown messages, Iroh-based P2P transport between package instances, local SQLite persistence, offline queueing, unread state, and channel lifecycle. Channel membership bounded by canonical workspace membership. The package maintains an Iroh `EndpointId` allow-list derived from membership epochs.

**Outcome:** Two peers on the same network exchange encrypted messages in a `#general` channel through the conversations package UI.

**Depends on:** Phase 2.

### Phase 4 — Membership Hardening

Revocation-driven key rotation, epoch transitions visible to packages, package-scoped membership with core eligibility checks, forward-only history for newly admitted members. Multi-device support: a human identity may have multiple device identities, with device-aware membership operations.

**Outcome:** A removed member cannot read or post future messages. A newly admitted member sees only post-admission content. Packages receive epoch transition events and rotate keys accordingly.

**Depends on:** Phase 3.

### Phase 5 — Repository Integration

Repository registration, local Git reading, and repository-backed package views. No peer replication of repository content. Iroh is not used for Git transport — Git remains the sole repository transport.

**Outcome:** A user registers a local clone and a package renders repository-derived context.

**Depends on:** Phase 2 (shell can mount packages; repo package is a second package).

## Maintenance rule

Update this roadmap when a phase starts, closes, changes order, or changes direction. Update subsystem requirements and specs when behavior or architecture intent changes. Record hard-to-reverse trade-off decisions in `context/.decisions/`, unresolved design questions in `context/open-questions.md`, and known implementation divergence in `context/.delta/`.
