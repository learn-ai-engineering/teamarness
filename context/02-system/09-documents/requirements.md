# 02-system/09-documents — Documents Requirements

Prefix: `TEAM.SYS.DOCS`

The Documents Package (`dev.teamarness.documents`) provides Workspace Collections, Documents, and Comments. See ADR 0023.

## TEAM.SYS.DOCS-R01 — Package Boundary

The Documents Package SHALL own Collections, Documents, Comments, their canonical record encoding and validation, local projections, and its UI. It SHALL use only the standard Package Manifest, Package Process Protocol, lifecycle, Package Data Directory, UI contract, and declared Semantic Capabilities. The Core Runtime, Package Host, and Shell SHALL contain no Documents-specific behavior.

## TEAM.SYS.DOCS-R02 — Record Replication

The Documents Package SHALL replicate every Collection, Document, Comment, and body edit record through one `teamarness.synchronization.immutable-record-log` 1.x scope per Package and Workspace. Documents SHALL require the additive 1.1 replay-boundary minor; 1.0 clients SHALL keep their existing stream behavior. The Core Runtime SHALL treat Documents canonical bytes as opaque.

## TEAM.SYS.DOCS-R03 — Identity and Convergence

A Collection, Document, or Comment SHALL be identified by the Record ID of the record that created it. Rename, move, archive, restore, resolve, and reopen SHALL converge by Order Key, so every Installation shows the same state regardless of arrival order. A record whose target has not arrived SHALL be retained and applied when the target arrives.

## TEAM.SYS.DOCS-R04 — Collections and Documents

Any current Member SHALL be able to create a Collection with a name of 1 to 128 characters and a Document in a Collection with a title of 1 to 256 characters, and to rename either, move a Document between Collections, and archive or restore either. Archiving SHALL hide, never delete; archived entries SHALL remain openable and restorable.

## TEAM.SYS.DOCS-R05 — Collaborative Body

A Document body SHALL be a plain-text Markdown document maintained by a text CRDT. Concurrent edits, including edits made while disconnected, SHALL converge to identical text on every Installation with no edit lost. Each bounded identified local edit SHALL produce one sealed body edit record; old kind-10 records SHALL remain readable. No single record SHALL exceed the capability's canonical bound, and a too-large export SHALL be refused before the live body changes. A body SHALL be rebuilt from its records at activation and SHALL NOT be persisted as plaintext. A Document whose edit records cannot yet be opened SHALL be presented as pending rather than partially. Recovery SHALL report inclusion and durability separately for a bounded set of editor request IDs captured with one text snapshot; missing IDs remain unresolved until a gap-free, from-start local replay boundary and all relevant sealed edits have opened. Neither synchronization health nor a saved cursor proves absence. Package SQLite schema v5 SHALL index each authenticated kind-11 high-entropy request ID to its Record ID, Document and author without storing a plaintext payload digest or body. The index SHALL be rebuilt from opened applied and deferred sealed records before bodies are exposed after v4 migration. Distinct authenticated records sharing an edit request ID SHALL persist a Document-scoped `identity_conflict` marker for every affected Document, regardless of arrival order. Affected bodies SHALL hide text, reject edits and absence proofs, report unresolved recovery, and remain read-only across restart; later edits to them SHALL remain sealed/unapplied. Other Documents' edits and delivery SHALL proceed. No first-seen record is a winner or automatically repaired; sealed history remains for a future explicit repair. A local conflicting retry SHALL be rejected before append without quarantining the Document. The bounded in-memory identity cache SHALL not determine provenance after eviction.

## TEAM.SYS.DOCS-R06 — Edit Latency

With both Installations online, a body edit SHALL be visible to another Member within two seconds (vision success criterion 4). Verification is manual across separate networks.

## TEAM.SYS.DOCS-R07 — Comments

A current Member SHALL be able to add a Comment of 1 byte to 32 KiB of Markdown to a Document, optionally quoting up to 2 KiB of excerpt and optionally replying to a parent Comment on the same Document, and to resolve or reopen any Comment. The signed add record SHALL carry an author-clock creation timestamp for display (not ordering); old records without it SHALL show “Time unavailable.” Comments SHALL be retained with the Document and never deleted. A reply to a Comment on another Document SHALL be rejected.

## TEAM.SYS.DOCS-R08 — Authorship Presentation

Records SHALL store author Installation identities, never names. The UI SHALL label authors by current Workspace Person Name through `teamarness.person-names`, update labels after rename, and fall back to an inspectable identity. An agent Workspace Person SHALL be labelled the same way.

## TEAM.SYS.DOCS-R09 — Bounds

Local edits SHALL have a 192 KiB growth budget; valid concurrent remote edits beyond this size SHALL remain convergent and visible through bounded full-text chunks. While over budget, local edits SHALL not grow the body further and shrinking edits SHALL remain possible, including after a rejected growth request. Unknown append outcomes SHALL preserve exact live retry bytes and an unsaved cue rather than guessing inclusion after restart. Comments SHALL be retrievable through serialized-byte-bounded Order Key cursor pages, including older Comments; no single response SHALL exceed the 256 KiB bridge bound.

## TEAM.SYS.DOCS-R10 — Non-Goals of Version 1

Folder mapping, rich text, document-level permissions beyond Canonical Membership, log compaction, and a Core document capability family are not delivered. A `document.snapshot` record kind is reserved for compaction.
