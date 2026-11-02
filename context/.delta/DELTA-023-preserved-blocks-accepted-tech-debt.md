# DELTA-023 — Preserved Blocks findings accepted as tech debt

**Status:** open (2026-11-02). Don accepted these dispositions during Verify round 2 of Filesystem Docs slice A, on 2026-11-02. They are deferred, not fixed. The owner of each group is the follow-up named in it.

## Divergence

Slice A of the Filesystem Docs workstream (`TEAM.SYS.DOCS-R11`, `R13`, ADR 0042) adds Preserved Blocks and upgrade re-evaluation. Verify round 2 confirmed three P1 findings. SF2-001 is fixed. The other two are recorded below, with the review's other retained findings. Verify of the fix is still to come. The finding IDs are those in the workstream's review artifacts, `verify/20261102T041109Z_slice-a-review-1.md` and `verify/20261102T060000Z_slice-a-review-2.md`. An evaluation-only diff that holds the tested fix ideas is `verify/20261102T060000Z_slice-a-candidate-fix.diff`.

### 1. A wide table with short rows costs far more than its bytes (CR2-001, regression)

- comrak fills short rows to the header's width, up to a private limit of 500 000 cells per table. `is_sound` parses each table in up to four placements on every export (`packages/documents-package/backend/src/markdown/export.rs`).
- Measured on a 163 KiB update, under `MAX_UPDATE`, sent through `document.revise` by a Member's Consumer Package or a bound Agent:
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

Follow-up: slice B, where a mapped file can hold this shape.

### 3. Non-first front matter stays a schema violation

- Two Installations can add front matter concurrently, or one can add it while another inserts a first block. The converged tree is schema-invalid on every Installation, and ADR 0029 pauses the Document. Re-evaluation does not cure it, because the tree still fails.
- A local merge against a stale base is refused before it appends, and the editor refuses a local step of that shape. So this needs a real concurrent race, and no test exercises it across Installations.
- Don ruled on 2026-11-02 that it stays a schema violation. The alternative is to read non-first front matter as a source that is not sound, which gives literal text.

Follow-up: reopen if the race is seen in use.

### 4. Import drops Markdown text in a mapped file (decision needed before slice B)

- Import drops a reference definition that no link uses, including a footnote such as `[^1]: note`. A used one becomes an inline link, so `a[^1]` becomes `a[^1](note)`. The contract names everything else that import drops.
- In slice B, a first sync of a mapped file would delete this text from the author's file.
- SF2-002: acceptance criterion A-6 was false for one-token definitions. Its wording is amended and now states the real behaviour.

Follow-up: Don's decision before the slice B plan.

### 5. Limits of the links paragraph (SF2-001 fix)

- A cell still reads `[x][r]` outside the app, with its target in the next paragraph.
- A reference in a cell past the header's width is not parsed, so its definition is unused and dropped, and its target is lost.
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
