# 0023 — Documents are Package-owned Loro updates on the immutable record log

**Status:** accepted (2026-10-21)
**Refines:** ADR 0002 (stack; neither Iroh blobs nor iroh-docs adopted), ADR 0008/0009 (immutable record capability and Core synchronization), ADR 0018 (a document family would be a new capability family), ADR 0022 (no Package-to-Package channel).

## Context

Teamarness needs collaborative Markdown documents so a whole team, humans and AI agents alike, can plan together (vision success criterion 4: an edit visible across NAT within two seconds). The stack already ships Iroh, and Iroh publishes `iroh-docs`, so a reader will expect documents to use it. Packages may not own Iroh protocols (ADR 0002, 0009), the only replicated store a Package has is the immutable record log, and no Package can read another Package's data (ADR 0022).

## Decision

Documents are an ordinary bundled Package, `dev.teamarness.documents`, with no Core change and no new capability family. Every change is a record on `teamarness.synchronization.immutable-record-log`:

- **Metadata records** (Collection and Document create, rename, move, archive, restore; Comment add, resolve, reopen) are small Package-owned canonical records. A Collection, Document, or Comment is identified by the `RecordId` of the record that created it. Every other metadata record is last-writer-wins by `OrderKey`, so arrival order does not matter and a late earlier-sorting record never overrides later state.
- **Body records** (`document.edit`) carry one Loro update for one Document. Each Document is one Loro plain-text container; Markdown is edited as plain text. Local edits are coalesced into one record after a short idle window or a size budget, large inserts are split, and remote updates import in any order because Loro holds an update until its dependencies arrive. The Loro peer id is derived from the Installation identity.
- **Nothing plaintext is persisted.** Like Conversations, the Package SQLite holds sealed records, ordering, archive state, and threading; names, titles, comment text, and document bodies are rebuilt in memory at activation by opening records through the capability. A Document whose edit records cannot be opened yet is pending, not partially shown.
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

- Record growth is append-only with no compaction; a snapshot record is the planned relief. Every non-archived Document's body is resident per Backend per Workspace.
- Folder mapping (editing documents from a mapped folder with any editor or CLI agent) is a separate workstream; Loro's text diff import keeps it open.
- The dependency tree grows by roughly 140 crates, including second `getrandom` and `rand` versions; all are MIT, Apache-2.0, MPL-2.0, BSL-1.0, Zlib, or Unlicense.
- The 256 KiB UI payload bound requires bounded full-text chunks and comment pages. Local edits have a 192 KiB growth budget; concurrent valid remote edits can grow a Document beyond it and remain convergent, with only no-growth or shrinking local edits allowed until it is back under budget.

## Evidence

- Workstream `workspace-documents`: alignment `scope/20261018T233105Z_workspace-documents.md`, research `research/20261019T010320Z_text-crdt-and-record-log-seams.md` (probe source retained), design `design/20261019T015245Z_workspace-documents.md`.
- `packages/documents-package/`; `context/02-system/09-documents/requirements.md`.
- `context/.decisions/0002-rust-tauri-iroh.md`, `0008-immutable-record-capability-contract.md`, `0009-core-synchronization-foundation.md`, `0018-package-seam-versioning.md`, `0022-workspace-person-names.md`.
