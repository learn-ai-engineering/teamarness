# 0023 — Documents are Package-owned Loro updates on the immutable record log

**Status:** accepted (2026-10-21)
**Refines:** ADR 0002 (stack; neither Iroh blobs nor iroh-docs adopted), ADR 0008/0009 (immutable record capability and Core synchronization), ADR 0018 (a document family would be a new capability family), ADR 0022 (no Package-to-Package channel).

## Context

Teamarness needs collaborative Markdown documents so a whole team, humans and AI agents alike, can plan together (vision success criterion 4: an edit visible across NAT within two seconds). The stack already ships Iroh, and Iroh publishes `iroh-docs`, so a reader will expect documents to use it. Packages may not own Iroh protocols (ADR 0002, 0009), the only replicated store a Package has is the immutable record log, and no Package can read another Package's data (ADR 0022).

## Decision

Documents are an ordinary bundled Package, `dev.teamarness.documents`, with no Documents-specific Core semantics and no new capability family. Generic initial-replay proof and Package Host generation retirement extend the existing `teamarness.synchronization.immutable-record-log` capability to minor 1.1; Documents requires that minor. Every change remains a record on this capability:

- **Metadata records** (Collection and Document create, rename, move, archive, restore; Comment add, resolve, reopen) are small Package-owned canonical records. A Collection, Document, or Comment is identified by the `RecordId` of the record that created it. Every other metadata record is last-writer-wins by `OrderKey`, so arrival order does not matter and a late earlier-sorting record never overrides later state.
- **Body records** (`document.edit`) carry one Loro update for one Document. Legacy kind 10 remains readable; identified kind 11 carries a 128-bit request ID and digest of the immutable request payload. Each bounded request produces one sealed record, not a coalesced group; a large UI action is split into sequential identified requests. The 300 ms idle delay remains a send/append delay, not permission to combine identities. The live Loro is changed only after an export on a deep fork fits one record. Remote updates import in any order because Loro holds an update until dependencies arrive. The Loro peer id is derived from the Installation identity.
- **No body or payload digest is persisted in plaintext.** Like Conversations, Package SQLite holds sealed records, ordering, archive state, and threading. Schema v5 also projects each authenticated kind-11 edit's opaque 128-bit request ID to its immutable Record ID, Document ID and author. The exact transactional index detects a second authenticated Record ID for one request ID before importing the second update. It atomically marks **both** affected Documents `identity_conflict` (including cross-Document collisions), retains all sealed records and acknowledges the cursor. Neither first-seen edit is an authoritative winner: existing body text becomes hidden and read-only, subsequent edits for the affected Documents remain sealed/unapplied, and unrelated Documents continue. On v4 migration, all opened applied and deferred edits are indexed before exposing any body. The monotonic marker persists across restarts; repair is a separate future operation, not an automatic rollback. Local conflicting retries are rejected before append and do not create a marker. Kind-10 edits have no index row, and upgrading a v4 database does not assert that the new index is complete: activation rebuilds it from authenticated opened sealed rows before qualified replay can prove an ID absent. Names, titles, comment text, document bodies and edit payload digests are rebuilt in memory by opening records through the capability. A Document whose edit records cannot be opened yet is pending, not partially shown. Recovery captures text and bounded queried-ID inclusion/durability statuses under one body lock. A missing ID is unresolved until a credited, gap-free from-start local replay marker has followed all record ingests, all prior projected record IDs have been seen again, and relevant edits are open. Stored cursor, `current` health, and zero pending count do not prove absence. The Host closes old-generation append admission, cancels queued work, and drains entered Core closures before replacement replay; a failed drain leaves the Instance failed and the draft unsaved.
- **Comments live in the Documents Package**, anchored to a Document with an optional quoted excerpt and an optional parent, so rationale and decisions travel with the document the way comments travel with code. They are never deleted; Documents and Collections are archived, never purged.
- **Agents need nothing special.** An agent is a Workspace Person (the `agent-invite` workstream); its records are attributed like anyone's.

The text CRDT is `loro` 1.16.2, pinned exactly.

## Rejected alternatives

- **`iroh-docs`.** It is a signed key-value replica with last-timestamp-wins reads: it does not merge concurrent text edits, it needs `iroh-blobs` and `iroh-gossip` mounted on Core's Router, its namespace-capability access model ignores Canonical Membership and revocation, and it has no Content Key protection. It would be a second replication stack that still needed a text CRDT on top, and it was still 0.x after iroh 1.0.
- **A Core document capability family now.** ADR 0018 names it as the expected next family, but there is no second consumer. The Package-owned format can be lifted into Core when one appears, as names were (ADR 0022).
- **Automerge or yrs.** Automerge was pre-1.0 on an alpha storage engine and its per-change replay was O(history) in measurement; yrs is leanest but has no built-in text diff import, which the folder-mapping follow-up needs. Loro replayed 10,000 per-edit records in under 50 ms with 600-byte snapshots and deterministic encoding.
- **Persisting a plaintext Loro snapshot per Document.** It would break the precedent that Package SQLite holds no plaintext; replay is cheap. A `document.snapshot` record kind is reserved for when logs grow.
- **Rich text, a UI-side CRDT, or exact text-range comment anchors.** Each adds merge or sandbox complexity for no v1 benefit; a quote survives edits where a range would dangle.
- **Comments in Conversations, or a fourth composing Package.** No Package-to-Package channel exists and ADR 0022 declined to add one.

## Consequences

- Record growth is append-only with no compaction; a snapshot record is the planned relief. A live transient append retries exact canonical bytes by request ID; after an uncertain restart, unknown IDs remain unsaved rather than guessed or resent until qualified replay proves absence. An abrupt UI process crash can still lose an unacknowledged in-memory draft. Every non-archived Document's body is resident per Backend per Workspace.
- Folder mapping (editing documents from a mapped folder with any editor or CLI agent) is a separate workstream; Loro's text diff import keeps it open.
- The dependency tree grows by roughly 140 crates, including second `getrandom` and `rand` versions; all are MIT, Apache-2.0, MPL-2.0, BSL-1.0, Zlib, or Unlicense.
- The 256 KiB UI payload bound requires bounded full-text chunks and comment pages. Local edits have a 192 KiB growth budget; concurrent valid remote edits can grow a Document beyond it and remain convergent, with only no-growth or shrinking local edits allowed until it is back under budget.

## Evidence

- Workstream `workspace-documents`: alignment `scope/20261018T233105Z_workspace-documents.md`, research `research/20261019T010320Z_text-crdt-and-record-log-seams.md` (probe source retained), design `design/20261019T015245Z_workspace-documents.md`.
- `packages/documents-package/`; `context/02-system/09-documents/requirements.md`.
- `context/.decisions/0002-rust-tauri-iroh.md`, `0008-immutable-record-capability-contract.md`, `0009-core-synchronization-foundation.md`, `0018-package-seam-versioning.md`, `0022-workspace-person-names.md`.
