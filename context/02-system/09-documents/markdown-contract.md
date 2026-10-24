# 02-system/09-documents — Markdown Contract

This contract is normative. It defines how a Document body converts to and from Markdown, and how a Markdown revision merges into a body. ADR 0031 decides that the Documents backend is the only Markdown authority. The implementation is the Markdown core, `packages/documents-package/backend/src/markdown/`.

The key words SHALL, SHALL NOT and MAY are used as in the Documents requirements.

## Body schema

A Document body is the Loro tree that `loro-prosemirror` 0.4.4 writes for one fixed ProseMirror schema. The Package UI and the backend share the schema.

- The root is the Loro map `doc`. Every node is a map `{nodeName, attributes, children}`. Consecutive text in a node is one `LoroText` entry in `children`, with marks as Loro text styles.
- **Block nodes:**
  - `paragraph` holds text and `hard_break`.
  - `heading` has `level` 1–6 and holds text only.
  - `blockquote` holds blocks.
  - `horizontal_rule` holds nothing.
  - `code_block` has `params` (the info string) and holds unmarked text.
  - `bullet_list` holds `list_item`.
  - `ordered_list` has `order`, from 0 to 999 999 999, and holds `list_item`.
  - `list_item` holds a `paragraph`, then blocks.
- **Marks:** `strong`, `em`, `strikethrough`, `code`, and `link` with `href` and an optional `title`.
- Number attributes are stored as JavaScript numbers (doubles).
- The schema has no list `tight` attribute. List spacing is not stored.
- Every writer SHALL configure mark expansion as the binding does: `after` for every mark, and `none` for `link`.
- The reader SHALL read text through Loro text deltas, never through `toJSON()`, because `toJSON()` drops marks.

### Schema violations

A tree is **schema-invalid** when a peer wrote a shape that the binding never writes. Each of these is a violation:

- the root has no `children` list, or its `nodeName` is not `doc`;
- a node has no string `nodeName`, or its name is not in the schema;
- a node, text run or plain value sits where its parent's content does not allow it, for example a `list_item` outside a list, a `hard_break` in a heading, or text directly in a block list;
- an `attributes` or `children` entry is not the container that the schema expects;
- an attribute has the wrong type or is out of range;
- a mark is not in the schema, a link has no string `href`, or a code block's text carries a mark;
- nodes are nested deeper than 64 levels (the root's children are level 1, and a list item is one level below its list).

Shapes that ordinary concurrent edits can produce are **not** violations, and the reader SHALL accept them:

- an empty body;
- an empty list or blockquote;
- a list item that lost its leading paragraph.

Unknown attributes on known nodes are ignored.

## Canonical Markdown

**Canonical Markdown** is the body as Markdown can carry it. The export SHALL be CommonMark with the GFM strikethrough extension only, written as follows:

- ATX headings (`#`);
- `-` bullets, and `1.`-style ordered lists that keep their start number;
- `*em*`, `**strong**` and `~~strike~~`;
- fenced code blocks with three backticks;
- `-----` for rules;
- a backslash for hard breaks;
- no line wrapping.

Two adjacent lists of the same kind are separated by `<!-- end list -->`, which import drops.

For every body, the export SHALL satisfy three guarantees:

1. **Fixed point.** Export, then import, then export again gives the same bytes.
2. **Canonical round trip.** Import of the export gives exactly the body's canonical form. For a body that Markdown can carry, the canonical form is the body itself.
3. **No content loss.** Every content character survives export and import. Only whitespace at the edges of a line is normalised.

### Normalisations

The canonical form makes only these changes. Content always wins over formatting.

- **Line-edge whitespace.** Spaces at the start of a line, and at the end of a block, are dropped, because Markdown drops them.
- **Delimiters move off whitespace.** Bold, italic and strikethrough move off whitespace at their edges, and off whitespace at boundaries where they overlap other formatting. For example, a body with overlapping bold and italic exports as `The **quick** ***brown*** *fox AAA BBB jumps* over the dog`. Only when CommonMark flanking rules make a span impossible is the smallest such span dropped.
- **Code and link fallback.** When comrak's writer cannot carry a paragraph's inline code or link formatting, the code formatting is dropped first and then the link formatting, until the paragraph round-trips. The known case is text that would read as a link reference definition (`]:`): written as is, it would delete the paragraph.
- **Lists are written tight whenever that is safe.** Spacing is not stored. A list is written loose only when a tight list would join two of its blocks.
- **Empty paragraphs** have no Markdown form. Export omits them, and merge keeps them (see below).
- **A list item's empty leading paragraph** is not written when other blocks follow it. Import restores it.
- **Soft line breaks** import as spaces.
- **Trailing hard breaks** at the end of a paragraph are dropped, and a hard break inside a heading imports as nothing.
- **Unsupported Markdown is literal text.** Tables, task lists, HTML and images import as the characters the author wrote, escaped on export, so nothing an author wrote is lost. Entities import as the characters they name.
- **Reference-style links** import as inline links.
- **Other syntax forms** import as their canonical equivalents: setext headings, `*` and `+` bullets, `1)` lists, `__bold__`, `_em_`, `~strike~`, indented code, `~~~` fences and two-space hard breaks.

The canonical corpus (`backend/tests/fixtures/markdown-corpus/`) records these normalisations. It has 17 byte-stable canonical documents and 22 agent-style inputs, each with its expected canonical export.

### Known gap

A list item whose first written block is a horizontal rule does not round-trip: comrak writes the rule after a blank line, so it leaves the item. The property generators exclude this shape. It is not a schema violation.

## Base-versioned merge

`merge(doc, base, markdown, peer)` merges a Markdown revision that was made against the version `base`. The result is one Loro update from `peer`.

- The base SHALL be in the Document's history. Otherwise the merge is refused, and nothing changes.
- `peer` SHALL have no operations in the Document. A fork at an older base cannot see a reused peer's later operations, so it would mint colliding op IDs. The backend allocates a fresh random peer for every merge.
- The merge forks the Document at the base and reconciles the fork against the parsed revision. Concurrent edits made since the base therefore merge through Loro.
- The merge does not change the Document. The caller appends the update as one record and then imports it.
- After the update is imported, with no concurrent edits, the body exports exactly the revision's Canonical Markdown.
- Merging back an unchanged export SHALL produce zero operations, even for a body that Markdown cannot carry exactly. No record is written.
- A revision whose nesting exceeds the schema's 64 levels is refused before any recursive walk.

Reconciliation works like this:

1. Blocks at each level are aligned on their canonical forms: the unchanged prefix and suffix, then a longest common subsequence of the blocks between them.
2. Unchanged blocks are never touched.
3. In each gap between aligned blocks, leftover blocks of the same kind are paired by position and edited in place. Text is edited by character diff, formatting by mark and unmark ranges, and attributes only where they differ. List items pair with list items, and their blocks are reconciled recursively.
4. Remaining leftovers are deleted or inserted.

A formatting-only change therefore costs mark operations and no text operations.

### Empty paragraphs and empty revisions

- An empty paragraph that the revision cannot mention stays where it is.
- When a block list shows nothing at all, such as a new Document's genesis paragraph, a revision that shows something replaces it. A paragraph is edited in place, so the genesis paragraph keeps its identity.
- An empty revision leaves one empty paragraph, because the schema's `doc` holds at least one block.

### Limitations

These are the accepted merge semantics:

- **A deleted block loses concurrent edits inside it.** This is ordinary CRDT behaviour.
- **A move is a delete plus an insert.** Concurrent edits inside the moved block are lost.
- **A change in hard-break structure rebuilds the paragraph.** If the revision adds or removes a hard break in a paragraph, that paragraph's inline content is replaced, and concurrent edits inside it are lost.
- **Leftovers in a gap are paired by position.** Similarity-based pairing would preserve more.
- **Large rewrites lose minimality.** When the blocks between the unchanged prefix and suffix would need more than 4 Mi alignment cells, they are paired by position without a common subsequence. The result still exports the revision exactly.
- **Without a base, a revision overwrites newer text.** A revision merged against the current version instead of the exported one reverts edits made after the export. This is why the port requires a base version.
- **Concurrent structural changes duplicate text.** For example, wrapping a block in a list while another Member makes it a heading converges with the text duplicated. This is how the binding replaces nodes.

## Verification

- Property suites in `backend/tests/markdown_properties.rs`:
  - P1: an expressible body round-trips exactly.
  - P1b: any body round-trips to its canonical form.
  - P2: the export is a fixed point.
  - P3: no content is lost.
  - The tree store round-trips.
  - P4: an unchanged revision is a no-op.
  - P5: any revision lands exactly.
  - P6: a one-block edit keeps every other block's identity.
  - P7: concurrent human and agent edits both survive.
- The `CASES` environment variable sets the scale, with a default of 3 000. Merge properties run a third of it. The long run is `CASES=60000 cargo test -p teamarness-documents-package --release --test markdown_properties`.
- `backend/tests/markdown_corpus.rs` checks the corpus.
- `packages/documents-package/fixtures/merge-scenarios.json` is the shared merge scenario table. The backend runs it against `merge`, and the Documents port system test runs it through `document.revise`.
