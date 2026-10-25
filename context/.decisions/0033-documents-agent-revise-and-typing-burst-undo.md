# 0033 — A bound Workspace Agent revises Documents, and typing undoes in bursts

**Status:** accepted (2026-10-25)
**Supersedes in part:** ADR 0032 (Refines line, "a second Person-only Port Contract"; **Documents Port Contract 1.0**, "a Person-only command" and the sentences "Authority follows Conversations 1.0 (ADR 0026): the Host-derived Installation Person is checked, and Core signs. Agent-signed revisions wait for the Agent authority decision."; **Sessions**, "Each local change is its own undo step (no merge interval), except that a split paste is one step"; Consequences, "Undo works per change, not per burst of typing")
**Refines:** ADR 0030 (adopts its Provider pattern for Documents)

## Context

The Documents Port Contract is the surface through which other Packages, and the Agents they act for, read and revise Documents as Canonical Markdown. ADR 0030 makes a Workspace Agent bound on this Installation a local signer: the Host derives an Agent actor for a bound caller, a Provider that accepts it appends with the Agent's Principal ID, and Core signs with the Agent's key or refuses with `principal_not_bound`. Later Providers adopt that pattern without a new authority decision.

A Documents revise is validated against the body before it is written. The local write path stages a validated update, shows it in the body at once, and retries its append in the background until Core accepts it, always signed by the Installation. A staged update cannot be withdrawn from the body.

Per-person undo in the rendered editor groups consecutive typing within one textblock into one step, as `TEAM.SYS.DOCS-R13` specifies.

## Decision

1. **Documents Port Contract 1.1 accepts the Agent actor on `document.revise`.** It is additive over 1.0. A Person actor must be the local Installation Person; any other Person is refused with `documents.actor_unauthorized`. An Agent actor is Host-derived and accepted as given: the `document.edit` record names the Agent's signing key as its author, and the Package appends it with the Agent's Principal ID through record-log 1.1. Queries and the event accept any Host-derived actor. No other Documents command takes an Agent actor.

2. **An Agent's revision is appended before the body adopts it.** The merge is validated on a fork exactly as a Person's revise is, but nothing is staged. The Package appends the one record while it holds the Document's append lock, and the body adopts the revision only from that appended record. A `principal_not_bound` refusal is reported as the declared, non-retryable `documents.principal_not_bound`, with certainty, and leaves the body unchanged. Any other failure after the append was requested is outcome-uncertain and is not retried. The Installation Person's revise keeps the staged write path.

3. **Typing undoes in bursts.** Consecutive plain-text insertion, backspace and delete within one textblock is one undo step. The step ends at an idle gap of 750 ms, at any other change, at a selection jump, at an undo or redo, and before an import of another peer's changes. Each other local change is its own step, except that a split paste is one step, and a burst grows only while its undo still fits one record (`TEAM.SYS.DOCS-R13`).

## Rejected alternatives

- **Staging an Agent's revision like a Person's.** The background retry signs as the Installation, and a refused Agent write could not be withdrawn from the body, so it would block the Document's later local writes.
- **A per-revise check that the Agent is bound.** Documents holds no Agent Binding, and the check would race the append that Core decides anyway.
- **Keeping each local change as its own undo step.** Undoing a sentence one character at a time is not usable.

## Consequences

- Agent-authored body edits are ordinary `document.edit` records. Receiving Installations validate and project them without change: the record's declared author equals its signer, and body edits show no author.
- An Agent's revision reaches other open Editor Sessions as a remote change, so it is never in a Member's undo history.
- The Agent Package does not yet call the Documents Ports. Agent tools, Agent mentions in Comments, and Comment ports are separate work.

## Evidence

- `packages/documents-package/ports.json` (1.1) and `backend/tests/fixtures/ports-1.0.json`, with the additive-minor test in `backend/src/backend.rs`.
- `packages/documents-package/backend/src/ports.rs` (`authorize_actor`, `revise`) and `backend/src/backend.rs` (`stage_local_write` `append_first`, `append_identified_edit`); backend tests for the Agent revise, its replay on a second Installation, and the `principal_not_bound` refusal.
- `tests/system/tests/documents_ports_proof.rs`: a Consumer bound through `bind_agent_package` revises through the Host broker, and Core holds the record signed with the Agent's key.
- `packages/documents-package/ui/src/editor/undo.ts` and `undo.test.ts`, and the undo frame scenario (`npm run verify:documents-editor`), for typing bursts.
- **Weaknesses.** The second-Installation check replays the appended records into a backend test double, not a second Core. The `principal_not_bound` path is exercised with a scripted record log, because the Host derives an Agent actor only while the binding exists.
