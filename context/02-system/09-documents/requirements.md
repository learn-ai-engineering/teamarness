# 02-system/09-documents — Documents Requirements

Prefix: `TEAM.SYS.DOCS`

The Documents Package (`dev.teamarness.documents`) provides Workspace Collections, Documents, and Comments. See ADR 0027 and ADR 0031.

## TEAM.SYS.DOCS-R01 — Package Boundary

The Documents Package SHALL own Collections, Documents, Comments, their canonical record encoding and validation, local projections, and its UI. It SHALL use only the standard Package Manifest, Package Process Protocol, lifecycle, Package Data Directory, UI contract, and declared Semantic Capabilities. The Core Runtime, Package Host, and Shell SHALL contain no Documents-specific behavior.

## TEAM.SYS.DOCS-R02 — Record Replication

The Documents Package SHALL replicate every Collection, Document, Comment, and body edit record through one `teamarness.synchronization.immutable-record-log` 1.x scope per Package and Workspace. Documents SHALL require the additive 1.1 replay-boundary minor; 1.0 clients SHALL keep their existing stream behavior. The Core Runtime SHALL treat Documents canonical bytes as opaque.

**Verification gap:** An existing Member's app-closed Collection and Document catch-up is unverified. See [DELTA-011](../../.delta/DELTA-011-documents-offline-metadata-catchup.md); this note does not weaken R02.

## TEAM.SYS.DOCS-R03 — Identity and Convergence

A Collection, Document, or Comment SHALL be identified by the Record ID of the record that created it. Rename, move, archive, restore, resolve, and reopen SHALL converge by Order Key, so every Installation shows the same state regardless of arrival order. A record whose target has not arrived SHALL be retained and applied when the target arrives.

## TEAM.SYS.DOCS-R04 — Collections and Documents

Any current Member SHALL be able to create a Collection with a name of 1 to 128 characters and a Document in a Collection with a title of 1 to 256 characters, and to rename either, move a Document between Collections, and archive or restore either. Archiving SHALL hide, never delete; archived entries SHALL remain openable and restorable.

## TEAM.SYS.DOCS-R05 — Collaborative Body

A Document body SHALL be a Loro tree of the shared body schema (ADR 0031), with Canonical Markdown as its interchange. Its first update SHALL be a body genesis, carried in the `document.create` record: one update from a fresh peer holding the root map, its attributes, the `children` list and one empty paragraph. A create record without a genesis, or with anything other than exactly a genesis, SHALL be rejected at decode, and every Installation SHALL import the genesis before any edit, so no editor initialises an empty body. Every Loro peer SHALL be a random 64-bit identity that the Backend allocates for one genesis, Markdown merge or editor session; it SHALL be absent from the Document's version vector and from the peers already allocated in that process, and SHALL NOT be derived from the Installation. Concurrent edits, including edits made while disconnected, SHALL converge to identical content on every Installation with no edit lost. Each accepted local update SHALL produce exactly one identified (kind-11) sealed body edit record; the retired plain-text kind 10 SHALL be rejected. Before append, a local update SHALL be imported into a fork and refused, with a typed error and nothing appended, unless it holds operations only from its session's peer, fits one record, leaves a tree valid under the shared schema, and respects the growth budget. A Markdown revision SHALL be merged from its base version through the same validation as one update from a fresh peer; one that does not fit one record SHALL be refused with nothing staged. A body SHALL be rebuilt from its genesis and edit records at activation and SHALL NOT be persisted as plaintext. A Document whose edit records cannot yet be opened SHALL be presented as pending rather than partially. Recovery SHALL report inclusion and durability separately for a bounded set of editor request IDs captured with one text snapshot; missing IDs remain unresolved until a gap-free, from-start local replay boundary and all relevant sealed edits have opened. Neither synchronization health nor a saved cursor proves absence. Package SQLite schema v6 SHALL index each authenticated kind-11 high-entropy request ID to its Record ID, Document and author without storing a plaintext payload digest or body. The index SHALL be rebuilt from opened applied and deferred sealed records before bodies are exposed after v4 migration. Distinct authenticated records sharing an edit request ID SHALL persist a Document-scoped `identity_conflict` marker for every affected Document, regardless of arrival order. Affected bodies SHALL hide text, reject edits and absence proofs, report unresolved recovery, and remain read-only across restart; later edits to them SHALL remain sealed/unapplied. Other Documents' edits and delivery SHALL proceed. No first-seen record is a winner or automatically repaired; sealed history remains for a future explicit repair. A local conflicting retry SHALL be rejected before append without quarantining the Document. The bounded in-memory identity cache SHALL not determine provenance after eviction. A gap-free Core replay boundary SHALL be distinct from the affected Document's opened/indexed history: unopened A does not prevent a qualified local edit on healthy B. A signed body edit whose Loro import returns an error, or whose import leaves a tree the shared schema does not allow (sanitized category `schema_violation`), SHALL retain its sealed row, opaque ID and cursor with an atomic Document-local import-failure marker; it SHALL NOT be counted as applied, included or Saved on that Installation. The last known-good text SHALL remain visible marked incomplete/read-only with a bounded local diagnostic reference; later edits on that Document remain sealed/unapplied while unrelated Documents continue. Missing causal history reported as a successful pending import SHALL remain pending rather than failed. This is not an all-peer import acknowledgment or automatic repair.

## TEAM.SYS.DOCS-R06 — Edit Latency

With both Installations online, a body edit SHALL be visible to another Member within two seconds (vision success criterion 4). Verification is manual across separate networks.

## TEAM.SYS.DOCS-R07 — Comments

A current Member SHALL be able to add a Comment of 1 byte to 32 KiB of Markdown to a Document, optionally quoting up to 2 KiB of excerpt and optionally replying to a parent Comment on the same Document, and to resolve or reopen any Comment. The signed add record SHALL carry an author-clock creation timestamp for display (not ordering); old records without it SHALL show “Time unavailable.” Comments SHALL be retained with the Document and never deleted. A reply to a Comment on another Document SHALL be rejected.

## TEAM.SYS.DOCS-R08 — Authorship Presentation

Records SHALL store author signing identities, never names. The UI SHALL label authors by current Workspace Principal Name through `teamarness.principals`, update labels after rename, and fall back to an inspectable signing identity. Workspace Person and Workspace Agent authors SHALL use the same lookup.

## TEAM.SYS.DOCS-R09 — Bounds

One body record SHALL carry at most 160 KiB of Loro update, so its base64 form fits one 256 KiB UI message. Local edits SHALL have a 192 KiB growth budget, measured as the UTF-8 text content of the body; valid concurrent remote edits beyond this size SHALL remain convergent and visible through bounded full-text chunks. While over budget, local edits SHALL not grow the body further and shrinking edits SHALL remain possible, including after a rejected growth request. Unknown append outcomes SHALL preserve exact live retry bytes and an unsaved cue rather than guessing inclusion after restart. Comments SHALL be retrievable through serialized-byte-bounded Order Key cursor pages, including older Comments; no single response SHALL exceed the 256 KiB bridge bound.

## TEAM.SYS.DOCS-R10 — Non-Goals of Version 1

Folder mapping, rich text, document-level permissions beyond Canonical Membership, log compaction, and a Core document capability family are not delivered. A `document.snapshot` record kind is reserved for compaction.

## TEAM.SYS.DOCS-R12 — Documents Port Contract

The Documents Package SHALL provide Port Contract `dev.teamarness.documents` 1.0 (`ports.json`, referenced from its manifest) with four Ports:

- `document.list` (query) SHALL return at most 64 Collections and Documents per page in Order Key order, optionally for one Collection, with each Document's Collection, title, archive state, body availability (`available`, `pending` or `unavailable`) and, for an available body, its version token, plus an Order Key cursor for the next page.
- `document.markdown` (query) SHALL return a Document body as Canonical Markdown with the version token it was exported at, or the typed error `pending` (records not yet opened), `unavailable` (identity conflict, import failure, or a version the token cannot carry) or `too_large` (the response exceeds the Host Port value bound, `MAX_UI_PAYLOAD_BYTES`).
- `document.revise` (command) SHALL require a base version and merge the Markdown revision of that base as exactly one body edit record through the R05 validation, or none when it changes nothing (`changed: false`). It SHALL report whether the input was already Canonical Markdown (`normalised`). An unknown, not-yet-arrived or non-canonically encoded base SHALL return `base_unavailable`, and a merge that does not fit one record SHALL return `revision_too_large`; neither SHALL change the body. It SHALL accept only the Host-derived local Installation Person as actor, and Core SHALL sign with the Installation signer. A failure after the record was staged SHALL be reported as outcome-uncertain, and an uncertain command SHALL NOT be retried automatically.
- `documents.changed` (event) SHALL be live-only, at-most-once and coalescible. It SHALL fire on adoption of a local or remote body or listing change, and MAY name at most 64 changed Documents in `documentIds`; without it, the Consumer re-lists.

A version token SHALL be the Loro frontiers' IDs sorted by peer then counter, each encoded as an 8-byte big-endian peer and a 4-byte big-endian counter, in base64url without padding, and SHALL carry at most 64 IDs (1 024 characters). The Provider SHALL read a token strictly. A body whose frontiers exceed 64 IDs SHALL have no token until a later local edit converges them. A system test SHALL drive every Port through the Host broker and run the shared merge scenario table (`fixtures/merge-scenarios.json`) through `document.revise`.
