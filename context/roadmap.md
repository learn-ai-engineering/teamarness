# Teamarness — Roadmap

## Status

Active. This file records the expected sequence of major work and the reason for that order.

## Current baseline

The VRS intent tree and accepted architecture decisions are established. Phases 0 through 4 and Phase 6 are complete. External Package authoring remains foundation-only while SDK publication policy is deferred (see Later). Phase 5 is next.

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

Workspace leave, manager assignment and transfer, member removal controls, revocation-driven Content Key rotation (a new Content Key Generation per revocation), epoch transitions visible to packages, package-scoped membership with core eligibility checks. Content Key rotation SHALL grant a newly admitted Member the earlier Content Key Generations of each Package scope, so they read the existing history (`TEAM.SYS.CONV-R12`). Phase 5 introduces multiple Workspace administrators — a small group (for example three or four Members) who can admit and revoke Members and perform other Workspace administration. Version 1's single Manager, the Workspace creator, is a scope limit, not the intended model (ADR 0013). Multi-device support lets one human identity have multiple Installation identities, with device-aware membership operations. Recognizable member names are a planned follow-up workstream (see **Planned follow-up workstreams** and `TEAM-DQ8`); this phase extends that model to multiple devices per human identity. Membership replication and synchronization peer authorization are delivered by ADR 0013; adding a membership action tag requires a new membership-sync protocol version, not a new envelope version. Package Instance lifecycle on leave and revocation (`TEAM-DQ9`) is decided in this phase: today the Package Host never disposes an Instance.

**Outcome:** Workspace managers and members can complete the deferred membership workflows. A removed member cannot read or post future messages, a newly admitted member reads the existing history, and packages rotate keys after membership transitions.

**Depends on:** Phase 4.

### Phase 6 - Repository Integration (complete)

Repository registration, local Git reading, and repository-backed package views. Teamarness does not replicate repository content. Iroh is not used for Git transport. Git remains the sole repository transport.

**Outcome:** A user registers a local clone and a package renders repository-derived context.

Completed on 2026-09-17. The Repo-aware Package (`packages/repo-aware`) registers local Git checkouts, validates and deduplicates their paths, and renders a refreshable snapshot showing path, origin, GitHub URL, branch, commit, and working-tree state. Registrations persist across restarts. It reads Git locally through `gix` and does not synchronize registrations between peers.

**Depends on:** Phase 3.

## Current work

The architecture review and the Shell and Package UI polish workstream are both closed and merged. Phase 5 remains next for the core roadmap. The separate Conversations UI overhaul workstream has completed its Package-local UI phase on the isolated `conversations-ui-overhaul` branch. The developer approved the implemented UI after the styling correction; automatic older-history loading has been implemented on the isolated branch. Repeated failed Verify reviews exposed a late-interior Message loss in the Package UI-only history contract. The developer accepted a replacement Package Backend/UI-contract Design and Plan, including a local projection-change journal and explicit retry for rare unannounced history access. The Package-local projection journal (`4f1681b`), Backend/UI contract reconciliation, and view anchoring have been implemented on the isolated branch. Exact-range review found further deferred-replay notification behavior to correct; lifecycle recovery, full re-Verify, and developer real-window checks remain pending. No merge or push is authorized yet.

### Shell and Package UI polish (complete)

Branch `ui-polish-member-names`, merged to `main` on 2026-09-22. Polished the Shell's Add Workspace screen, Installation controls, Workspace screens, and theme selector, and gave every Package UI the same page structure as the Shell's Workspace screens through the Package UI SDK. The branch was ported onto the reviewed tree deliberately: `main` (129 commits of the architecture review) merged into the branch first, with the reviewed implementations kept where the two touched (`adoptPackageStyles`, the typed package client, the error-fidelity fixture) and the branch's page structure kept everywhere else. The port also fixed what the merge silently broke — the Tutorial and Repo-aware UIs rendered their kit page structure without the kit installed — and delivered the two adopted follow-ups: the `createChannel` error-mapping fix (a valid channel name no longer reports the slug-grammar message when the real failure is something else) and the `MAX_PAYLOAD_BYTES` cross-language pairing in `check-protocol-bounds.mjs`. `MAX_PENDING_REQUESTS` was recorded as unpairable in the architecture-review issues log: the Host bounds no per-mount in-flight request count, so no Rust constant exists to pair it with.

**Outcome:** The collected Shell defects are fixed and Package UIs share one consistent page structure.

### Comprehensive code and architecture review (complete)

Branch `architecture-review`, merged to `main` on 2026-09-22. Reviewed the whole system across five parallel lanes — architecture, code quality, synchronization, security, Package authoring — and executed five fix rounds closing every finding at the cause, with ADRs 0013–0018 recorded under `context/.decisions/`. Verification: independent round-4 confirmation (37 executed perturbations, sandbox hermeticity observation) and a consolidated passing review record; full required check green; developer manual smoke 2026-09-22 confirmed the branch's behaviour changes. Carried follow-ups live in the workstream issues log: the content-key stall (Member cannot post until a key holder syncs — Phase 5), two load-dependent test flakes, and the Shell bounds pairing.

## Later (unscheduled)

Intent with no delivery phase yet. Moving an item into a phase is a roadmap change.

- Custom relay server configuration for teams (`TEAM-R11`, `TEAM.SYS.TRANS-R02`).
- System tray, native menus, and auto-updater (`TEAM.SYS.SHELL-R01`, ADR 0002).
- CI-published updates that reach every Member within one hour (vision success criterion 6), with code signing, notarization, and CI/CD (`TEAM.DEL`).
- Builds for platforms other than macOS on Apple silicon.
- Published, versioned Rust and TypeScript SDK artifacts and a generated-project upgrade policy (Phase 3 external authoring condition, ADR 0010).
- Workspace-scoped Package registration and per-Workspace enablement (Package Registration Scope `workspace`). Today every admitted Package runs one Backend per Workspace (4 bundled Packages × 3 Workspaces = 12 processes); measure before scheduling.
- Collaborative planning documents (vision success criterion 4).
- A Conversations delivery qualification that meets `TEAM.SYS.CONV-R16`, including a 25-Member measurement (DELTA-008).
- Platform key custody: store a key-encryption key in the macOS Keychain once signed builds exist (ADR 0017 triggers).

## Not planned

- A `workspace-files` Package transported as Iroh blobs. ADR 0002 does not adopt Iroh blobs; a future document feature will choose its own mechanism.
- Commonware transport (rejected by ADR 0002).

## Planned follow-up workstreams

Identified on 2026-09-17 during Scope of the UI polish workstream. Each needs its own Scope.

### Member names

Let each person give a name for each Workspace when they create or join it, and show that name to every other Member in the Shell and in Packages, replacing the temporary adjective-animal aliases. Resolves `TEAM-DQ8` for one Installation per person.

**Depends on:** membership synchronization from the architecture review. Today only the issuer and the joiner learn of an admission, so a name carried by membership or a Workspace-scoped record could not reach every Member.

### Shell notifications

Replace the persistent status and error messages at the top of the selected-content pane with a Shell-owned notification mechanism that reserves or overlays space and dismisses non-error notifications automatically. Packages use the same mechanism through a generic capability, so notification presentation stays consistent.

### Package visibility

Let a person hide a Package from contextual navigation and show it again from Workspace settings, for example to hide the Tutorial and Reference Packages after first use.

### Conversations UI overhaul (in progress)

Redesign the Conversations Package UI beyond the shared Package page structure. The developer approved the implemented UI and the separate automatic older-history phase is implemented on the isolated branch. Failed re-Verify found further history-state defects; the accepted replacement plan now includes a Package-local migration/change feed, coherent UI request ownership and row anchoring. Implementation proceeds under a documented developer-authorized lifecycle exception; passing re-Verify and developer Phase 2 browser checks remain pending, so this workstream is not delivered.

## Maintenance rule

Update this roadmap when a phase starts, closes, changes order, or changes direction. Update subsystem requirements and specs when behavior or architecture intent changes. Record hard-to-reverse trade-off decisions in `context/.decisions/`, unresolved design questions in `context/open-questions.md`, and known implementation divergence in `context/.delta/`.
