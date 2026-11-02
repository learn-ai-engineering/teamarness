# 0042 — Markdown the editor cannot carry is kept as Preserved Blocks, and Documents re-evaluate after a format upgrade

**Status:** accepted (2026-11-02)
**Amends:** ADR 0032 (**Body model**: the fixed schema gains a node; **Markdown**: tables, front matter and HTML blocks are no longer imported as literal text)
**Refines:** ADR 0029 (Consequences: "an explicit future repair or compatible reimport path" is now built for format upgrades)

## Context

ADR 0032 makes the Documents backend the only Markdown authority, and its body schema has no table, front matter or HTML node. Import turned those constructs into literal text. A table became one escaped paragraph, and front matter became a horizontal rule and a heading. No characters were lost, but the line structure was, and it cannot be recovered. A census of 979 real planning documents found front matter in 636 files and tables in 538.

Filesystem mapping makes this visible to every Member, because a mapped file holds what the export writes. Real table nodes are a larger workstream that also needs editor behaviour.

A new schema node has a cost. An Installation on an older build reads a tree that holds the node as schema-invalid, and ADR 0029 pauses that Document with no way back.

## Decision

**Preserved Blocks.** The shared schema gains `preserved_block`, with an attribute `kind` (`table`, `front_matter` or `html`) and unmarked text as its content, like `code_block`. It holds the construct's exact source lines without any container prefix. It may sit wherever a `code_block` may, except that `front_matter` is only ever the first top-level block. Anywhere else, front matter is a schema violation, because it has no Markdown form there.

**Import and export.** The backend recognises the three constructs on every import path. The writer emits the source as raw text and never escapes it, so the export is byte-stable. The parser enables the table and front matter extensions, and the writer does not, so nothing else in the export changes.

**The fallback rule.** A construct becomes a Preserved Block only when its source is sound: it re-imports as the same block in each place it can move to. Otherwise it imports as literal text, as it did before this decision, and content is never lost. Export applies the same rule to a stored source, because a character-level merge of two valid sources can leave one that is no longer a table. Such a source is not a schema violation.

**What counts as front matter.** An opening `---` block with a blank line directly inside either delimiter is read as horizontal rules. Authors write rules that way, and the front matter extension alone would turn everything between two rules into one read-only block.

**Merge.** A Preserved Block is a text block. The same `kind` is edited in place by character diff, and another `kind` is replaced. New front matter is placed before any empty paragraph a revision cannot mention, and a merge that would leave a schema-invalid tree is refused.

**Upgrade re-evaluation.** Documents keeps a format generation. After an upgrade, an Installation re-evaluates once what its older build paused or quarantined, so a Document paused only because its build was old recovers without any action. A record that still fails stays failed.

## Rejected alternatives

- **Keep literal text.** It loses the line structure of tables and front matter for every Member.
- **Real table nodes now.** They need a schema, editor behaviour and merge rules of their own. A Preserved Block is a bridge: a later table node can convert from it.
- **Keep the literal text in the shared Document, and restore the source only in a mapped file.** The file is correct for the Member who mapped it, and the Document stays mangled for everyone else.
- **Show a Preserved Block rendered.** It needs a backend-rendered form and overlaps the future tables work.
- **Accept the older-Installation pause without re-evaluation.** Under ADR 0029 the Document stays paused after the upgrade.
- **Gate Preserved Blocks on peer versions.** No peer-version signal exists, and a gate adds a synchronization protocol.

## Consequences

- **Rollout.** An Installation on an older build pauses a Document that gains a Preserved Block, with ADR 0029's read-only warning, until it updates. Release notes must say so.
- **Existing Documents are not rewritten.** Existing literal-text tables stay literal until a revision of them is merged.
- **Concurrent inserts above front matter can pause a Document.** If two Installations add front matter concurrently, or one adds it while another inserts a first block, the converged tree is schema-invalid on every Installation. Re-evaluation does not cure this, because the tree still fails. A local merge against a stale base is refused before it appends, so this needs a real concurrent race. The editor must also refuse a local step that puts a block above front matter.
- **Task lists, images, inline HTML, footnotes and reference definitions stay literal text.** They belong to later workstreams.
- **Line endings and first-line indentation are normalised.** A source is byte-exact apart from those two, and `markdown-contract.md` records both.

## Evidence

- `packages/documents-package/backend/src/markdown/{schema,export,merge}.rs`; `context/02-system/09-documents/markdown-contract.md`.
- `backend/tests/markdown_properties.rs` runs the round-trip, fixed-point, no-loss, no-op, revision, block-identity and concurrency properties with Preserved Blocks in the generators, at the top level and nested. `markdown_corpus.rs`, `markdown_merge_scenarios.rs` and `cross_language.rs` pin the corpus, the shared scenarios and the tree format.
- A census over 985 Markdown files, `context/` plus the MyFlow workstream artifacts, found 10 971 table lines and 8 638 front matter lines, and none of them changed through import and export. The census is not committed. It ran once at the commit that adds the node.
- **Weaknesses.** The concurrent front matter race is reasoned from the schema rule and Loro list semantics, and no test exercises it across Installations. The census corpus is planning documents, so HTML blocks are thinly covered (15 files).

### Provenance (non-versioned)

- `filesystem-docs` workstream: the prototype census, the design, and the slice A plan.
