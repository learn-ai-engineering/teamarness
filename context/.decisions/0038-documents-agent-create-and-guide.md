# 0038 — A bound Workspace Agent creates Documents, and Documents publishes a guide

**Status:** accepted (2026-10-31)
**Supersedes in part:** ADR 0033 (§1, the sentence "No other Documents command takes an Agent actor.")
**Refines:** ADR 0030 (adopts its Provider pattern for `document.create`), ADR 0035 (Documents becomes an agent-facing contract)

## Context

The Documents Port Contract is the surface through which other Packages, and the Agents they act for, read and revise Documents as Canonical Markdown. Version 1.1 accepts a Host-derived Agent actor on `document.revise` only (ADR 0033). A Document can be created only through the Documents UI, as the Installation Person.

A `document.create` record carries the title and the Body Genesis, which is always one empty paragraph (`TEAM.SYS.DOCS-R05`). The Document ID is the create record's ID. Its version token is the genesis frontiers, encoded as `document.revise` expects a base version.

Under ADR 0035, an Agent harness discovers a contract only when it has a `runtime.guide`: a query with a closed empty request whose response is bounded Markdown. Documents has no guide, so a generic Agent harness cannot discover or use it.

## Decision

1. **Documents Port Contract 1.2 adds `document.create`.** It is additive over 1.1. The request is the closed `{collectionId, title}`, and the title follows the existing rules: 1 to 256 characters, no control characters, and no leading or trailing whitespace. The response is `{documentId, version}`, where `version` is the token of the Body Genesis. The command creates an empty Document; a Consumer adds content with `document.revise`, using the returned `version` as its `baseVersion`.

2. **`document.create` accepts the Agent actor, as `document.revise` does.** A Person actor must be the local Installation Person; any other Person is refused with `documents.actor_unauthorized`. An Agent actor is Host-derived and accepted as given: the `document.create` record names the Agent's signing key as its author, and the Package appends it with the Agent's Principal ID through record-log 1.1. A `principal_not_bound` refusal is reported as `documents.principal_not_bound`, non-retryable and certain. The private UI create path is unchanged.

3. **Refusals before the append are certain.** An unknown Collection gives `documents.unknown_collection`, an archived Collection gives the new `documents.collection_archived`, and an invalid title gives `documents.invalid_title`. None appends a record. Any other failure after the append is requested is outcome-uncertain and is not retried automatically. A create is not repeatable: each accepted create makes a new Document, so a Consumer lists and reconciles before it tries again.

4. **Documents 1.2 publishes `runtime.guide`.** It is an ordinary query with a closed `{}` request and a closed `{markdown}` response of at most 32 768 characters. Its text is built from the constants the Backend enforces. It covers discovering and reading Documents, creating then revising, revising against a version, Host-derived authorship, and errors and repeatability. With a conforming guide, Documents is an agent-facing contract: a bound Agent uses it only after its Steward enables it (ADR 0035).

## Rejected alternatives

- **Create with initial Markdown.** The body would need a create record and an edit record. A failure between them leaves an empty Document with an uncertain second step, and one command would no longer be one record.
- **Creating in an archived Collection, as the UI does.** An Agent cannot see that a Collection is archived from the place it works, so a Document it creates there would be hidden from the Members it is working for.
- **A Documents-specific Agent adapter.** ADR 0035 makes `runtime.guide` the generic way an Agent harness learns a contract.

## Consequences

- Agent-created Documents are ordinary `document.create` records. Receiving Installations validate and project them without change, because the record's declared author equals its signer.
- A bound Agent that has Documents 1.1 access without the guide now needs its Steward to enable Documents. A declared Consumer acting as a Person is unaffected.
- A create followed by a revise is two commands. A revise straight after a create works locally once the Document's body is available. Peers receive the create record before any edit; offline metadata catch-up for Agent-created Documents is as for any other (DELTA-011).
- Comment Ports and Agent mentions in Comments remain separate work.

## Evidence

- `packages/documents-package/ports.json` (1.2), `backend/tests/fixtures/ports-1.0.json` and `backend/tests/fixtures/ports-1.1.json`, with the additive-minor tests in `backend/src/backend.rs`.
- `packages/documents-package/backend/src/ports.rs` (`authorize_actor`, `create`), `backend/src/backend.rs` (`append_as`) and `backend/src/guide.rs`; backend tests for the Person and Agent creates, the certain refusals, create then revise, and the guide's validity and size.
- `tests/system/tests/documents_ports_proof.rs`: a Consumer bound to a Workspace Agent reads the guide, creates a Document and revises it through the Host broker, and Core holds both records signed with the Agent's key.
- **Weaknesses.** As in ADR 0033, the `principal_not_bound` path is exercised with a scripted record log, because the Host derives an Agent actor only while the binding exists.

### Provenance (non-versioned)

Workstream `agent-doc-exhibit`: research `research/20261031T043000Z_agent-doc-exhibit.md` and plan `plan/20261031T050000Z_agent-doc-exhibit.md`.
