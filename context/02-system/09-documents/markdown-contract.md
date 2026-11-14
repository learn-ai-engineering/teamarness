# 02-system/09-documents — Markdown Contract

This contract is normative. It defines how a Document body converts to and from Markdown, and how a Markdown revision merges into a body. ADR 0032 decides that the Documents backend is the only Markdown authority. The implementation is the Markdown core, `packages/documents-package/backend/src/markdown/`.

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
  - `preserved_block` has `kind` (`table`, `front_matter` or `html`) and holds unmarked text. It may sit wherever a `code_block` may, except that `front_matter` is only ever the first top-level block.
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
- a mark is not in the schema, a link has no string `href`, or the text of a code block or a Preserved Block carries a mark;
- a `preserved_block` has a `kind` that is not in the schema;
- a `front_matter` Preserved Block is not the first top-level block;
- nodes are nested deeper than 64 levels (the root's children are level 1, and a list item is one level below its list).

Shapes that ordinary concurrent edits can produce are **not** violations, and the reader SHALL accept them:

- an empty body;
- an empty list or blockquote;
- a list item that lost its leading paragraph;
- a Preserved Block whose source no longer reads as its kind.

Unknown attributes on known nodes are ignored. A `preserved_block` with no `kind` reads as `html`, the default of the editor schema.

## Canonical Markdown

**Canonical Markdown** is the body as Markdown can carry it. Apart from the source of Preserved Blocks, the export SHALL be CommonMark with the GFM strikethrough extension only, written as follows:

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
3. **No content loss.** Every content character of a body survives export and import. Only whitespace at the edges of a line is normalised.

This guarantee covers a body. Import of Markdown from outside does not keep all of it. Import drops these. The list is what testing has found so far, not a proof that nothing else can be lost.

- a reference definition that no link uses;
- the `<!-- end list -->` separator comment;
- the title of an image, and a link inside an image's alt text;
- a link with no text, outside a table that keeps its lines. The exception is a reference in a table that falls back to literal text: the links paragraph still holds its URL;
- a link target, under the [limits on link targets](#limits-on-link-targets).

### Preserved Blocks

A **Preserved Block** keeps Markdown that the editor cannot carry as its exact source lines. Import recognises three constructs on every path, and export writes the source byte for byte:

- a GFM table;
- YAML front matter: a `---` block that opens the document, with no blank line directly inside either delimiter, and no whitespace after either delimiter;
- an HTML block, except the `<!-- end list -->` separator.

The stored source has no container prefix (`> `, list indent) and no final line ending. The writer adds the prefix back.

A source is **sound** when it re-imports as the same Preserved Block in every place it can move to: after a list, inside a blockquote, as the first block of a list item, and after a paragraph in a list item, with another block following each time. Front matter is checked at the top of the document only. A source that reads as deeper nesting than a revision may have is not sound, because import never walks a syntax tree that deep.

Import SHALL fall back to literal text for a construct that is not sound, so content is never lost. An HTML block becomes one literal paragraph. A table becomes what its lines are without the table extension. An opening `---` block is read without the front matter extension, so two horizontal rules stay rules. An unclosed `<!-- comment` is the common case of an HTML block that falls back, because it would swallow whatever follows it. Export treats a stored source that is not sound the same way: a concurrent edit can leave one, and it exports as one literal paragraph.

### Normalisations

The canonical form makes only these changes. Content always wins over formatting.

- **Line-edge whitespace.** Spaces at the start of a line, and at the end of a block, are dropped, because Markdown drops them.
- **Delimiters move off whitespace.** Bold, italic and strikethrough move off whitespace at their edges, and off whitespace at boundaries where they overlap other formatting. For example, a body with overlapping bold and italic exports as `The **quick** ***brown*** *fox AAA BBB jumps* over the dog`. Only when CommonMark flanking rules make a span impossible is the smallest such span dropped.
- **Code and link fallback.** When comrak's writer cannot carry a paragraph's inline code or link formatting, the code formatting is dropped first and then the link formatting, until the paragraph round-trips. The known cases are text that would read as a link reference definition (`]:`), which would delete the paragraph if written as is, and a link destination with a space.
- **Lists are written tight whenever that is safe.** Spacing is not stored. A list is written loose only when a tight list would join two of its blocks. A list item with a Preserved Block followed by another block is always loose.
- **Empty paragraphs** have no Markdown form. Export omits them, and merge keeps them (see below).
- **A list item's empty leading paragraph** is not written when other blocks follow it. Import restores it.
- **Soft line breaks** import as spaces.
- **Trailing hard breaks** at the end of a paragraph are dropped, and a hard break inside a heading imports as nothing.
- **Unsupported Markdown is literal text.** Task lists, images, inline HTML and a footnote with no link definition import as the characters the author wrote, escaped on export. An image keeps its alt text and URL and loses its title. Entities import as the characters they name.
- **Reference definitions are consumed.** A definition is consumed only when it parses as a link definition: one token as its destination, and an optional title. A definition that a link uses resolves into that link, which imports as an inline link, within the [limits on link targets](#limits-on-link-targets). A definition that no link uses is dropped, and its text is lost. A footnote definition such as `[^1]: note` parses as a link definition, so `a[^1]` with that definition exports as `a[^1](note)`. Any other footnote, such as `[^1]: a longer note`, stays literal text.
- **A table's resolved links get a paragraph.** A table keeps its lines, so a cell's `[x][r]` cannot hold the target of a definition. Import adds one paragraph directly after the table that holds each Link and Image of the table that a definition resolved, in cell order, as `[x](target)`. This is text the author did not write. For example, `| a |\n|---|\n| [x][r] |\n\n[r]: https://x.test/page\n` exports as `| a |\n|---|\n| [x][r] |\n\n[x](https://x.test/page)\n`. A table with no resolved link gets no paragraph. The same applies in a list item, in a blockquote, and to a table that falls back to literal text.
  - The cell still reads `[x][r]` outside the app, and its target is in the next paragraph.
  - The export holds no definition, so a later import adds nothing. The paragraph is ordinary content, and a revision can remove it.
  - A revision that brings the definition back adds no second paragraph when the same links already follow the table.
  - A link with no text uses its URL as its text.
  - A cell past the header's width is not parsed, so its reference resolves to nothing. If no other link uses the definition, the definition is dropped with its target.
- **A Preserved Block's edges and line endings.** `\r\n` and `\r` in a source become `\n`. The first line loses its indentation, and trailing blank lines are dropped. Whitespace at the end of the final line stays, as on any other line. Front matter is the exception: whitespace after a delimiter means the block is not front matter, and it imports as it did before Preserved Blocks. Export puts a blank line between a Preserved Block and the next block.
- **A paragraph line that would make a table.** Hard breaks can put a delimiter row such as `|---|` directly under a line of text. Export writes a backslash before its first character, so the paragraph stays a paragraph.
- **Reference-style links** import as inline links.
- **Other syntax forms** import as their canonical equivalents: setext headings, `*` and `+` bullets, `1)` lists, `__bold__`, `_em_`, `~strike~`, indented code, `~~~` fences and two-space hard breaks.

The canonical corpus (`backend/tests/fixtures/markdown-corpus/`) records these normalisations. It has 22 byte-stable canonical documents and 40 agent-style inputs, each with its expected canonical export.

### Limits on link targets

Three limits can lose the target of a link in imported Markdown. Nothing signals the loss. `context/.delta/DELTA-023-preserved-blocks-accepted-tech-debt.md` records the evidence.

- **Comrak's reference budget.** On each successful lookup of a definition, comrak 0.57.0 charges the bytes of the URL and the title against a budget. The budget is the body's size in bytes, up to 100 000, and front matter does not count towards it. The budget cannot be configured.
  - A lookup that does not fit resolves to nothing and charges nothing, so a later, shorter URL can still resolve.
  - A reference that does not resolve exports as escaped literal text. For example, three uses of a 100-byte URL in a 199-byte document export as one link and two `\[x\]\[r\]`.
  - The first resolved use of a label keeps the target. A label whose first use does not fit counts as unused, so its definition and its target are lost.
  - A table's links paragraph holds only the references that resolved.
- **A destination the writer cannot carry.** A destination with a space keeps the link's text and loses its target: `a [x](<b c>) d` exports as `a x d`, and `[r]: <b c>` puts only `x` in a table's links paragraph. An inline link in a table that keeps its lines stays in the cell. The fallback applies to the whole paragraph: one destination with a space or a tab also removes the target of every other link and the inline code in that paragraph, including in a table's links paragraph.
- **A reference past the header's width.** The cell is not parsed, so the reference resolves to nothing. If no other link uses the definition, its target is lost.

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
3. In each gap between aligned blocks, leftover blocks of the same kind are paired by position and edited in place. Text is edited by character diff, formatting by mark and unmark ranges, and attributes only where they differ. A Preserved Block is edited as text; one of another `kind` is replaced. List items pair with list items, and their blocks are reconciled recursively.
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
- **Concurrent inserts above front matter pause the Document.** Two Installations that add front matter concurrently, or one that adds it while another inserts a first block, converge on a tree where front matter is not first. That tree is schema-invalid, so ADR 0029's import failure applies on every Installation that receives both records. One Installation that merges against a stale base is refused instead, because it checks the result before it appends. See ADR 0042.

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
  - P8: concurrent edits to one table both survive, in one block.
  - P9: a Preserved Block source of any shape exports stably, with no content lost.
  - P10: a table's link through a reference definition keeps its target, and bringing the definition back changes nothing.
- The `CASES` environment variable sets the scale, with a default of 3 000. Merge properties run a third of it. The long run is `CASES=60000 cargo test -p teamarness-documents-package --release --test markdown_properties`.
- `backend/tests/markdown_corpus.rs` checks the corpus.
- `packages/documents-package/fixtures/merge-scenarios.json` is the shared merge scenario table. The backend runs it against `merge`, and the Documents port system test runs it through `document.revise`.
