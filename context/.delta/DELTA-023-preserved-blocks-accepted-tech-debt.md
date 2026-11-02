# DELTA-023 — Preserved Blocks findings accepted as tech debt

**Status:** open (2026-11-03). These findings from Verify of Filesystem Docs slice A are deferred, not fixed. The owner of each group is the follow-up named in it.

Don ruled on these:

- Round 2, 2026-11-02: defer CR2-001 and CR2-002, keep non-first front matter as a schema violation, fix SF2-001 with the links paragraph, and gate slice B on the dropped text.
- Round 3, 2026-11-03: defer CR3-001 and add it to the slice B gate, and accept the limit on a reference past the header's width.

The other findings are P2 findings that Verify retained without an individual ruling, because a P2 finding does not block.

**Slice B gate.** Three decisions come before the slice B plan: the dropped text (group 4), the reference budget (group 8), and CR2-002 (group 2).

## Divergence

Slice A of the Filesystem Docs workstream (`TEAM.SYS.DOCS-R11`, `R13`, ADR 0042) adds Preserved Blocks and upgrade re-evaluation. Verify round 2 confirmed three P1 findings. SF2-001 is fixed, and round 3 confirmed the fix. Round 3 confirmed one more P1 finding, CR3-001. CR3-001 and the other two round 2 findings are recorded below, with the reviews' other retained findings. The finding IDs are those in the workstream's review artifacts, `verify/20261102T041109Z_slice-a-review-1.md`, `verify/20261102T060000Z_slice-a-review-2.md` and `verify/20261103T000000Z_slice-a-review-3.md`. These `verify/` paths are in the `filesystem-docs` workstream folder, not in this repository. An evaluation-only diff that holds the tested fix ideas is `verify/20261102T060000Z_slice-a-candidate-fix.diff`.

### 1. A wide table with short rows costs far more than its bytes (CR2-001, regression)

- comrak fills short rows to the header's width, up to a private limit of 500 000 cells per table. `is_sound` parses each table in up to four placements on every export (`packages/documents-package/backend/src/markdown/export.rs`).
- Measured on an update of about 163 KB, at the 160 KiB limit of `MAX_UPDATE`, sent through `document.revise` by a Member's Consumer Package or a bound Agent:
  - The receiver's import took 12.13 s, for each later record.
  - The lock wait was 12.11 s. The lock covers all bodies, so every Document on that Installation waits.
  - A later revise took 71.8 s.
  - Memory peaked at about 2.4 to 2.6 GB.
  - `canonical` took 0.019 s before slice A.
- No data is lost, and ordinary tables are unaffected. A later measure of 50 ordinary tables, 20 rows each, took about 20 ms.
- Tested fix idea: scan before any parse with a budget of about 4 cells for each source byte. Above the budget, parse with the table extension off, and check the same budget in `is_sound`. On a scratch copy the same input took about 0.02 s. The idea is coarse, because every table in that text goes literal.

Follow-up: revisit before wider release.

### 2. Front matter with whitespace after a delimiter is restructured (CR2-002)

- comrak requires a newline straight after the delimiter, so `--- ` is not recognised. `"---\ntitle: X\ndate: Y\n--- \n\nBody\n"` exports as `"-----\n\n## title: X date: Y\n\nBody\n"`.
- This is not a regression, because the bytes are identical before slice A. No characters are lost. A scan of about 4 600 Markdown files found 0 occurrences. Acceptance criterion A-1 and the contract state the limit.
- Tested fix idea: trim the two delimiter lines in a view that only comrak sees, and take the source from the original lines. On a scratch copy the whitespace stayed byte-exact.

Follow-up: the slice B gate, because a mapped file can hold this shape.

### 3. Non-first front matter stays a schema violation

- Two Installations can add front matter concurrently, or one can add it while another inserts a first block. The converged tree is schema-invalid on every Installation, and ADR 0029 pauses the Document. Re-evaluation does not cure it, because the tree still fails.
- A local merge against a stale base is refused before it appends, and the editor refuses a local step of that shape. So this needs a real concurrent race, and no test exercises it across Installations.
- Don ruled on 2026-11-02 that it stays a schema violation. The alternative is to read non-first front matter as a source that is not sound, which gives literal text.

Follow-up: reopen if the race is seen in use.

### 4. Import drops Markdown text in a mapped file (decision needed before slice B)

- Import drops a reference definition that no link uses, including a footnote such as `[^1]: note`. A used one becomes an inline link, so `a[^1]` becomes `a[^1](note)`. The contract names everything else that import drops.
- In slice B, a first sync of a mapped file would delete this text from the author's file.
- SF2-002: acceptance criterion A-6 was false for one-token definitions. Its wording is amended and now states the real behaviour.

Follow-up: the slice B gate. Don's decision comes before the slice B plan.

### 5. Limits of the links paragraph (SF2-001 fix)

- A cell still reads `[x][r]` outside the app, with its target in the next paragraph.
- A reference in a cell past the header's width is not parsed, so its definition is unused and dropped, and its target is lost (SF3-002). This is a regression: `0119e2d` exports `| a | |---| | b | [x](https://x.test/page) |` for the input in `the_links_paragraph_has_its_stated_limits`. Don accepted the limit on 2026-11-03, and A-6 states it.
- A table that has any link pays for one more parse of its own lines. Measured: about 7 percent on 50 ordinary tables with links, and about 10 percent on the wide tables in group 1. A table with no link or image does not pay.

Follow-up: the richer-Markdown workstream, with real table nodes.

### 6. Correctness and test gaps

- **CR2-003.** `is_sound` ignores position. A sound table nested deep can make an export fail to re-import, and `document.revise` then returns a schema violation. Nothing aborts.
- **CR-004.** A Document that opens with a rule can have prose captured as read-only front matter. Fix: reject interior lines that are blank or that start with a block marker.
- **SM2-001.** The SF-003 correction is incomplete: the parser's doc comment in `ui/src/editor/preserved.ts` is stale, and a forged frame of a known kind has no test.
- **SM2-003.** Four of six hostile shapes in `tests/markdown_hostile_depth.rs` cannot detect a depth regression.
- **SM2-004.** A merge test's doc comment in the same file claims what the test does not check.
- **SM2-005.** A regression abort in the same file does not name the failing test.
- **SM2-006.** The `normalize` bound and its `Err` arm in `markdown/merge.rs` have no test, and no input is known to reach them.
- **SM2-010.** The record-abort test in `backend.rs` cites no requirement.
- **SM-007.** Property P9 omits front matter, although the contract says "any shape". Fix: add the kind.
- **SM-008.** The Rust half of "no `kind` reads as html" has no test. Fix: add one `schema.rs` unit test.
- **SF-005.** The plan's Verification Map promises footnote corpus evidence. `import_drops_what_the_contract_names` in `tests/markdown_corpus.rs` now covers footnote import, but no corpus pair exists.

Follow-up: the next Documents workstream.

### 7. Documentation and naming

- **SM-003.** ADR 0042's Context is a past-tense narrative. Fix: restate it as a present fact.
- **SM-004.** "Preserved Block" is missing from `context/ontology.md`. Fix: add one row.
- **SM-009.** A fixture name in `tests/cross_language.rs` depends on the corpus count, and UI tests hard-code it. Fix: use a name with no count.
- **SM2-007.** `reimport` in `markdown/export.rs` is misnamed where `parse_revision` calls it.
- **SM2-008.** `context/.decisions/README.md` cites one supersession three ways.
- **SM2-009.** The contract's Verification section omits `tests/markdown_hostile_depth.rs`.

Follow-up: slice A Close, or the next Documents workstream.

### 8. comrak's reference budget can lose a link target (CR3-001)

- On each successful lookup, comrak 0.57.0 charges `url.len() + title.len()` against `min(100000, body bytes)`. Front matter bytes are not counted. The limit cannot be configured, and 0.59.0 has the same limit.
- A lookup that does not fit resolves to nothing and charges nothing, so a later, shorter URL can still resolve. A reference that does not resolve exports as escaped literal text. The first resolved use of a label keeps the target. A label whose first use comes too late loses its target, and nothing signals the loss.
- Measured at `d214ae8`:
  - Three uses of a 100-byte URL resolve 1 of 3 in a 199-byte document, and 2 of 3 in a 200-byte document.
  - 12 prose uses of a 100-byte URL resolve 4 in a 450-byte document.
  - 50 uses of a 25-byte URL resolve all 50 in a 1 881-byte document.
- A verifier's scan of 4 699 Markdown files found 1 324 that use references, 0 over budget and 0 that lose a link.
- The core loss matches `0119e2d`. Slice A adds two regressions:
  - Front matter no longer counts towards the budget. With about 2 KB of front matter and 12 uses of a 100-byte URL in a 450-byte body, `0119e2d` resolves 12 and `d214ae8` resolves 4.
  - Repeated revises of an export plus its definitions can grow a table's Document until every link resolves. The verifier measured ops 771, 116, 214, 0 and lengths 1 578, 3 323, 6 594, then stable. An 8-row table with one 100-byte URL gives ops 118, 20, 36, 0 and lengths 313, 738, 1 587, then stable. On `0119e2d` that input gives ops 94, 0, and its length stays 322.
- Rejected fix: trailing newlines for the parse only. They write about 100 KB of blank lines into an unclosed fence, and no test suite catches it. Any fix needs tests for budget reuse and for an unclosed fence with `]:`.
- Prototype: `verify/20261103T000000Z_slice-a-budget-prototype.diff`, in the `filesystem-docs` workstream folder. It inserts one line of spaces after any BOM and front matter, in `parse_revision` and `parse` only.
  - The verifier ran it on 4 699 real files, 72 edge cases, 40 000 fuzz documents and the package suites. No expected output changed, and both regressions went away.
  - It is not checked by fmt or clippy, and it has no new tests. The 100 KB cap stays reachable.

Follow-up: the slice B gate.

### 9. Other Verify round 3 findings

- **SF3-001 (also CR3-003).** A link destination with a space loses its target, as at `0119e2d`: `a [x](<b c>) d` exports as `a x d`. The contract now states this limit.
- **CR3-002.** An export kept with its definition and a changed target leaves a stale paragraph on each revision. The new paragraph goes directly after the table, and the old one stays.
- **CR3-004.** A revision that adds a table's links paragraph and also edits the next paragraph pairs those two by position. A concurrent edit to the next paragraph lands in the links paragraph: a Person's `Next para. HUMAN` gives `[x](https://x.test/page) HUMAN`.
- **SM3-005.** Property P10 in `tests/markdown_properties.rs` passes if the table stops being a table, because it checks the link and not the table lines.
- **SM3-006.** In `fixtures/merge-scenarios.json`, the three names that start with "the same" depend on the row before them. The name that starts with "a table with its links paragraph" claims two merges, and its row runs one.
- **SM3-007.** The test in `tests/markdown_corpus.rs` that brings the definition back runs three identical rounds. The `Recognise::Checked` guard in `links_paragraph` has no comment.
- `markdown/mod.rs` says import "drops only what the contract names". The contract's list is what testing has found, so the code comment claims more than the contract does.

Follow-up: the next Documents workstream. CR3-002 and CR3-004 can wait for the richer-Markdown workstream, with real table nodes.
