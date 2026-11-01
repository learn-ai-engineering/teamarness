# DELTA-021 — Backlog Package findings accepted as tech debt

**Status:** open (2026-11-01). Don accepted these dispositions when he closed the Backlog Package on 2026-11-01. They are deferred, not fixed. The owner of each group is the follow-up named in it.

## Divergence

The Backlog Package workstream (`TEAM.SYS.BACKLOG-R01`–`R10`, ADR 0039, Port Contract `dev.teamarness.backlog` 1.0) passed Verify with no P0 or P1 findings. Manual checks M1–M7 passed, except M5 part B, which could not run. The review findings below remain open. None of them breaks ordinary use. The finding IDs are those in the workstream's review artifacts.

### 1. Untested manual check: "Former member" assignee (AC12, `TEAM.SYS.BACKLOG-R06`)

- No manual check has seen a Ticket whose assignee left the Workspace. This build has no control to remove a Member (roadmap Phase 5, Membership Management), so manual step M5 part B could not run.
- The display is covered only by `packages/backlog-package/ui/src/detail.test.ts`.
- The former assignee's name also needs `teamarness.principals` 1.1. Until then the UI shows "Former member" and a 12-character ID.

Follow-up: Phase 5 Membership Management. Once a Member can be removed, assign a Ticket to them, remove them, and confirm that the Ticket shows "Former member" and can be reassigned.

### 2. Port Contract 1.0 text and shape (change before the first release, or in Backlog 1.1)

- **SF-002.** In `ticket.get` and `ticket.list`, `creator` and `lastEditor` hold signing keys, not Principal IDs as the plan's Ticket shape stated. An Agent cannot compare them with `assignee`, which is a Principal ID. The meaning is pinned in 1.0, and the guide states it. Fix: map the signer to its Principal ID, or add Principal ID fields in an additive minor version.
- **SM-004.** The `ticket.update` description in `packages/backlog-package/ports.json` says that a base *older* than the current version is refused. In fact any base that differs is refused. The error lists for `ticket.create` and `ticket.update` also omit `backlog.actor_unauthorized` and `backlog.unavailable`. Fix: correct the text, then update the fixture and `CONTRACT_1_0_SHA256`.
- **SF-003.** No Port test sends `title` or `priority` in a successful `ticket.update`. The shared update path is covered through the UI.

Follow-up: the next Backlog contract change, or before the first published release.

### 3. Capacity and convergence (Backend)

- **SF-004.** The `backlog.changes` snapshot sends every Epic and Ticket, archived ones included, in one event. The Host limits UI events to 256 KiB, so about 500 Tickets fit in total. Nothing is ever deleted. When a Backlog grows past the limit, the tree stops updating ("Backlog updates stopped"). Fix: chunk the snapshot, or omit archived Tickets unless Show archived is on.
- **CR-001.** After a save, the returned `version` is read again after the append. A remote record for the same Ticket that lands in that window can be included in it. The next save is then accepted over a change the saver never read. This is a rare race. Fix: return the appended record's own OrderKey.
- **CR-002.** If an ingest or reapply fails in storage, the rollback reloads Epics and Tickets but drops the in-memory set of records waiting for their target. Those records apply only at the next activation. Fix: carry the waiting set over on rollback, as titles and bodies are.
- **CR-003.** The per-Ticket lock map adds an entry for every well-formed Ticket ID, including unknown ones, and never removes it. Fix: remove an entry when no one holds it.
- **SM-011.** `epic_archived` copies every Epic to read one flag. Fix: add `BacklogStore::epic(id)`.
- **SM-012.** `ERROR_DETAILS_LIMIT` restates the Host's 4,096-byte limit without a test that pins the two together.

Follow-up: the next Backlog workstream. Do SF-004 before a team's Backlog reaches a few hundred Tickets.

### 4. UI

- **CR-004 / SM-002.** Every `backlog.invalid` refusal shows the title message, even when the body is the problem (for example, a pasted control character). Fix: choose the message from `details.field`.
- **CR-005.** A draft body over 65,536 characters makes every `backlog.draft.put` fail. While that draft exists, none of the remembered UI state is saved. Fix: drop or cut over-limit bodies in `boundState`.
- **SF-001 / SM-006.** The New Ticket draft (`new:<epicId>`) is not remembered across a Package switch, although the contract allows it. The contract also carries an `editing` flag that is never read and an empty `baseVersion` value that is never used. Fix: build the feature, or remove the unused contract surface.
- **SM-005.** The `backlog.draft.*` requests carry the whole remembered UI state, and the ontology already defines Draft for Exhibits. Fix: rename them `backlog.remembered.*`.
- **SM-101.** The inline-form typing test does not prove that the caret is restored, because happy-dom keeps the selection when an input is detached and re-attached.
- **SM-009.** `remembered.test.ts` waits 350 ms of real time twice. Use fake timers.
- **SM-008.** `styles.css` refers to `--teamarness-color-warning`, which the SDK does not define.
- **SM-010.** `KIT`, `button` and the SVG icon helpers are repeated in `view.ts`, `detail.ts` and `listbox.ts`. The store's rollback block is repeated in `ingest` and `reapply`.

Follow-up: the next Backlog workstream.

### 5. Documentation and code hygiene

- **SM-001.** `TEAM.SYS.BACKLOG-R01` and `R05` do not state that a title cannot start or end with whitespace, although the code, the UI contract and the guide enforce it.
- **SM-003.** The Backend's `lib.rs` exports more than any caller uses. `BacklogActive::store()` has no caller. `BacklogStore::waiting()` and `KINDS` are used only by tests, and the doc comment on `KINDS` describes restart behavior that the code does not use.
- **SM-007.** Two UI comments say "Member" where they mean the Person using the UI. A test fixture comment refers to "Phase 2" of the workstream plan.

Follow-up: the next Backlog workstream.
