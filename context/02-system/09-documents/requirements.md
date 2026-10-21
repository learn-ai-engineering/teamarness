# 02-system/09-documents — Documents Requirements

Prefix: `TEAM.SYS.DOCS`

The Documents Package (`dev.teamarness.documents`) provides Workspace Collections, Documents, and Comments. See ADR 0023.

## TEAM.SYS.DOCS-R01 — Package Boundary

The Documents Package SHALL own Collections, Documents, Comments, their canonical record encoding and validation, local projections, and its UI. It SHALL use only the standard Package Manifest, Package Process Protocol, lifecycle, Package Data Directory, UI contract, and declared Semantic Capabilities. The Core Runtime, Package Host, and Shell SHALL contain no Documents-specific behavior.

## TEAM.SYS.DOCS-R02 — Record Replication

The Documents Package SHALL replicate every Collection, Document, Comment, and body edit record through one `teamarness.synchronization.immutable-record-log` 1.x scope per Package and Workspace. The Core Runtime SHALL treat Documents canonical bytes as opaque.

## TEAM.SYS.DOCS-R03 — Identity and Convergence

A Collection, Document, or Comment SHALL be identified by the Record ID of the record that created it. Rename, move, archive, restore, resolve, and reopen SHALL converge by Order Key, so every Installation shows the same state regardless of arrival order. A record whose target has not arrived SHALL be retained and applied when the target arrives.

## TEAM.SYS.DOCS-R04 — Collections and Documents

Any current Member SHALL be able to create a Collection with a name of 1 to 128 characters and a Document in a Collection with a title of 1 to 256 characters, and to rename either, move a Document between Collections, and archive or restore either. Archiving SHALL hide, never delete; archived entries SHALL remain openable and restorable.

## TEAM.SYS.DOCS-R05 — Collaborative Body

A Document body SHALL be a plain-text Markdown document maintained by a text CRDT. Concurrent edits, including edits made while disconnected, SHALL converge to identical text on every Installation with no edit lost. Local edits SHALL be appended as bounded body edit records; no single record SHALL exceed the capability's canonical bound. A body SHALL be rebuilt from its records at activation and SHALL NOT be persisted as plaintext. A Document whose edit records cannot yet be opened SHALL be presented as pending rather than partially.

## TEAM.SYS.DOCS-R06 — Edit Latency

With both Installations online, a body edit SHALL be visible to another Member within two seconds (vision success criterion 4). Verification is manual across separate networks.

## TEAM.SYS.DOCS-R07 — Comments

A current Member SHALL be able to add a Comment of 1 byte to 32 KiB of Markdown to a Document, optionally quoting up to 2 KiB of excerpt and optionally replying to a parent Comment on the same Document, and to resolve or reopen any Comment. Comments SHALL be retained with the Document and never deleted. A reply to a Comment on another Document SHALL be rejected.

## TEAM.SYS.DOCS-R08 — Authorship Presentation

Records SHALL store author Installation identities, never names. The UI SHALL label authors by current Workspace Person Name through `teamarness.person-names`, update labels after rename, and fall back to an inspectable identity. An agent Workspace Person SHALL be labelled the same way.

## TEAM.SYS.DOCS-R09 — Bounds

Document text is bounded to 192 KiB in version 1; an edit that would exceed it SHALL be refused with a stable error. A Comments listing SHALL carry at most the newest 256 Comments and say when older ones were omitted.

## TEAM.SYS.DOCS-R10 — Non-Goals of Version 1

Folder mapping, rich text, document-level permissions beyond Canonical Membership, log compaction, and a Core document capability family are not delivered. A `document.snapshot` record kind is reserved for compaction.
