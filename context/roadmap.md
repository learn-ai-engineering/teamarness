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

Workspace leave, manager assignment and transfer, member removal controls, revocation-driven Content Key rotation (a new Content Key Generation per revocation), and epoch transitions visible to packages. Content Key rotation SHALL grant a newly admitted Member the earlier Content Key Generations of each Package scope, so they read the existing history (`TEAM.SYS.CONV-R12`). Phase 5 introduces multiple Workspace administrators — a small group (for example three or four Members) who can admit and revoke Members and perform other Workspace administration. Version 1's single Manager, the Workspace creator, is a scope limit, not the intended model (ADR 0013). Multi-device support lets one human identity have multiple Installation identities, with device-aware membership operations. Workspace-specific names for one Installation per Person are delivered by the incremental Member names workstream (see **Planned follow-up workstreams** and `TEAM-DQ8`); grouping multiple devices under one Person remains Phase 5 design work. Membership replication and synchronization peer authorization are delivered by ADR 0013; adding a membership action tag requires a new membership-sync protocol version, not a new envelope version. Package Instance lifecycle on leave and revocation (`TEAM-DQ9`) is decided in this phase: today the Package Host never disposes an Instance.

**Outcome:** Workspace managers and members can complete the deferred membership workflows. A removed member cannot read or post future messages, a newly admitted member reads the existing history, and packages rotate keys after membership transitions.

**Depends on:** Phase 4.

### Phase 6 - Repository Integration (complete)

Repository registration, local Git reading, and repository-backed package views. Teamarness does not replicate repository content. Iroh is not used for Git transport. Git remains the sole repository transport.

**Outcome:** A user registers a local clone and a package renders repository-derived context.

Completed on 2026-09-17. The Repo-aware Package (`packages/repo-aware`) registers local Git checkouts, validates and deduplicates their paths, and renders a refreshable snapshot showing path, origin, GitHub URL, branch, commit, and working-tree state. Registrations persist across restarts. It reads Git locally through `gix` and does not synchronize registrations between peers.

**Depends on:** Phase 3.

## Current work

The architecture review and the Shell and Package UI polish workstream are closed and merged. Phase 5 remains next for the core roadmap. The Conversations UI overhaul is complete and verified locally on the isolated `conversations-ui-overhaul` branch at `793b42f`. It adds automatic older history, Package-local projection replay, explicit history-access retry, and stable reading position. Full Cargo and npm checks and exact-range independent review pass with no P0/P1; three P2 follow-ups remain. The developer accepted a limited two-instance desktop check because long-history and other fixtures were unavailable. Those checks are not recorded as passed. An earlier blocked lifecycle journal attempt remains audit debt, not a product defect. The developer approved integration into local `main`; no push is authorized.

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

Incremental delivery on `feature/member-names`: Core-owned Workspace Person IDs and authenticated names, generic `teamarness.person-names` capability, Shell controls and Conversations author presentation. Automated Phase 4 verification is pending independent review and final two-instance desktop Verify; this is not merged into `main`. Resolves `TEAM-DQ8` for one Installation per Person when verified; multi-device grouping remains separate Phase 5 work.

**Depends on:** membership synchronization delivered by the architecture review.

### Shell notifications

Replace the persistent status and error messages at the top of the selected-content pane with a Shell-owned notification mechanism that reserves or overlays space and dismisses non-error notifications automatically. Packages use the same mechanism through a generic capability, so notification presentation stays consistent.

### Package visibility

Let a person hide a Package from contextual navigation and show it again from Workspace settings, for example to hide the Tutorial and Reference Packages after first use.

### Conversations UI overhaul (verified; remote delivery pending)

The Conversations Package UI redesign and history-continuity replacement are complete on the isolated `conversations-ui-overhaul` branch. The Package owns its projection journal, bounded change feed, independent history retries, and reading-position anchor. Automated checks and exact-range review pass. The developer accepted limited two-instance desktop coverage; long-history and other unavailable fixtures remain untested. The developer approved a local merge into `main`. Remote delivery needs a separate decision.

### Conversations history maintenance (unscheduled)

If prioritized, start a separate Scope for these nonblocking review findings: make the advancing-page mount fixture supply a valid open watermark (`SM-007`); avoid one redundant terminal delta request (`SF-006`); and restore the no-request assertion for malformed Channel slugs (`SM-008`). Retest the accepted but unavailable long-history, theme, resize, keyboard-error, and pending-access desktop cases when safe fixtures exist. If an issue appears, open a new workstream rather than rewriting the accepted manual result. Real Message time provenance also needs a separate Scope before dates or times can be shown; no timestamp was fabricated in this workstream.

### Package platform direction (recorded 2026-10-21)

A design conversation on 2026-10-21 compared Teamarness with a thin-shell, package-host application model (WebAssembly components, typed ports, drop-in installation, per-package updates) and a biological membrane analogy. Reading the conversation against the context tree found that most of the "membrane" already exists at the Host seam: manifest-declared capability families negotiated per activation (ADR 0018), closed-schema UI contracts with requests and subscriptions, a Shell-owned UI contract (ADR 0015), and host-brokered calls. Two gaps remain: a Package cannot offer commands, queries, or events to another Package, and there is no Package signing, endorsement, or update distribution. Once Core owns Packages' seams, almost all later development is Package authoring, so these seams decide how far Package work can be parallelized.

The agreed sequence is four workstreams. Each needs its own Scope.

#### Package interface

Implementation in progress: Phase 1 establishes shared declarations and a static authoring catalog; the installation graph, routing, events and Agent integration remain pending.

Workstream `package-interface`. A Package declares the commands, queries, and events it provides and the ones it consumes; the Package Host brokers every call, authorizes by grant with the acting Principal resolved from the Instance binding (ADR 0023), derives the wiring graph from manifests at Package Admission, and activates a provider lazily on first use. This is the interface the Agent Package from `agent-invite` and the Documents Package from `workspace-documents` both need, so it is scheduled first. No change to the Package Process Protocol major, the UI contract, or existing capability families.

#### Data format versioning

An ADR that governs the stored-record seam separately from the live Host seam that ADR 0018 governs. Synced records are signed by peers that cannot be asked to stop, so an old reader must tolerate additive fields and degrade visibly, never reject or fork. Today the Conversations and Documents canonical codecs are positional and reject trailing bytes, and the pinned Loro CRDT will make its encoding part of the Documents format. Scheduled early because there is no install base yet, which makes the rule cheapest now.

#### WebAssembly Package class

Re-evaluate ADR 0006's deferral. Its blocking reason, Package-owned Tokio and Iroh work, was removed by ADR 0007 when Core took ownership of transport and replication, and WASI 0.3 with component-model async has since shipped. The open question is Package-local state without native SQLite or `gix`. Agent Packages holding an Agent key with the user's ambient authority (ADR 0023) are the motivating case for a stronger execution boundary. Outcome: an ADR introducing a second Package class, or a dated re-deferral with explicit criteria.

#### Package distribution and per-Instance restart

Author signature over the Package artifact, a team endorsement of an exact digest and capability set recorded in the membership log, a Host-owned update feed per Package (The Update Framework's threat list as the checklist, Packages never fetch their own updates), a consolidated installed-Packages view with permission diffs, and restarting one Package Instance without restarting the application. Depends on the code-signing work under Later.

## Maintenance rule

Update this roadmap when a phase starts, closes, changes order, or changes direction. Update subsystem requirements and specs when behavior or architecture intent changes. Record hard-to-reverse trade-off decisions in `context/.decisions/`, unresolved design questions in `context/open-questions.md`, and known implementation divergence in `context/.delta/`.
