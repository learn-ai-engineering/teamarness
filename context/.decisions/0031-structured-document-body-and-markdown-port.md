# 0031 — Document bodies are structured Loro trees edited in the UI, with Markdown as the interchange

**Status:** accepted (2026-10-24)
**Supersedes:** ADR 0027 (Decision, the **Body records** bullet: "Legacy kind 10 remains readable", "The live Loro is changed only after an export on a deep fork fits one record" as applied to backend-made text edits, and "The Loro peer id is derived from the Installation identity"; Rejected alternatives, the bullet "Rich text, a UI-side CRDT, or exact text-range comment anchors" except its exact-range-anchor clause; Consequences, the 256 KiB bullet's "bounded full-text chunks")
**Refines:** ADR 0026 (a second Person-only Port Contract), ADR 0029 (adds a schema-violation import disposition), ADR 0030 (consumes WebAssembly in frames)

## Context

The Documents body must be edited in rendered form by Members and read and revised as Markdown by other Packages and, later, agents. Concurrent edits from several Installations must converge through Loro on the immutable record log, within its canonical record bound and the 256 KiB UI payload bound, without plaintext persistence.

A rendered editor that merges formatting needs a CRDT that understands document structure. `loro-prosemirror` maps a ProseMirror document onto a Loro tree and requires the editor to be a Loro peer.

Under Loro, two writers that create the same root child container concurrently resolve last-writer-wins, so one side's content vanishes. Two writers that share a peer id silently diverge. A fork taken at an older version and reusing a peer id mints conflicting op ids.

Markdown cannot represent every structured body exactly.

## Decision

**Body model.** A Document body is the Loro tree that `loro-prosemirror` writes for one fixed baseline ProseMirror schema, which the Package UI and backend share. The schema stores no list tightness.

**The UI is a Loro peer.** The Package UI runs `loro-crdt` and `loro-prosemirror` in its frame and sends Loro updates to its backend. The backend imports each UI update into a fork of the applied body and checks four things: the update contains ops only from that session's assigned peer, it fits one record, the resulting tree is valid under the schema, and it respects the local growth budget. Only then does the backend append one record and adopt the fork. An update refused for a reason retrying cannot cure ends its session with a resync, and the UI discards that session's state and reloads (see **Sessions**).

A remote record whose import fails, or leaves the tree schema-invalid, takes ADR 0029's import-failure disposition, with the new category `schema_violation`.

**Genesis.** `document.create` carries the body genesis: the root containers and one empty paragraph, written once by the creating backend. No UI and no later record initializes a body.

**Peer identity.** Every Loro peer is a random 64-bit id allocated by the backend, absent from the Document's version vector. One goes to each UI editor session, each Markdown merge fork and each genesis. Peer ids are never derived from the Installation. A peer is reused only when an editor session resumes as its own peer under the guarded rule in **Sessions**; every other peer is fresh. Peers carry no authority: records are still signed by the Installation signer.

**Records.** Body edits keep ADR 0027's identified kind-11 envelope (request ID, payload digest, one update) and its identity, recovery and conflict rules. The canonical update bound is 160 KiB, so a base64 update fits one UI message without chunking. A large local paste is split at top-level block boundaries into sequential records. A single top-level block that cannot fit one record, including a whole list or blockquote, is refused before it is committed locally, and so is any other local change whose update cannot fit one record, such as a structural edit that rewrites large blocks. A local change whose undo could not fit one record is not undoable: it is committed with an origin the undo manager excludes, so it never enters the undo history. A redo that could not fit one record is removed with the redo stack as soon as the undo that pushed it runs. No undo or redo therefore writes more than one record, whichever step the undo manager runs. Kind 10 and plain-text bodies are removed. No Documents existed outside development, and development data is reset.

**Sessions.** A UI editor session loads from a backend-produced snapshot, delivered in bounded chunks, never from replayed record history. It then receives live updates from remote records and other local sessions, including while the body waits for missing history, so its own unacknowledged edits survive out-of-order delivery. A session holds a doc when it opened with the snapshot of an editable body or the backend accepted its resume as its own peer. Such a session never loses its unacknowledged edits to a reset the backend starts: when the body gains more than one update can carry, the session falls behind the body's events, or the body becomes editable for a session that holds a doc but has not yet been sent it, the backend sends a fresh opening as the same peer on the same stream, and the UI merges that snapshot into the doc it kept and recovers its queue. A fresh opening sent while the body is pending carries no snapshot; the session keeps following the body and gets another opening once the body is editable. Only an update refusal that retrying cannot cure, or an identity conflict, ends a session with a resync that discards its local state; a session that holds no doc is resynced for the other resets too. When a session's stream ends without a resync because the backend generation was replaced, or the backend no longer knows the session, the UI may resume as the same peer. The backend accepts that only for a peer that no open session holds, that no other Installation's record carries, and that this process either handed to an ended session for that Document or never handed out. Because the backend hands out a fresh peer while it still holds the old session, the UI closes the old stream before it resumes, and asks again a bounded number of times when it is handed another peer. The resumed session then resends only the requests that recovery proves absent. Only the latest recovery decides the queue, and a session that closes reports what is still unacknowledged, so no queued edit is dropped without notice. Per-person undo uses the binding's undo plugin and never reverts another peer's ops. Each local change is its own undo step (no merge interval), except that a split paste is one step, and the history holds at most 500 steps.

**Markdown.** The backend is the single Markdown authority. It exports **Canonical Markdown**: the body as Markdown can carry it, with formatting that Markdown cannot express dropped before writing, so the export always imports back to the body it describes. Content always wins over formatting. The normalisations are a documented contract (`context/02-system/09-documents/markdown-contract.md`).

A Markdown revision merges against an explicit base version. The backend forks at the base with a fresh peer and reconciles by block alignment, so unchanged blocks are untouched and changed blocks are edited in place. The merge runs on a blocking thread without holding any body, and its character diffs share a time budget; once it is spent, the remaining changed text runs are replaced between their unchanged prefix and suffix, so no revise can stall the backend. The merge returns one update without changing the body. The body adopts it only after its one record is appended, so concurrent edits since the base merge through Loro. Merging back an unchanged export produces no ops. The UI reads Canonical Markdown from the backend in request-sized chunks, each carrying a digest of the whole text.

**Documents Port Contract 1.0.** It offers:

- `document.list` and `document.markdown` (read-only queries);
- `document.revise` (a Person-only command whose `baseVersion` is required, where one revise is one record and an over-bound revise changes nothing);
- `documents.changed` (a live, coalescible invalidation).

A version token is the canonical encoding of Loro frontiers. A response larger than the Host Port value bound (`MAX_UI_PAYLOAD_BYTES`) is refused as `too_large`. Authority follows Conversations 1.0 (ADR 0026): the Host-derived Installation Person is checked, and Core signs. Agent-signed revisions wait for the Agent authority decision.

## Rejected alternatives

- **Keeping the backend as the only Loro peer, with the UI sending text operations or ProseMirror steps.** A rendered editor would then rewrite Markdown text, so formatting would not merge and per-person undo could not be built.
- **Loro version-vector identity instead of request IDs.** Cleaner, but it replaces the recovery, index and conflict machinery that ADRs 0027–0029 qualified, for no new behaviour.
- **A recordless, deterministic genesis derived from the Document ID.** It makes correctness depend on byte-identical op generation across Loro versions.
- **UI-chosen peer ids.** The UI can't see the whole version vector, and the backend must enforce the peer restriction anyway. A resuming session names only the peer the backend gave it, and the backend accepts that only under the **Sessions** rule.
- **Markdown conversion in the UI as well.** Two implementations cannot keep one canonical byte-stable form.
- **A last-writer revise without a base version.** It overwrites newer human text.
- **Splitting one revise across several records.** Peers can observe a partial revision if a later append fails.
- **Tiptap.** Its default style injection violates the frame policy, and a fixed shared schema gains little from it.

## Consequences

- The UI bundle carries `loro-crdt` WebAssembly and ProseMirror, about 5.0 MB raw (4.76 MiB), within the 8 MiB entrypoint bound. `loro-crdt` and the Rust `loro` crate must stay at the same exact version, and a qualification test enforces it. `loro-prosemirror` is pre-1.0 and pinned exactly. Committed cross-language fixtures guard the shared schema.
- Markdown fidelity and merge minimality are properties tested at scale: round-trip, fixed point, content preservation, no-op merge, revision equality, block identity and concurrent survival.
- Known merge semantics, documented in the contract: an agent's block deletion loses concurrent edits inside that block; moves are a delete plus an insert; a hard-break structure change rebuilds its paragraph; concurrent structural changes to one block can duplicate text.
- Future log compaction (`document.snapshot`) must keep port base versions resolvable or refuse stale ones explicitly.
- A Member can still forge another peer's op ids in their own signed records from another Installation. As in ADR 0027, this is not addressed.
- Exact text-range Comment anchors stay rejected. Quoted excerpts remain.
- Undo works per change, not per burst of typing, and a change too large to undo in one record, such as deleting most of a large body, cannot be undone. Recovering from such a change needs a future way to view earlier Document versions and copy from them, built on the immutable record history. Undoing an agent's edits is out of scope.

## Evidence

- `packages/documents-package/` — `backend/src/{body,domain,markdown,ports}`, `backend/migrations/0007_schema_violation.sql`, `ui-contract.json`, `ports.json`, `ui/src/editor/`, `ui/frame-scenarios/`; `context/02-system/09-documents/requirements.md` and `markdown-contract.md`.
- `scripts/check-exact-pins.mjs` enforces `loro`/`loro-crdt` parity; `packages/documents-package/fixtures/body-trees/` holds the cross-language schema fixtures.
- `packages/documents-package/ui/frame-scenarios/` (`npm run verify:documents-editor`): same-paragraph overlapping-format, offline-then-online and two-view convergence, per-person undo, split pastes, refusal before commit of an over-bound block and structural edit, and refusal of an undo or redo too large for one record, in WebKit and Chromium under the frame policy.
- `packages/documents-package/ui/src/editor/undo.test.ts` (seeded interleavings of local and remote edits, undo and redo, with no update over one record) and `session.model.test.ts` (a model-based, seeded interleaving test of the Editor Session against a fake that follows the backend's session rules).
- `packages/documents-package/fixtures/body-trees/` with `backend/tests/cross_language.rs` and `ui/src/editor/cross-language.test.ts`: Rust reads every binding-written tree, and the binding reads every Rust-written tree under the shared schema.
- `backend/tests/markdown_properties.rs`, `markdown_corpus.rs` and `markdown_merge_scenarios.rs`: the Markdown and merge properties, the corpus and the shared merge scenarios.
- **Weaknesses.** The frame scenarios run in a local harness, not the packaged WKWebView. IME and composition input are not exercised. The tree layout belongs to a pre-1.0 binding.

### Provenance (non-versioned)

- Feasibility spike on the local branch `spike/rendered-document-editing` (`packages/documents-package/prototype-rendered-body/`), described in the prototype artifact below:
  - Same-paragraph overlapping-format, offline and two-view convergence passed in WebKit and Chromium.
  - Concurrent empty-document creation lost one side's text (the genesis race).
  - Two views sharing a peer silently diverged.
  - Rust read every binding-written snapshot, and 300 of 300 random Rust-written trees passed the binding's `node.check()`.
  - Markdown and merge properties held at 60 000 and 20 000 cases.
  - An ordinary edit was 100–160 B, a 100 KB paste was one 102 551 B update, and a 200 KB document's snapshot was 63 531 B.
- Workstream `rendered-document-editing`: `scope/20261024T054907Z_rendered-document-editing.md`, `prototype/20261024T062700Z_option-c-feasibility-spike.md`, `design/20261024T070649Z_rendered-document-editing.md`.
