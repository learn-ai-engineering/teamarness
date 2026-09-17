# Teamarness — Roadmap

## Status

Active. This file records the expected sequence of major work and the reason for that order.

## Current baseline

The VRS intent tree and accepted architecture decisions are established. Phases 0 through 4 and Phase 6 are complete. External Package authoring remains foundation-only while SDK publication policy is deferred. Current work runs in two parallel workstreams (see **Current work**); Phase 5 follows them.

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

### Phase 4 - Conversations Package (complete)

Started on 2026-09-14. Core now owns the first generic Package-scoped Synchronization Capability: a durable, bounded immutable-record journal with production LogSync, authenticated sealed persistence, membership gates, durable wakes, and awaited lifecycle. The Conversations Package owns Channels, signed Markdown Messages, local schemas and projections, unread state, and its UI. Automated qualification covers encrypted two-profile synchronization, bounded history, persistence, Package composition, and delivery timing. Completed on 2026-09-17 after automated and manual verification with two packaged profiles, and integrated into `main`.

**Outcome:** Two separately stored peers exchange encrypted Messages and Channels through the Conversations Package UI without Conversations-specific Core, Host, or Shell routes.

**Depends on:** Phase 3.

### Phase 5 - Membership Management and Hardening

Workspace leave, manager assignment and transfer, member removal controls, revocation-driven key rotation, epoch transitions visible to packages, package-scoped membership with core eligibility checks, and forward-only history for newly admitted members. Multi-device support lets one human identity have multiple Installation identities, with device-aware membership operations. Recognizable member names are a planned follow-up workstream (see **Planned follow-up workstreams** and `TEAM-DQ8`); this phase extends that model to multiple devices per human identity.

**Outcome:** Workspace managers and members can complete the deferred membership workflows. A removed member cannot read or post future messages, a newly admitted member sees only post-admission content, and packages rotate keys after membership transitions.

**Depends on:** Phase 4.

### Phase 6 - Repository Integration (complete)

Repository registration, local Git reading, and repository-backed package views. Teamarness does not replicate repository content. Iroh is not used for Git transport. Git remains the sole repository transport.

**Outcome:** A user registers a local clone and a package renders repository-derived context.

Completed on 2026-09-17. The Repo-aware Package (`packages/repo-aware`) registers local Git checkouts, validates and deduplicates their paths, and renders a refreshable snapshot showing path, origin, GitHub URL, branch, commit, and working-tree state. Registrations persist across restarts. It reads Git locally through `gix` and does not synchronize registrations between peers. `02-system/06-repos/requirements.md` still contains only a placeholder summary.

**Depends on:** Phase 3.

## Current work

These two workstreams run in parallel on separate branches and worktrees. Phase 5 starts after them.

### Shell and Package UI polish

Branch `ui-polish-member-names`. Polish the Shell's Add Workspace screen, Installation controls, Workspace screens, and theme selector, and give every Package UI the same page structure as the Shell's Workspace screens through the Package UI SDK. Member names were split out on 2026-09-17 (see **Planned follow-up workstreams**); the branch name predates that change.

**Outcome:** The collected Shell defects are fixed and Package UIs share one consistent page structure.

### Comprehensive code and architecture review

Branch `architecture-review`. Review the whole system now that several Packages exist, covering architecture, code quality, synchronization, security, and how easy it is to author a Package. Findings become recorded decisions, open questions, deltas, or follow-up workstreams.

**Outcome:** A prioritized, evidence-backed findings report and an agreed remediation sequence.

## Planned follow-up workstreams

Identified on 2026-09-17 during Scope of the UI polish workstream. Each needs its own Scope.

### Member names

Let each person give a name for each Workspace when they create or join it, and show that name to every other Member in the Shell and in Packages, replacing the temporary adjective-animal aliases. Resolves `TEAM-DQ8` for one Installation per person.

**Depends on:** membership synchronization from the architecture review. Today only the issuer and the joiner learn of an admission, so a name carried by membership or a Workspace-scoped record could not reach every Member.

### Shell notifications

Replace the persistent status and error messages at the top of the selected-content pane with a Shell-owned notification mechanism that reserves or overlays space and dismisses non-error notifications automatically. Packages use the same mechanism through a generic capability, so notification presentation stays consistent.

### Package visibility

Let a person hide a Package from contextual navigation and show it again from Workspace settings, for example to hide the Tutorial and Reference Packages after first use.

### Conversations UI overhaul

Redesign the Conversations Package UI beyond the shared Package page structure.

## Maintenance rule

Update this roadmap when a phase starts, closes, changes order, or changes direction. Update subsystem requirements and specs when behavior or architecture intent changes. Record hard-to-reverse trade-off decisions in `context/.decisions/`, unresolved design questions in `context/open-questions.md`, and known implementation divergence in `context/.delta/`.
