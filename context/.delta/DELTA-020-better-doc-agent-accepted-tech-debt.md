# DELTA-020 — Better Doc Agent findings accepted as tech debt

**Status:** open (2026-11-01). Don accepted these dispositions when he closed Better Doc Agent on 2026-11-01. They are deferred, not fixed. Owner: the follow-up named in each group.

## Divergence

The Better Doc Agent workstream (Documents Port Contract 1.3, `TEAM.SYS.DOCS-R07`/`R12`, and the standalone Agent Package's Comment source) passed Verify with no P0 or P1 findings. The review findings below are still open, and none of them breaks ordinary use. The finding IDs are those in the workstream's review artifact.

### 1. Agent Comment source (Agent Package)

- **CR-001.** `harness::start` runs the Conversations and Comment sources under one `tokio::join!`. The harness task finishes only when both sources end. A source that exits on its own, for example on a fatal `invalid_progress`, is therefore not replaced while the other keeps running. Fix: keep one task handle per source, or cancel the sibling when one source returns.
- **CR-002.** Each source sets the shared status to Idle by checking only its own replies. A Posted result from one source can also clear the other's error. This affects the status display only. Fix: count in-flight replies across both sources.
- **CR-003.** One Workspace-wide cursor serializes Comment mentions. If a second mention arrives on a Document whose reply is still running, the scan stops there. Mentions on other Documents then wait until that reply ends. Nothing is lost or repeated. A full fix needs a persisted queue of deferred triggers.
- **CR-004.** A Comment add whose text never opens (`WrongScope` or `Corrupt`) stays `pending` in `comment.changes`. The Agent retries that revision forever, so every later Comment waits behind it. Fixes: the Agent can skip pending events that are not adds, and Documents can add an `unavailable` status that Consumers skip, as Conversations does.
- **CR-005.** The Comment feed gives a new revision to every Comment it projects, including during catch-up sync. Older Person mentions can therefore be answered in two cases: when the Agent baselines before Documents finishes its first sync, or after the Documents database is recreated with a new epoch. Fix: ignore adds whose `createdAt` is earlier than the baseline time. This is a rare race.
- **SM-001.** `comments.rs` repeats the subscription loop of `harness.rs`: the `Loop` struct, `schedule`, `shutdown` and the reconnect handling. Fix: extract one generic source loop.
- **SM-008.** The Comment cursor is stored in the `channels` progress map under the sentinel key `dev.teamarness.documents#comments`. Document the key namespace on `Progress`.
- **SM-009** and **SF-003.** No test runs both sources at once, and the test fake that serves Documents is still named `FakeConversations`. No test covers the fallback to the thread root's quote.

Follow-up: the next Agent Package workstream, or the Agent-to-Agent Scope, which changes the trigger rules anyway.

### 2. Documents (Host)

- **SF-001 / SM-004.** `TEAM.SYS.DOCS-R12` requires a system test that drives every Port through the Host broker. `tests/system/tests/documents_ports_proof.rs` lists only the four 1.3 Comment Ports, and real Core is not shown signing an Agent's `comment.add`. Backend tests cover the behavior.
- **SM-005.** The `runtime.guide` Port description in `packages/documents-package/ports.json` still describes 1.2. Changing it changes the published contract text, so it waits for the next Documents contract change.
- **SM-007.** Six test sites repeat the SQL that simulates a downgrade. Add one `downgrade_to_vN` helper.
- **SF-004.** A hovered Document row highlights the whole row, but the selected Document highlights only its title. Don accepted the look at the manual check.

Follow-up: Host Port maintenance on `main`.

### 3. Markdown copies (Conversations and Agent Packages)

- **CR-006.** `@types/dompurify` is a deprecated stub, because dompurify ships its own types. Remove it from both Packages together.
- Column alignment in Markdown tables is dropped, because the sanitizer forbids `style` attributes. Reconsider this if Backlog ticket bodies need aligned columns.
- Under happy-dom, the sanitizer drops a link's `rel` attribute, so no automated test checks it. Confirm it in real WebKit when either copy next changes.

Follow-up: the next change to either Markdown copy.
