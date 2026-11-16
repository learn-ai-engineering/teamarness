# 0043 — Document folders are replicated paths with independent placement

**Status:** accepted (2026-11-16)
**Refines:** ADR 0027 (Document metadata), ADR 0038 (optional folder on create), ADR 0042 (format generation 2)

## Context

A Collection can map to a flat folder under ADR 0041. Teams also need nested planning documents. Replicated Folder entities would require identities and lifecycle rules for empty folders, although the needed behavior is placement of Documents.

Metadata records with identical bytes have the same Record ID. Encoding only author, Document, and folder would prevent a later return to a folder from expressing a new move. Existing mappings also ignore subfolders: silently making those mappings recursive could share files their owner never intended to publish.

## Decision

**Placement.** Kind 12, `document.place`, carries author, Document ID, canonical folder, and a 128-bit operation identity. Its canonical layout is `TDOCS\x01`, kind byte 12, length-prefixed UTF-8 author, Document ID and folder, then 16 operation bytes. Earlier record encodings do not change. Each intentional placement gets a fresh identity; retry of an uncertain filesystem operation must reuse its original identity.

The projection uses its own greatest Order Key. Placement waits for its Document and is independent of Collection, title, archive state, and body edits. Collection moves preserve the folder. Root is `""`; folders exist through their Documents, not as entities.

**Paths.** Local ingress normalizes to NFC. Canonical records must already be NFC. Paths are slash-separated, at most eight segments and 512 UTF-8 bytes. Empty segments, dot and dot-dot segments, absolute paths (including drive-rooted paths), backslashes, and control characters are refused. Hidden paths remain valid metadata, but recursive mapping must refuse paths its scan policy or filesystem cannot represent rather than silently sanitize them. Actual disk spelling remains separate from logical folder paths.

**Caller contracts.** Snapshots and Port 1.4 listing include `folder`. UI request `documents.document.place` changes replicated placement; `documents.place.get/set` still holds only local navigation. UI and Port create accept an optional folder, validated before append. Root create remains one record. Non-root create appends create then place with the same authorized Person or Host-derived Agent. Port create returns the genesis version for revise, not a metadata version. Any failure after create is outcome-uncertain, even a certain refusal of the placement append. Neither caller automatically retries the whole create. No Agent move Port is added.

**Persistence and upgrade.** Forward-only migration 0011 adds `documents.folder`, `documents.place_order`, and `folder_mappings.recursive` with default false. State updates preserve the local mode. Format generation rises from 1 to 2. ADR 0042 re-evaluation recovers genuine retained kind-12 rows without redelivery; unopenable history keeps the stored generation retryable. Existing body bytes and file associations are unchanged. Older builds cannot open an upgraded local database. Older peers on their own databases quarantine placement until upgrade, while ordinary body edits remain compatible.

**Consent and recursive safety.** The accepted direction for later slice C phases is an in-place preview and confirmation before an existing mapping enables subfolders on that Installation. New mappings become recursive through their preview-confirm flow. Do not unmap/remap or clear durable bases. Before consent, tracked root files retain their last paths and body associations; new foldered Documents must not flatten into root files. A missing or unreadable root archives nothing. Preserve the existing guard when the active scan has zero eligible readable Markdown files, including deletion of its only populated subtree. Cleanup removes only empty directories Teamarness's own operation emptied, never recursively and never the root.

## Alternatives

- **Folder entities:** unnecessary identities, conflicts, and empty-folder lifecycle.
- **Extend `document.move`:** changes an existing record's encoding and semantics.
- **Deterministic placement without operation identity:** cannot express repeated returns.
- **Automatically enable existing mappings:** may share previously ignored private files.
- **Unmap and remap to enable recursion:** discards durable file state and conflicts with first-mapping rules.

## Consequences and evidence

Phase 1 implements metadata, migration, upgrade recovery, and caller contracts only. Recursive scanning, consent settings, folder controls, and two-Installation qualification remain implementation work. R14 and ADR 0041 still describe the flat worker until those phases ship.

Tests in `backend/src/domain.rs`, `lib.rs`, `store/mappings.rs`, and `backend.rs` cover canonical bounds, normalization, arrival order, repeated placement, restart, v10 migration, retained-kind recovery, actor fidelity, root-only compatibility, second-append uncertainty, listing invalidation, and bounded Port payloads. The frozen `backend/tests/fixtures/ports-1.3.json` and contract comparison permit only the declared 1.4 schema additions. No Core, Host, Shell, SDK, protocol, or dependency changes are needed.

A crash between create and place can leave an empty root Document. It requires listing and reconciliation, not automatic create retry. Hash-only disk move detection will not promise identity for a move combined with changed content. Accepted repeat-archive and filesystem race limits remain in DELTA-023 group 11. Don's folder UI and real-editor checks belong to final Verify; automated metadata tests do not establish those observations.
