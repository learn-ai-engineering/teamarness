# 0039 — Backlog Tickets converge per changed field; stale saves are refused against the local projection only

**Status:** accepted (2026-11-01)
**Refines:** ADR 0027 (adopts its last-writer-wins-by-OrderKey rule for metadata records and its no-plaintext-at-rest rule), ADR 0030 (adopts its Provider pattern for Agent-signed Backlog records)

## Context

The Backlog Package (`dev.teamarness.backlog`) holds Epics and Tickets on the immutable record log and replicates them to every Member. A Ticket's Markdown body is edited by one Person, or by one Person working with their Agent. It is not edited collaboratively. A Workspace Agent also edits Ticket title, body, status and priority through the Backlog Port Contract.

The developer requires that a save based on an out-of-date Ticket is refused rather than silently overwriting newer work. The record log has no global compare-and-set. Documents handles stale bases by merging them (`document.revise`), not by refusing them. Exhibits refuses with `expectedRevision`, but its Drafts are local and never replicated.

## Decision

1. **Changed-field updates.** A `ticket.update` record carries only the fields the saver changed, from title, body, status and priority. Assignment, move, archive and restore are separate record kinds.
2. **Per-field convergence.** Each field keeps the value from the applied record with the greatest OrderKey that set it. Arrival order does not matter, and every Installation converges. Concurrent saves to different fields are all kept.
3. **Version.** A Ticket's version is the greatest OrderKey among the records applied to it.
4. **Local stale refusal.** Every Ticket-changing request carries `baseVersion`. This applies to Persons through the UI and to Agents through the Port. The Backend holds a per-Ticket lock. It refuses with `backlog.stale_version` and returns the current Ticket when the base differs from the version this Installation has applied. Otherwise it appends.
5. **What is not refused.** Saves made concurrently on Installations that have not yet seen each other's record are not detected. They converge under rule 2, so the later change to the same field wins.
6. **No plaintext at rest.** Ticket titles, bodies and unsaved drafts live only in Backend memory. Titles and bodies are rebuilt from sealed records on activation. Drafts are lost on restart.

## Rejected alternatives

- **One whole-Ticket record per save.** It overwrites fields the saver did not touch.
- **One record kind per field.** One save would become several records that can fail independently, and the save would no longer have one version.
- **Merging a stale body three ways, as Documents does.** It adds merge machinery for a field that has a single editor. The developer chose refusal.
- **Detecting conflicts across Installations.** Each record would carry its base, and the projection would flag divergence. Nothing in the codebase does this today. It is deferred until concurrent offline edits prove to be a problem.

## Consequences

- An Agent and a Person working on the same Installation cannot silently overwrite each other. The later save is refused and retried against the current Ticket.
- Two Members editing the same field while offline from each other can lose one change, with the later change in log order winning. Records are immutable, so the earlier change remains in the log.
- The record format is permanent once shipped. A later conflict-detection scheme must be added as new kinds or fields.
- Epic rename and archive do not check a version. Last change wins.

## Evidence

- `packages/documents-package/backend/src/store/projection.rs` and `store/mod.rs`: the per-field `*_order` last-writer-wins pattern.
- `packages/documents-package/backend/src/ports.rs`: `document.revise` merges against `baseVersion`.
- `packages/exhibits-package/ports.json`: `draft.update` uses `expectedRevision` and returns `exhibits.revision_conflict` for local Drafts.
- `contracts/package-capabilities/v1/immutable-record.golden.json`: OrderKey is a total, byte-sortable causal order.
