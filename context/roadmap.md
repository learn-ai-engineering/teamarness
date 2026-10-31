# Teamarness — Roadmap

## Status

Active. This file records the expected sequence of major work and the reason for that order.

## Current baseline

The VRS intent tree and accepted architecture decisions are established. Phases 0 through 4 and Phase 6 are complete. Phase 7 (Documents Package) was integrated into `main` on 2026-10-24 for developer testing by explicit approval. Its overall Verify remains failed on two accepted internal-stage risks; the separate Documents layout workstream passed its scoped Verify. Integration is not a production release or a passing overall Verify. External Package authoring remains foundation-only while SDK publication policy is deferred (see Later). Documents rendered editing (ADRs 0031, 0032 and 0033) passed Verify and was integrated into `main` on 2026-10-25. Phase 5 is next for the core.

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

Workspace leave, manager assignment and transfer, member removal controls, revocation-driven Content Key rotation (a new Content Key Generation per revocation), and epoch transitions visible to packages. Content Key rotation SHALL grant a newly admitted Member the earlier Content Key Generations of each Package scope, so they read the existing history (`TEAM.SYS.CONV-R12`). Phase 5 introduces multiple Workspace administrators — a small group (for example three or four Members) who can admit and revoke Members and perform other Workspace administration. Version 1's single Manager, the Workspace creator, is a scope limit, not the intended model (ADR 0013). Multi-device support lets one human identity have multiple Installation identities, with device-aware membership operations; ADR 0023 already models a Workspace Person as a key group, so this work attaches further Installation keys to that group. Workspace-specific names for one Installation per Person were delivered by the Member names workstream and generalized to Principal Names by the Workspace Principals workstream (see **Current work** and `TEAM-DQ8`); grouping multiple devices under one Person remains Phase 5 design work. Membership replication and synchronization peer authorization are delivered by ADR 0013; adding a membership action tag requires a new membership-sync protocol version, not a new envelope version. Package Instance lifecycle on leave and revocation (`TEAM-DQ9`) is decided in this phase: today the Package Host never disposes an Instance.

**Outcome:** Workspace managers and members can complete the deferred membership workflows. A removed member cannot read or post future messages, a newly admitted member reads the existing history, and packages rotate keys after membership transitions.

**Depends on:** Phase 4.

### Phase 6 - Repository Integration (complete)

Repository registration, local Git reading, and repository-backed package views. Teamarness does not replicate repository content. Iroh is not used for Git transport. Git remains the sole repository transport.

**Outcome:** A user registers a local clone and a package renders repository-derived context.

Completed on 2026-09-17. The Repo-aware Package (`packages/repo-aware`) registers local Git checkouts, validates and deduplicates their paths, and renders a refreshable snapshot showing path, origin, GitHub URL, branch, commit, and working-tree state. Registrations persist across restarts. It reads Git locally through `gix` and does not synchronize registrations between peers.

**Depends on:** Phase 3.

## Current work

### Agent Doc Exhibit (in progress)

Started on 2026-10-31 on branch `don-smith/agent_doc_exhibit` in the Host and the standalone Agent Package, workstream `agent-doc-exhibit`. A Steward's Agent works with Documents and Exhibits in chat, and the Agent's Configuration moves to its Package page.

- ADR 0038 (accepted 2026-10-31) amends ADR 0033 §1. Documents Port Contract 1.2 adds `document.create`, which a bound Agent may call, and a `runtime.guide`, so Documents is an agent-facing contract under ADR 0035. A create is title-only and returns the version for a first revise. An archived Collection is refused with `documents.collection_archived`.
- The Agent creates and revises Documents as itself, including Documents a Person created.
- The Agent stops withholding the Exhibits `file.put` and `file.read` Ports, and publishes small multi-page Exhibits. Before it sends a tool call, it refuses one whose arguments would exceed the turn's remaining message budget, with `agent.tool_arguments_too_large`.
- The Agent's main view shows Chat only, and its Configuration is the Package's settings view.
- Exhibits Port Contract 1.1 (additive) adds `file.write`: a model sends a whole text file as plain `text` (at most 98,304 bytes), and the Provider computes the SHA-256 and stores it. The guide tells models never to compute hashes or base64 themselves; `file.put` stays for binary files.
- Exhibits Port Contract 1.2 (additive) adds `file.text`, the read-back pair of `file.write`: it returns a stored text file as plain text in pages of 8,192 bytes by default, so a model can revise an Exhibit it did not just write. `file.read` stays for binary files.

Integration evidence, 2026-10-31: three external cases in `tests/system/tests/agent_reply_flow.rs` run the built Agent against real Documents and Exhibits, with only the model scripted. In the first case, the bound Agent enables Documents, reads the guide, creates and revises a Document, and revises a Document that a Person created. Core shows the Agent's key as the author of the create and both edits. In the second case, the Agent uploads two linked pages, creates a Draft, finds it by title, makes revision 2 and reads a page back as text with `file.text`. In the third case, an oversized `file.put` is refused with `agent.tool_arguments_too_large` and nothing is stored. The next model request carries the elided placeholder, and the turn ends with its reply. Verify has not run yet.

**Outcome:** In chat, an Agent creates and revises a Document and publishes an Exhibit that the Steward opens by name, all attributed to the Agent.

**Follow-ups:** a shared, human-curated Exhibit library catalog; larger Exhibits and an archive format; per-Agent turn bounds; and staging the Agent harness for the Host's integration tests.

### Packagement (complete, verified locally)

Implemented on 2026-10-25 on branch `don-smith/packagement`, workstream `packagement`. ADR 0036 (accepted 2026-10-25) splits Package Admission into a Team tier, fixed at Host open, and a Personal tier that changes while the Host runs. A Team Package never routes to a Personal Provider.

- The Host's registration store inspects a Package Directory without running it and writes the existing `registrations.v1.json` atomically, after an admission preflight that records nothing. Hand-written files keep working.
- The Host installs, reloads, restarts, and removes one Personal Package at a time without restarting Team Packages. A live claim on a Port Contract that a running Package provides is refused with `port_provider_conflict`.
- The Shell adds **+** (**Add Personal Package**) on the Workspace page: folder picker or pasted path, preview, native-code consent, and install. A Team Package override needs a second confirmation and applies after **Restart Teamarness**. Refused registrations appear under **Needs attention on this device**.
- Every Package has a Package page with its metadata, trust, capabilities, and Ports. Personal Packages get Reload, Restart, Pause, Resume, Start, and Remove; Remove keeps the Package Data Directory.
- The manifest gains optional `presentation.description`, `author`, and `homepage`, and an optional `ui.views` settings view (`TEAM.SYS.PKG-R19`). `uiHostVersion` is 2 (ADR 0037).

On 2026-10-31 the branch was integrated with Package Ports 2.0 (plan `ports-2-integration`). `main` was merged in. The broker now resolves each operation against one swappable, immutable Port snapshot, and every Personal change publishes a new snapshot that invalidates open metadata cursors. Team callers, including a Team Package acting as an Agent, see Team Providers only. Personal callers, including the standalone Agent harness, see both tiers. A Personal Provider that loses a required Port is no longer routed to or offered to Agents. Host integration tests cover Provider exit, retained Agent selections, and Agent reload with an unresolved command, which is dispatched at most once and never replayed. The external cross-profile Agent reply case also fails on unchanged `main`; Don accepted it on 2026-10-31 as pre-existing, and [DELTA-017](.delta/DELTA-017-cross-profile-agent-reply.md) tracks it.

Verify passed on 2026-10-31 after one corrective phase. Don's manual checks passed on two installations, including a reply to a mention from a member who did not install the Agent. Don accepted the remaining review findings as tech debt, and [DELTA-018](.delta/DELTA-018-packagement-accepted-tech-debt.md) records them. The branch was integrated into local `main` on 2026-10-31. This branch's ADRs, accepted as 0031 and 0032, were renumbered to 0036 and 0037 when `main` was merged in, because `main` already uses 0031 to 0035.

**Outcome:** A person installs, rebuilds, and removes a Personal Package from the Shell without touching files or restarting Teamarness, and Team Packages keep running.

**Next:** the `package-builder` workstream, which builds Personal Packages inside the Shell on top of this live tier.

### Agent-controlled Package Ports (complete, verified locally)

Completed on 2026-10-30 in workstream `agent-port-discovery`, with the paired standalone Agent migration. ADR 0035 supplies complete bounded metadata, package-ports 2.0, durable Host-owned per-harness selections and lifecycle revocation. Conversations 1.3 adds bounded runtime guidance without changing its five earlier operations. Declared composition and dynamic Agent access share broker validation, authorization-time actors, generation safety and uncertainty handling. Old 1.x consumers must be rebuilt; there is no compatibility dispatcher.

The qualified Host implementation is `6073dd0`, paired with standalone Agent `7cf7358` on `agent-package-ports-2`. The Agent discovers unknown guide-bearing contracts, uses their complete schemas and configures Host-owned selections without a Provider-specific adapter. Its bounded schema preparation refuses excessive work before model or Port effects. Agent-owned unresolved commands prevent unsafe identity changes, survive cancelled waiters and are never replayed. Ordinary Provider refusals release ownership; uncertain outcomes do not silently clear it.

All required implementation checks, the nine-case real Agent/Host integration target, cumulative independent source reviews and provenance audits pass. Final source evidence is in `verify/20261027T012416Z_{review,validation}.md`; manual acceptance is in `verify/20261030T234629Z_validation.md`. Don confirms the rebuilt Agent, real-model Conversations guidance, enable/disable, A/B binding and unbind, and retained binding/selections after an app restart. Provider stop/recover has no current desktop control; Don accepts the existing automated evidence instead of a manual observation. Catch-up variation is accepted, not a requested behavior change.

Don approved local integration: reconcile main's documentation into alignment, land the Host first, then fast-forward standalone Agent main. No push or release is authorized. Registration replacement still takes effect on the next Host open. Packagement was then separate and had to preserve ports 2.0 when integrating, and to qualify restarting or reloading the Agent while commands remained unresolved. Its 2026-10-31 integration preserved ports 2.0 and qualified reloading the Agent with an unresolved command; see **Packagement** above. Per-Package Restart with an unresolved command is not separately tested ([DELTA-018](.delta/DELTA-018-packagement-accepted-tech-debt.md)). Existing full-Host retirement tests did not prove that case or that detached Provider work is quiescent.

Follow-ups remain separate: Documents guidance/tools and Comments; Agent Chat versus the Host-owned Configuration view in Packagement; Exhibits file tooling; and the nonblocking published contract-identity grammar mismatch (`SM-003`, `contracts/package-capabilities/v1/package-ports-2.0.schema.json`). Historical Documents `StaleMount`, Core async-boundary stall and name-convergence timeout remain unexplained; passing checks do not establish their cause or repair.

### Exhibits Package (slice 1 delivered; slice 2 planned)

Slice 1 was implemented on 2026-10-25 on branch `don-smith/link_and_show`, workstream `exhibits-package`. It passed Verify and was integrated into `main` on 2026-10-25. The developer's manual checks are deferred until an Agent can publish Exhibits. An Agent, or any Port Consumer acting for a Person, authors an Exhibit: a multi-page, interactive web experience whose files are all bundled. The bundled Exhibits Package (`dev.teamarness.exhibits`, on-demand activation) stores Drafts in its Package Data Directory and provides the `dev.teamarness.exhibits` 1.0 Port Contract: chunked, content-addressed uploads, atomic Draft updates with revision checks, paged list and read, and a `runtime.guide` query that publishes the enforced limits (`TEAM.SYS.PKG-R18`). Its UI lists Exhibits newest first and shows the selected Draft in a nested frame, replacing it when a newer revision arrives.

Two generic seams carry the content, and neither names Exhibits. The Host capability `teamarness.content-views` 1.0 serves verified files from a Package's own Package Data Directory (`TEAM.SYS.PKG-R16`). The Shell scheme `teamarness-content:` serves them with an opaque origin and scripts only, and only a Package that declares the capability may frame it (`TEAM.SYS.PKG-R17`, ADR 0034). The packaged-app smoke renders a Draft through the real Ports, Host and scheme, changes the open Draft to its next revision without reselecting it, and its isolation probe finds no breach, with or without a `sandbox` attribute on the frame. The frame without the attribute is nested in the Shell's sandboxed Package UI Frame and inherits its flags, so the smoke proves the end-to-end result; the `content_protocol.rs` tests prove the CSP `sandbox` directive itself at header level. Content frames can still reach the network through navigation and WebRTC; see [DELTA-015](.delta/DELTA-015-exhibit-content-network-access.md).

**Outcome:** A Person sees an Agent's multi-page Exhibit inside Teamarness, interacts with it, and sees the Agent's next revision without reopening it.

**Depends on:** Package interface, Agent Package (ADR 0030 actor).

**Slice 2 (planned):** a Person takes an immutable Snapshot of a Draft to share it with the Workspace. Snapshots are manifest records on the immutable record log. Their files travel through a new Core-native blob capability family (`teamarness.synchronization.blobs`) as content-addressed, Content-Key-sealed blobs. Slice 2 also adds Snapshot history, continuing from any Snapshot, and local remove and restore. Teaching the standalone Agent Package to call the Exhibits Ports is a separate workstream in that Package's repository.

### Agent Package (complete, verified locally)

Completed and verified on 2026-10-24, including incoming local main `a947a17` merged into the feature branch and constructor/provenance correction `a67f3c4`. The final merged Root checks, all four explicit external-Agent/Host cases, and focused independent review pass. The standalone Agent's verified runtime is `ed3c016`. The developer approved fast-forward integration into local `main`. No source push or publication is authorized.

Implemented on 2026-10-24 on branch `don-smith/agent_package`, workstream `agent-package`. ADR 0030 (accepted 2026-10-24) makes a bound Agent a standing local signer: Core signs with an Agent's key for any Package Instance on the Steward's Installation once the Agent has a binding there, bindings are keyed by Package through the new `teamarness.agent-binding` 1.0 capability, and the Host derives an Agent actor for a bound caller's Port calls. Conversations 1.1 accepts that actor on `message.post` and adds `channel.list`. The optional standalone Personal Agent Package `dev.teamarness.agent` is the harness for one Agent per Package Instance: the Steward picks the Agent and enters an OpenAI-compatible endpoint, model and API key in the Package's own UI; the Agent replies once when a Person addresses it by name, keeps per-Channel progress so a restart never re-replies, and reports endpoint failures as Package-owned status codes without retrying.

The delivery correction adds Conversations 1.2 `message.changes`, exposing the existing UI projection journal through a query Port. Agent catch-up now persists epoch/revision progress rather than Message display positions, processes late lower-order arrivals and bounded backlog across passes, and retries safe reads without retrying settled model/post attempts. Automated regression and two-profile tests cover offline lower-order delivery and a 257-Message backlog. No hypothetical legacy binding/progress migration is introduced. Bounded cleanup also fixes rebind attribution, startup/reconnect roster reconciliation, completion redirects, status ordering and blocking storage paths. Independently owned dispatch and binding work survives local waiter cancellation; private Host authority generations fence obsolete work across Backend replacement. Main's append-generation retirement and qualified replay are preserved. A failed or uncertain post or binding write can stop dispatch until full application-process restart; configuration changes do not bypass that safety state.

**Outcome:** A Member on another Installation addresses the Agent by name in Conversations, and both Installations show the reply attributed to the Agent with its Steward.

**Depends on:** Workspace Principals, Package interface.

Automated coverage spans Core, Host, SDK, Conversations and Agent tests, a 257-Message two-profile backlog, queued-post actor attribution, and cancelled-binding-waiter ownership against a real test Host. The final merged validation and review are `verify/20261024T092503Z_post-merge-{validation,review}.md` in the `agent-package` workstream. No current Agent or merge findings remain. The developer observed real-endpoint replies, Personal Package discovery, retained configuration after restart, and an offline-addressed reply visible at the first inspection after Alice returned. These observations are not a complete manual matrix, measured latency, or duplicate audit. They were not repeated after integration. Incoming Documents limitations remain separate. The inherited record-log authoring-schema mismatch is tracked in [DELTA-012](.delta/DELTA-012-record-log-principal-schema.md). Key custody, streaming, tools, Documents participation, scheduled posts, SDK publication, and stronger Package isolation remain separate Scopes under Later.

### Agent Package tools and guides, slice 1 (complete)

This slice was implemented on 2026-10-25 in the standalone Agent Package repository, branch `proper_harness`, workstream `agent-harness`.

**What it delivers**

- The Agent offers each enabled agent-facing Package to the model as tools that the generic Port client builds. A contract is agent-facing when it has a `runtime.guide`.
- The model first sees one line per Package. It loads the Package guide, and the Agent's own compiled-in skill, only when it asks for them.
- Arguments are validated against the Port schema before any dispatch.
- A Provider error reaches the model with its stable code.
- A command is never re-sent, and an unknown outcome is reported to the model as unknown.
- Each turn is bounded by a model-call budget, a tool-call budget, a cap on each result, a cap on how much the turn's own tool exchange can grow its messages, and a deadline.
- Chat shows which Package operations an answer used.
- Channel replies can use tools, and the harness still posts exactly one reply.
- With nothing enabled, requests are unchanged.

**Change to the earlier Agent Package**

At `b5202d0`, this slice removed the earlier dispatch fence. The later **Agent-controlled Package Ports** workstream reproduced a real preauthorization attribution race and superseded that decision with Agent-owned command tracking. Binding changes now refuse while commands remain unresolved; ordinary certain refusals release ownership, while uncertainty retains the identity-change guard. Already-authorized commands keep their original Host-derived actor. The original Agent Package's broad fail-closed dispatch rule is not the current policy.

**Status**

Completed on 2026-10-26. Verify attempt 3 passed at `b5202d0`, and the developer approved a fast-forward of the Agent repository's local `main` to `b5202d0`. No remote exists and nothing was published. Two corrective phases fixed issues that earlier Verify attempts found:

- An over-wide message bound broke unchanged no-tool turns.
- A failed turn lost its activity.
- A tool batch from the last model call still ran.
- Unsolicited tool calls were acted on.
- The Packages switch showed the wrong state.

**Manual evidence**

On 2026-10-26 the developer checked the following in the real app:

- With Exhibits not installed, the agent couldn't reach it.
- With Exhibits installed and enabled, Chat saw it and answered correctly.
- Switching Exhibits on and off worked.
- Mentions in Conversations still got replies.

The developer didn't report checking restart persistence of the switch, an exact count of one reply per mention, or a tool command while the Agent is unbound. Automated tests cover the first two.

**Teamarness follow-up**

The later **Agent-controlled Package Ports** workstream replaced the obsolete full-dispatch-fence expectation with precise preauthorization and already-authorized actor checks. All nine external `agent_reply_flow` cases pass against Agent `7cf7358` and Host `6073dd0`; no attribution or no-replay assertion was waived.

**Retained P2 follow-ups**

These are recorded in the `agent-harness` Verify reports:

- an Agent can't create an Exhibit yet, because the Agent withholds `file.put` (and `file.read`). Uploads were judged too large for the turn bounds, but a small HTML Exhibit fits. The developer confirmed the gap in a manual check on 2026-10-26. The fix is either to stop withholding `file.put` and let the turn bounds refuse an oversized chunk with a stable code, or to wait for the Exhibits slice 2 blob capability;
- a command in flight at the deadline is missing from the failed-turn activity;
- an interrupted chat turn drops the activity of tool calls that already finished;
- the final model call is still offered tools it can't use;
- keyboard focus is lost on each Packages toggle;
- a running turn keeps its tools after a Package is disabled;
- Channel content can steer commands on reply turns, a prompt-injection surface;
- documentation and comment cleanups.

**Not how it should stay**

The original slice used a bundled catalog, manifest-declared optional contracts, Agent-local enablement and package-ports 1.x. The paired **Agent-controlled Package Ports** migration replaces these with runtime metadata and Host-owned selections, resolving [DELTA-016](.delta/DELTA-016-agent-harness-bundled-catalog.md). A real unknown major-3 Provider works without an Agent rebuild, and Conversations guidance is qualified. Documents still needs a guide before its deferred Agent chat/revise check can run.

### Workspace Principals (complete)

Verified on 2026-10-23 on branch `don-smith/agent_invite`, workstream `agent-invite`. ADR 0023 (accepted 2026-10-21) defines Workspace Principals: a Workspace Person is a key group nested in the Workspace group, a Workspace Agent is a Principal whose key the Steward's Installation custodies, and attribution is the signer key resolved to its Principal. The workstream replaces the names capability of ADR 0022 with `teamarness.principals` 1.0, adds the optional `principal` selector to `teamarness.synchronization.immutable-record-log` 1.1, lets any Member add an Agent by name from the Shell without a Manager, lists Agents with their Steward, and shows Agent authors in Conversations. The agent runtime Package, cross-Package reads, and Shell-driven Package binding are the next workstream.

**Outcome:** Every Member sees a Workspace Agent with its name and Steward on every Installation, and a bound Package Instance appends records as that Agent.

**Depends on:** Member names.

The final source passed the full repository check and an exact-range review with no P0 or P1 findings. Don verified create, invite, join, Agent management and naming, two-profile synchronization, restart persistence, and Person-authored Conversations on the final source. Person revocation and Agent-authored Message composition have no supported desktop route; Core, Host, and UI tests cover their respective seams. See the `agent-invite` Verify artifacts for the five retained P2 notes and the intermittent Core test failure.

The architecture review and the Shell and Package UI polish workstream are closed and merged. Phase 5 remains next for the core roadmap. The Conversations UI overhaul is complete and verified locally on the isolated `conversations-ui-overhaul` branch at `793b42f`. It adds automatic older history, Package-local projection replay, explicit history-access retry, and stable reading position. Full Cargo and npm checks and exact-range independent review pass with no P0/P1; three P2 follow-ups remain. The developer accepted a limited two-instance desktop check because long-history and other fixtures were unavailable. Those checks are not recorded as passed. An earlier blocked lifecycle journal attempt remains audit debt, not a product defect. The developer approved integration into local `main`; no push is authorized.

### Shell and Package UI polish (complete)

Branch `ui-polish-member-names`, merged to `main` on 2026-09-22. Polished the Shell's Add Workspace screen, Installation controls, Workspace screens, and theme selector, and gave every Package UI the same page structure as the Shell's Workspace screens through the Package UI SDK. The branch was ported onto the reviewed tree deliberately: `main` (129 commits of the architecture review) merged into the branch first, with the reviewed implementations kept where the two touched (`adoptPackageStyles`, the typed package client, the error-fidelity fixture) and the branch's page structure kept everywhere else. The port also fixed what the merge silently broke — the Tutorial and Repo-aware UIs rendered their kit page structure without the kit installed — and delivered the two adopted follow-ups: the `createChannel` error-mapping fix (a valid channel name no longer reports the slug-grammar message when the real failure is something else) and the `MAX_PAYLOAD_BYTES` cross-language pairing in `check-protocol-bounds.mjs`. `MAX_PENDING_REQUESTS` was recorded as unpairable in the architecture-review issues log: the Host bounds no per-mount in-flight request count, so no Rust constant exists to pair it with.

**Outcome:** The collected Shell defects are fixed and Package UIs share one consistent page structure.

### Comprehensive code and architecture review (complete)

Branch `architecture-review`, merged to `main` on 2026-09-22. Reviewed the whole system across five parallel lanes — architecture, code quality, synchronization, security, Package authoring — and executed five fix rounds closing every finding at the cause, with ADRs 0013–0018 recorded under `context/.decisions/`. Verification: independent round-4 confirmation (37 executed perturbations, sandbox hermeticity observation) and a consolidated passing review record; full required check green; developer manual smoke 2026-09-22 confirmed the branch's behaviour changes. Carried follow-ups live in the workstream issues log: the content-key stall (Member cannot post until a key holder syncs — Phase 5), two load-dependent test flakes, and the Shell bounds pairing.

### Phase 7 - Documents Package

Started on 2026-10-18 on the `workspace-documents` workstream. The Documents Package (`packages/documents-package`) owns Collections, Documents, Comments, and a collaborative plain-text Markdown body replicated as Loro updates on the immutable record log, with no Documents-specific Core change (ADR 0027). Automated qualification covers record convergence in any order, deferral and quarantine, Loro import order independence, coalesced and bounded edit records, restart replay, and the UI edit and comment flows. The separate `documents-layout` workstream completed its layout, quoting, and minimal-controls changes with a passing scoped review, automated checks, and developer two-instance UI observations. The developer explicitly approved merging and pushing this Package for further testing despite the failed overall Verify on SF-701 and SF-702. The merged Host and Documents Principal-capability seams passed the full `npm run check`; no new post-merge native-browser observation was claimed. Two-second visibility across separate networks, offline merge, and late join were not reported as passed. An existing Member's app-closed metadata catch-up observation is deferred in [DELTA-011](.delta/DELTA-011-documents-offline-metadata-catchup.md).

**Outcome:** Two team members on separate networks edit a planning document together and discuss it in comments that stay with the document (vision success criterion 4).

**Depends on:** Phase 4. Follow-up: folder mapping, so a mapped folder lets any editor or CLI agent edit documents.

#### Documents rendered editing (verified and integrated 2026-10-25)

Implemented on branch `don-smith/rendered_doc_editing`, workstream `rendered-document-editing`, from `e6b2afc`; it passed Verify and was integrated into `main` on 2026-10-25. ADR 0032 replaces the plain-text body: a Document body is a structured Loro tree that a Member edits in rendered form, and the Package UI is a Loro peer through `loro-prosemirror`. ADR 0031 lets Package UI frames run WebAssembly by adding only `'wasm-unsafe-eval'`. The workstream delivers:

- the Rust Markdown core: Canonical Markdown export and base-versioned merge, proven by property tests, with the [Markdown contract](02-system/09-documents/markdown-contract.md);
- the Body Genesis in `document.create`, Backend-allocated peers, the 160 KiB record bound, and validation before append;
- Editor Sessions fed by snapshots, with relay, resync, and peer resume;
- the rendered editor, with per-person undo, split-then-refuse paste guards, copy as Markdown, and comment on selection;
- Documents Port Contract 1.1 (`document.list`, `document.markdown`, `document.revise`, `documents.changed`), proven through the Host broker, where a Workspace Agent bound on the Installation revises a Document as itself (ADR 0033).

The record format changed incompatibly, so development data directories that hold Documents must be reset. Automated checks pass, including the WebKit and Chromium frame scenarios (`npm run verify:documents-editor`). The long property run passed in Verify, and the developer checked live sync and an offline-then-online merge across two instances. The packaged-app isolation probe, IME input, and the two-second latency observation were not run before integration and remain open checks. Frame-policy findings are recorded in [DELTA-014](.delta/DELTA-014-package-ui-frame-form-and-select-behaviour.md). Edge cases that Verify confirmed and the developer accepted as tech debt are recorded in [DELTA-013](.delta/DELTA-013-documents-editor-accepted-tech-debt.md). Agent document tools, Agent mentions and Comment ports are tracked under **Agent participation in Package Ports**, and the remaining editor work is tracked under **Documents editor maintenance**, both in Planned follow-up workstreams.

**Outcome:** Members edit a Document in rendered form and converge, and another Package reads and revises it as Markdown against a base version.

**Depends on:** Phase 7 (Documents Package), Package interface (ADR 0026).

## Later (unscheduled)

Intent with no delivery phase yet. Moving an item into a phase is a roadmap change.

- Custom relay server configuration for teams (`TEAM-R11`, `TEAM.SYS.TRANS-R02`).
- System tray, native menus, and auto-updater (`TEAM.SYS.SHELL-R01`, ADR 0002).
- CI-published updates that reach every Member within one hour (vision success criterion 6), with code signing, notarization, and CI/CD (`TEAM.DEL`).
- Builds for platforms other than macOS on Apple silicon.
- Published, versioned Rust and TypeScript SDK artifacts and a generated-project upgrade policy (Phase 3 external authoring condition, ADR 0010).
- Workspace-scoped Package registration and per-Workspace enablement (Package Registration Scope `workspace`). Today every admitted eager Package runs one Backend per Workspace (5 eager bundled Packages × 3 Workspaces = 15 processes); the on-demand Exhibits Package starts only on first use. Measure before scheduling.
- Folder mapping for Documents: a mapped folder lets any editor or CLI agent edit documents, importing external changes as Markdown revisions merged against a base version (follow-up to Phase 7, ADRs 0027 and 0032).
- View earlier Document versions and copy from them, built on the immutable record history. A change too large to undo in one record is not undoable (ADR 0032), and this is how a Member recovers from one.
- Document log compaction via the reserved `document.snapshot` record kind (ADR 0027). It must keep Port Contract version tokens resolvable or refuse stale ones explicitly (ADR 0032).
- Existing-Member Documents metadata catch-up after app restart ([DELTA-011](.delta/DELTA-011-documents-offline-metadata-catchup.md)): diagnose the observed Collection/Document delay before proposing a fix. A later edit coincided with visibility, but no full-app cause or reproduction is established.
- A Conversations delivery qualification that meets `TEAM.SYS.CONV-R16`, including a 25-Member measurement (DELTA-008).
- Platform key custody: store a key-encryption key in the macOS Keychain once signed builds exist (ADR 0017 triggers). The Agent Package's `api-key` file, protected only by file mode `0600` today, joins this item.
- Agent Package: streaming replies and unprompted or scheduled posts. Generic tool use beyond declared Ports and runtime discovery are delivered by **Agent-controlled Package Ports**. Documents participation remains under **Documents Agent tools and Comments**.
- Network denial for Exhibit content frames: a navigation handler and WebRTC denial beside the content policy's `connect-src` ([DELTA-015](.delta/DELTA-015-exhibit-content-network-access.md), ADR 0034).
- Package platform authoring contracts: align the immutable-record-log 1.1 JSON request schema with the optional `principal` selector already supported by Host and SDK. See [DELTA-012](.delta/DELTA-012-record-log-principal-schema.md). This inherited mismatch is not resolved by the Agent integration.

## Not planned

- A `workspace-files` Package transported as Iroh blobs. ADR 0002 does not adopt Iroh blobs; a future document feature will choose its own mechanism.
- Commonware transport (rejected by ADR 0002).

## Planned follow-up workstreams

Identified on 2026-09-17 during Scope of the UI polish workstream. Each needs its own Scope.

### Member names

Incremental delivery on `feature/member-names`: Core-owned Workspace Person IDs and authenticated names, generic `teamarness.person-names` capability, Shell controls and Conversations author presentation. Merged into `main`; resolves `TEAM-DQ8` for one Installation per Person. Superseded in part by ADR 0023, which replaces the capability with `teamarness.principals` (see **Current work**). Multi-device grouping remains separate Phase 5 work.

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

Implementation in progress: the declaration, admission, scoped broker, and live event phases are committed on `package-interface`. The final source-built Conversations proof, local observations, documentation, and full verification are in progress. ADR 0026 records the Host-brokered interface.

A Package declares the exact commands, queries, and events it provides or consumes. The Host resolves one Provider per contract at admission and routes grants in the caller's Workspace. On-demand activation is opt-in; Conversations remains eager for background synchronization. The real Conversations contract shipped a Person-only post, read-only page, and live coalescible invalidation; the Host derived a Person actor only. The Agent Package workstream (ADR 0030, see **Current work**) then added the Host-derived Agent actor, Conversations 1.1 with Agent-authored posts and `channel.list`, and the Agent runtime. An in-app diagnostics view needs a separate Observability Scope; bounded local observations do not provide one. The Package Process major, UI Host version, and existing capability families do not change.

#### Data format versioning

An ADR that governs the stored-record seam separately from the live Host seam that ADR 0018 governs. Synced records are signed by peers that cannot be asked to stop, so an old reader must tolerate additive fields and degrade visibly, never reject or fork. Today the Conversations and Documents canonical codecs are positional and reject trailing bytes, and the pinned Loro CRDT will make its encoding part of the Documents format. Scheduled early because there is no install base yet, which makes the rule cheapest now.

#### WebAssembly Package class

Re-evaluate ADR 0006's deferral. Its blocking reason, Package-owned Tokio and Iroh work, was removed by ADR 0007 when Core took ownership of transport and replication, and WASI 0.3 with component-model async has since shipped. The open question is Package-local state without native SQLite or `gix`. Agent Packages holding an Agent key with the user's ambient authority (ADR 0023) are the motivating case for a stronger execution boundary. Outcome: an ADR introducing a second Package class, or a dated re-deferral with explicit criteria.

#### Package distribution and per-Instance restart

Author signature over the Package artifact, a team endorsement of an exact digest and capability set recorded in the membership log, a Host-owned update feed per Package (The Update Framework's threat list as the checklist, Packages never fetch their own updates), a consolidated installed-Packages view with permission diffs, and restarting one Package Instance without restarting the application. Depends on the code-signing work under Later. The Packagement workstream (see **Current work**) delivers live install, reload, and per-Instance restart for Personal Packages only; Team Package changes still apply at the next start.

### Workspace Principal maintenance (unscheduled)

Start a separate Scope if sustained name-delivery or membership churn becomes an ordinary workload. Two bounded deferred-name replay schedules can starve an eligible name while churn continues; finite backlogs converge. That Scope should also decide whether to use or retire the unused deferred-reference target index. A smaller Core maintenance change can reuse one clock sample for local Agent removal. Clarify the ontology's Agent Identity keypair wording without changing key custody. Diagnose the intermittent `names_converge_without_package_instance_after_restart` test failure before changing a deadline or claiming that a passing retry fixed it. The `agent-invite` final Verify report owns the evidence; these are follow-ups, not unverified passes or blockers to the completed supported flow.

### Documents Agent tools and Comments (follow-up)

Identified on 2026-10-25 when rendered Document editing closed. Documents Port Contract 1.1 already lets a bound Workspace Agent list, read, observe and revise Documents (ADR 0033). The Agent Package now supports generic tools. Documents 1.2 adds `runtime.guide` and `document.create` (ADR 0038), so it is eligible for Agent discovery; see **Agent Doc Exhibit** under **Current work**. The generic guidance direction is delivered by [ADR 0035](.decisions/0035-agent-controlled-port-discovery.md) and the completed alignment workstream above. [TEAM-DQ10](open-questions.md) retains these Documents follow-ups:

- Documents guidance and generic Agent tools can follow the ordinary `runtime.guide` convention; no Documents-specific Host adapter is planned.
- Agent document tools in the standalone Agent Package, built on that guidance. A tool reads before every revise and revises against the version it read (R12).
- Agent mentions and Comment Ports for Documents. The Documents Port Contract has no Comment operations today.
- A developer check once tools exist: an Agent revises a Document that a Member has open, and the change appears live.

### Documents editor maintenance (unscheduled)

Start a separate Scope if prioritized. These items were left open when rendered Document editing closed on 2026-10-25.

- **Accepted tech debt:** [DELTA-013](.delta/DELTA-013-documents-editor-accepted-tech-debt.md) records the editor's size-estimate and undo-budget gaps, pasted link addresses, the concurrent-open session leak, and the retained test and naming items.
- **Frame-policy divergences:** [DELTA-014](.delta/DELTA-014-package-ui-frame-form-and-select-behaviour.md) records the form-submit and `<select>` behaviour.
- **Manual checks not yet run:**
  - the packaged-app isolation probe (`npm run tauri build -- --bundles app,dmg`, then `npm run verify:packaged-app`);
  - IME input in WKWebView;
  - the two-second remote-edit latency observation (`TEAM.SYS.DOCS-R06`);
  - the large-paste and delete-all undo checks.
- **Robustness:**
  - CPU and SQLite work run on async workers, and import costs grow with records × body size (`CR-006`).
  - The hub-wide `identity_lookup_lock` is held across the Core append, which serialises local writes (`CR-007`).
  - The growth budget counts only text (`CR-009`).
  - A split paste continues after one piece is refused (`CR-103`).
  - Exact measurement runs on keystrokes in large lists (`CR-104`).
  - A revise merges before its cheap refusals while it holds the append lock (`CR-105`).
  - A recovered poisoned gate can leave a body missing a committed record without a diagnostic (`CR-106`).
  - A shared 256-slot session channel can cause a storm of snapshot reopens (`CR-204`).
  - An Agent's refusal before the append is reported as outcome-uncertain rather than certain (`CR-501`). A refused Agent write also leaves no diagnostic.
- **Contract and records:**
  - `document.revise` can return undeclared `documents.document_too_large` and retryable `documents.edit_failed` (`SF-102`).
  - `markdown-contract.md` omits the diff time budget (`SF-103`).
  - The system proof never reaches the Provider's `actor_unauthorized` (`SF-205`).
  - Conversations and Documents report Core's `principal_not_bound` in different shapes.
- **Maintainability:**
  - Cross-language bounds and the mark-expand configuration are hand-copied between TypeScript and Rust (`SM-003`, `SM-004`).
  - The frame-scenario harness re-implements the Backend session protocol, uses fixed sleeps, and is not linted or type-checked (`SM-005`–`SM-007`).
  - Loro parity is checked twice (`SM-008`).
  - `skipLibCheck` is undocumented (`SM-009`).
  - `backend.rs` holds the Editor Session seam without a module boundary (`SM-010`).
  - `adopt_import` copies fields by hand (`SM-103`).
  - There is duplication from corrective commits (`SM-105`).
  - The Agent revise path is a boolean flag inside the shared write path (`SM-503`).
- **Test gaps:**
  - the undo growth refusal (`SM-106`);
  - `Verifies:` citations in the TypeScript tests (`SM-206`);
  - an uncertain Agent append failure, and an Agent revision relayed to an open Editor Session (`SM-504`).

### Core test reliability (unscheduled)

Three Core tests failed or hung under machine load during rendered Document editing Verify, and passed when run alone. That branch changed no Core files. Diagnose before changing them:

- `runtime_tests::async_boundary::inbound_log_sync_receiver_calls_do_not_block_the_executor` hangs intermittently.
- `runtime_tests::sync_authorization::names_converge_without_package_instance_after_restart` fails with "connection lost".
- `sync_bounds::revocation_during_backoff_prevents_redial` fails once under load.

## Maintenance rule

Update this roadmap when a phase starts, closes, changes order, or changes direction. Update subsystem requirements and specs when behavior or architecture intent changes. Record hard-to-reverse trade-off decisions in `context/.decisions/`, unresolved design questions in `context/open-questions.md`, and known implementation divergence in `context/.delta/`.
