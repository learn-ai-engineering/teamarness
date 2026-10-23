# 0025 — A durable body edit may be unapplied on one Installation

**Status:** accepted (2026-10-23)
**Refines:** ADR 0023 (Documents body import and invalid-history presentation), ADR 0024 (Package checks after qualified local replay).

## Context

An immutable-record append confirms durable, signed bytes in the local Core log. It does not confirm that every Installation has opened the record or imported its Loro update. A missing causal predecessor is a normal Loro `ImportStatus.pending` result; an actual import error may instead reflect invalid or incompatible bytes. Domain validation of the signed edit envelope does not establish that its Loro update can be applied. The Package stores sealed records and opaque metadata, not plaintext bodies.

## Decision

Documents distinguishes **record durability** from **body application** at each Installation. A successful import with missing causal history keeps the existing pending-history state and may resume when earlier updates arrive. An actual Loro import error is never silently treated as an applied edit, as proof of inclusion in visible text, or as a Saved cue. On the Installation that encounters it, retain the signed sealed record and an opaque, durable Document-scoped `import_failure` disposition. Keep the last known-good text visible but explicitly incomplete, show a prominent read-only warning with a diagnostic reference, and retain unsaved in-memory draft state without promising crash survival. Do not apply later body updates for that Document until deliberate reimport or repair. Other Documents remain editable and their record delivery continues. A qualified Core replay boundary does not override failed Package import.

Persist the failure disposition with the sealed row and delivery cursor atomically. On restart or migration, detect it before presenting a Document as complete. Recovery reports affected request inclusion unresolved even when the sealed record itself is durable; the editor must not clear an outstanding edit solely because durable bytes exist. Local diagnostics include bounded identifiers, payload lengths, phase, and a sanitized error category, but never plaintext text, raw update or sealed bytes, payload digest, keys, or unbounded error text. This is local detection, not an all-peer acknowledgment or automatic repair protocol. `identity_conflict` remains a different state: when two signed identities compete, its accepted policy hides the body; `import_failure` may show the last known-good text marked incomplete.

A gap-free Core replay marker is distinct from Document readiness. An unopened or failed Document cannot certify its own missing request IDs absent, but it does not block another Document whose own opened/indexed history and local Core prefix are qualified. An eventual cross-Document ID collision takes the accepted conflict path, never a silent first-winner decision.

## Rejected alternatives

- **Skip the failing update and continue editing without proof.** Later updates may depend on it, and installations on different Loro versions may disagree about importability.
- **Treat every error as temporary missing history.** Loro reports missing dependencies separately; decode and unsupported-version errors need a visible response.
- **Log and continue reporting included/durable.** A user can see Saved while the text change never appeared.
- **Stop the entire Package stream or require every peer to acknowledge import.** The former blocks unrelated Documents; the latter adds a new synchronization protocol outside this decision.
- **Hide the last good text as an identity conflict.** No competing applied winner is established by a failed import. Visible, labeled incomplete text aids diagnosis without claiming completeness.

## Consequences

- Affected Document editing can pause on one Installation until an explicit future repair or compatible reimport path is built. No automatic deletion, skip, rollback or synthetic text/comment insertion is authorized. Other Documents continue.
- A sender's local Saved cue means its Core accepted the record; it does not claim remote import success. A receiver that cannot import exposes the local failure. Cross-peer notification is a separate future decision.
- The Package needs an opaque failure marker, a distinct bounded UI reason, structured privacy-safe diagnostics, and live/deferred/restart regressions. The pending-history path for valid but causally incomplete updates remains unchanged.

## Evidence

- `packages/documents-package/backend/src/body.rs` — Loro import distinguishes `Ok(status.pending)` from `Err` and keeps the body in memory.
- `packages/documents-package/backend/src/domain.rs` — signed edit envelope validation bounds bytes without parsing the Loro payload.
- `packages/documents-package/backend/src/backend.rs` and `src/store/mod.rs` — local append, sink projection, cursor/index, recovery and activation seams.
- `packages/documents-package/ui/src/editor.ts` and `ui-contract.json` — local request and text/recovery status interface.
- `context/02-system/09-documents/requirements.md` — valid-edit convergence, separate inclusion/durability, Document-local liveness.
- **Weaknesses.** Source tracing and deliberately invalid test bytes establish a possible failure path, not its frequency in production. No remote import receipt exists, and this decision does not add one. The corrective implementation and developer two-profile observations are still to be verified.
