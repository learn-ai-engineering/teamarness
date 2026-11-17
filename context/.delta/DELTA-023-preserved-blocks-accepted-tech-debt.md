# DELTA-023 — Preserved Blocks findings accepted as tech debt

**Status:** open (2026-11-03; updated at slice A Close and in slice B Phase 1, 2026-11-15, and in slice B Phase 6 and at slice B Close, 2026-11-16). Groups 1–10 hold findings from Verify of Filesystem Docs slice A, and group 11 holds findings from slice B. They are deferred, not fixed. The owner of each group is the follow-up named in it.

Don ruled on these:

- Round 2, 2026-11-02: defer CR2-001 and CR2-002, keep non-first front matter as a schema violation, fix SF2-001 with the links paragraph, and gate slice B on the dropped text.
- Round 3, 2026-11-03: defer CR3-001 and add it to the slice B gate, and accept the limit on a reference past the header's width.
- Slice A Close, 2026-11-15: CR2-002 is ordinary tech debt, not part of the slice B gate. Fix the round 4 wording findings at Close, and defer SM4-005 and SM4-006 (group 10).
- Slice B plan, 2026-11-15: gate 1 (group 4), refuse lossy mapped files through a loss check; gate 2 (group 8), adopt the budget prototype with tests.

The other findings are P2 findings that Verify retained without an individual ruling, because a P2 finding does not block.

**Slice B gate.** Decided 2026-11-15 and implemented in slice B Phase 1: the dropped text (group 4) and the reference budget (group 8).

## Divergence

Slice A of the Filesystem Docs workstream (`TEAM.SYS.DOCS-R11`, `R13`, ADR 0042) adds Preserved Blocks and upgrade re-evaluation. Verify round 2 confirmed three P1 findings. SF2-001 is fixed, and round 3 confirmed the fix. Round 3 confirmed one more P1 finding, CR3-001. CR3-001 and the other two round 2 findings are recorded below, with the reviews' other retained findings. The finding IDs are those in the workstream's review artifacts, `verify/20261102T041109Z_slice-a-review-1.md`, `verify/20261102T060000Z_slice-a-review-2.md`, `verify/20261103T000000Z_slice-a-review-3.md` and `verify/20261103T010000Z_slice-a-review-4.md`. These `verify/` paths are in the `filesystem-docs` workstream folder, not in this repository. An evaluation-only diff that holds the tested fix ideas is `verify/20261102T060000Z_slice-a-candidate-fix.diff`.

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

Follow-up: ordinary tech debt (Don, 2026-11-15). Slice B did not fix it. A mapped file with this shape loses no token, so the loss check lets it through and it imports restructured, as in the app.

### 3. Non-first front matter stays a schema violation

- Two Installations can add front matter concurrently, or one can add it while another inserts a first block. The converged tree is schema-invalid on every Installation, and ADR 0029 pauses the Document. Re-evaluation does not cure it, because the tree still fails.
- A local merge against a stale base is refused before it appends, and the editor refuses a local step of that shape. So this needs a real concurrent race, and no test exercises it across Installations.
- Don ruled on 2026-11-02 that it stays a schema violation. The alternative is to read non-first front matter as a source that is not sound, which gives literal text.

Follow-up: reopen if the race is seen in use.

### 4. Import drops Markdown text in a mapped file (decision needed before slice B)

- Import drops a reference definition that no link uses, including a footnote such as `[^1]: note`. A used one becomes an inline link, so `a[^1]` becomes `a[^1](note)`. The contract lists the other drops that testing has found. The list is not a proof that nothing else is dropped.
- In slice B, a first sync of a mapped file would delete this text from the author's file.
- SF2-002: acceptance criterion A-6 was false for one-token definitions. Its wording is amended and now states the real behaviour.

**Decided (Don, 2026-11-15): refuse lossy files.** `markdown::lost_tokens` (`packages/documents-package/backend/src/markdown/loss.rs`) reports the tokens an import would lose, and folder sync refuses a mapped file for which it reports any. The file stays byte-identical. Import itself still drops this text; the check is what keeps it out of an author's file. The Markdown contract defines the check and its conservatism.

Done in slice B: the reconciler calls the check before every import and pre-check, and the settings view's pre-mapping report lists each lossy file with a sample of its lost tokens (ADR 0041, `TEAM.SYS.DOCS-R14`). The check also exempts ordered-list item numbers, because export renumbers `1.` `1.` `1.` as `1.` `2.` `3.`.

Follow-up: none.

### 5. Limits of the links paragraph (SF2-001 fix)

- A cell still reads `[x][r]` outside the app, with its target in the next paragraph.
- A reference in a cell past the header's width is not parsed. If no other link uses its definition, the definition is dropped and its target is lost (SF3-002). This is a regression: `0119e2d` exports `| a | |---| | b | [x](https://x.test/page) |` for the input in `the_links_paragraph_has_its_stated_limits`. Don accepted the limit on 2026-11-03, and A-6 states it.
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

Follow-up: the next Documents workstream. Slice A Close did not take these.

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

**Decided (Don, 2026-11-15): prototype adopted.** Slice B Phase 1 lands it in `markdown/export.rs` (`with_budget`, `front_matter_end`), checked by fmt and clippy, with tests in `tests/markdown_corpus.rs` for budget reuse, front matter, growth, an unclosed fence with `]:` and the 100 000-byte cap. Both regressions are gone. A reference past the cap still loses its target; the loss check reports it for a mapped file.

Follow-up: none.

### 9. Other Verify round 3 findings

- **SF3-001 (also CR3-003).** A link destination with a space loses its target, as at `0119e2d`: `a [x](<b c>) d` exports as `a x d`. The contract now states this limit.
- **CR3-002.** An export kept with its definition and a changed target leaves a stale paragraph on each revision. The new paragraph goes directly after the table, and the old one stays.
- **CR3-004.** A revision that adds a table's links paragraph and also edits the next paragraph pairs those two by position. A concurrent edit to the next paragraph lands in the links paragraph: a Person's `Next para. HUMAN` gives `[x](https://x.test/page) HUMAN`.
- **SM3-005.** Property P10 in `tests/markdown_properties.rs` passes if the table stops being a table, because it checks the link and not the table lines.
- **SM3-006.** In `fixtures/merge-scenarios.json`, the three names that start with "the same" depend on the row before them. The name that starts with "a table with its links paragraph" claims two merges, and its row runs one.
- **SM3-007.** The test in `tests/markdown_corpus.rs` that brings the definition back runs three identical rounds. The `Recognise::Checked` guard in `links_paragraph` has no comment.
- `markdown/mod.rs` says import "drops only what the contract names". The contract's list is what testing has found, so the code comment claims more than the contract does.

Follow-up: the next Documents workstream. CR3-002 and CR3-004 can wait for the richer-Markdown workstream, with real table nodes.

### 10. Verify round 4 findings

Round 4 (`verify/20261103T010000Z_slice-a-review-4.md`) retained eight P2 findings. Slice A Close fixed the wording ones: SM4-001, SM4-002, SM4-003, SM4-004, SF4-002 and CR4-001. These remain:

- **SM4-005. Fixed in slice B Phase 1.** Three tests near the limits test in `tests/markdown_corpus.rs` pin the 100 000-byte cap and the space-destination behaviour.
- **SM4-006. Fixed in slice B Phase 1.** The Agent guide in `backend/src/guide.rs` tells Agents to write inline links, not reference definitions.
- **CR4-001, behaviour.** The contract now states it: one destination with a space or a tab removes every other link target and the inline code in its paragraph. The behaviour itself is part of the dropped text in group 4.

Follow-up: CR4-001's behaviour stays with group 4: the loss check refuses a mapped file that hits it.

### 11. Slice B folder sync findings (Verify rounds 1 and 2)

Slice B (`TEAM.SYS.DOCS-R14`, ADR 0041) passed Verify round 1 on `dd73c5b` and round 2 on `78e76f0`. Neither round found a P0 or P1 finding. The reviews are `verify/20261116T031118Z_slice-b-review-1.md` and `verify/20261116T050000Z_slice-b-review-2.md`. Corrective Phase 7 fixed CR-001, CR-002 and CR-003, together with SM-001, SM-002, SM-006, SM-007, SM-008 and SF-001. Don ruled the following tech debt on 2026-11-16:

- **CR2-001. Deleting the last synced file stops sync.** When the last or only synced file of a mapped folder is deleted, the mapping becomes `unavailable` instead of archiving the Document. This is the emptied-root guard that protects against iCloud eviction and empty mount points. Write-back for that Collection then stops, and the settings view shows "Folder unavailable" with no reason. Adding any `.md` file resumes sync. Fix it if Members run into it: show a hint for the reason, or apply the guard only when two or more files were mapped.
- **CR2-002. A recovered Linux folder may stay unavailable.** On Linux, an existing root that is `unavailable` may not recover until a restart: the reconcile is not retried, and the inotify watch can sit on an empty mount point or a dead inode. macOS is not affected, and Linux is not qualified.
- **CR2-003. Write-back stops on mounts that refuse chmod.** If copying permissions fails, the write-back stops. On CIFS, SMB or FUSE mounts that refuse chmod, in-app edits then never reach the file. Make the permission copy best effort.
- **SF2-001. R14 and ADR 0041 overstated the emptied-root rule (documentation corrected in slice C).** They said the rule applies to a root that "holds none of its synced files". The code applies it only when no `.md` file is found at all. The R14 Import clause also lacked the exception from CR2-001. R14 and ADR 0041 now state the exact zero-eligible-readable-files guard and subtree exception. CR2-001 behavior is unchanged.
- **CR-004. Reconciles cost too much.** A Document that stays pending, or an error that persists, rehashes the folder every second. Each of Teamarness's own writes triggers a full pass that reads the Document list once per Document (O(N²)).
- **Maintainability.**
  - SM-003: pass-through wrapper layers (`append_record`, `reopen_unopened`, `body_state`).
  - SM-004: `mapped_file_by_name_key` is unused.
  - SM-005: the splice block splitter copies the Markdown import options instead of using `import_options`.
  - SM2-001: the system test header said "on Linux CI". Slice C corrects it to local macOS qualification; CI still runs no Rust tests.
  - SM2-002: the case-only rename test cites R14 (Write-back, No lost text) instead of R14 (Safety).
  - SM2-003: the reconciler's module doc and ADR 0041's test list left out the emptied-root guard and the Phase 7 tests. Slice C's reconciler module doc now states the guard, and ADR 0041's evidence covers the zero-files and recursive tests. This documentation finding is corrected; unrelated maintainability debt remains deferred.
  - SM2-004: `DRAFT_SAVE_DELAY_MS` is exported but unused.
  - SM2-005: the settings page keeps `teamarness-page--readable` and then cancels its width in CSS.
- **Accepted rare risks.** Recorded in the slice B plan checkpoint:
  - A file edited at the moment it is removed loses the edit.
  - A crash after an import, followed by an offline edit, can drop a teammate's concurrent edit.
  - A link swapped in between the scan and the open is followed.
  - A remap or clear during a reconcile can write to the old folder.
  - A failed Create Collection from folder leaves an empty Collection.
  - A crash between `document.create` and the first revision leaves an empty Document.
- **Repeat archive by the same author.** This is a Documents limitation that slice B exposed; it is not caused by slice B. After an archive and a restore, the same author cannot archive the Document again, and deleting its mapped file does not archive it either. Another author can. The likely cause: `document.archive` and `document.restore` records carry only `(author, document)`, so a repeat record is identical to the first and is probably deduplicated by the record log. This is not confirmed. Don judged it a tolerable edge case.

Follow-up: the next Documents workstream, or a bug report if Members run into CR2-001 or the repeat archive in use.

### 12. Slice C accepted limitations and follow-up checks

Don accepted the Filesystem Docs outcome on 2026-11-17 after hands-on subfolder creation, filesystem edits appearing across app instances, renames and archives. He requested folders-first alphabetical display, then confirmed it after creating new Documents and folders. That UI-only follow-up passed all 162 Documents UI tests. Don explicitly chose focused tests and manual acceptance instead of another full validation or review round, approved local integration, and deferred the items below.

The cumulative technical review `verify/20261117T000228Z_slice-c-review-3.md` remains failed. All 15 automated commands passed at `1988351`, but independent verification confirmed CR-301 and CR-302. Acceptance does not mean those findings were fixed or falsified. The later UI ordering commit does not alter either mechanism.

- **CR-301 — A file replaces a tracked directory.** If a regular file occupies a previously tracked directory path, reconciliation can archive the former descendants. Restoring the directory can create new Documents instead of recovering the original mapping identities. The archived body history remains. Two real-caller tests reproduced this. Don accepts the unusual directory/file replacement scenario as a current limitation rather than requesting more recovery machinery.
- **CR-302 — Case-only directory move on case-sensitive storage.** Renaming a single directory from `Plans` to `plans` can leave its local move refused and subsequent target edits unsynchronized on case-sensitive lookup. This is independently confirmed by code tracing, not runtime reproduction on case-sensitive storage. The available test location equated case spellings; that case passed. Don accepts deferral and did not approve creating a disk image. Renaming through an intermediate name is not a verified repair for an already retained intent.
- **SM-201 — Uncertain-create instructions.** The Agent guide should explicitly search the whole Collection, including root and the requested folder, and inspect candidate identities before a deliberate new create. Creation may survive while placement fails. Automatic retry remains prohibited.
- **SM-202 — Conditional alias test.** The physical dual-directory test returns on any second-directory creation error. Distinguish an established unsupported-storage condition from unrelated setup failures and report physical noncoverage explicitly. Real-store ownership tests do not qualify simultaneous physical aliases.
- **SF-201 — Create comment certainty.** Scope the `principal_not_bound` certainty exception in `ports.rs` to the first/create append. The implementation correctly reports every post-create placement failure as uncertain; the broad comment needs correction.

**Manual details not separately established:** legacy preview/cancel/enable state preservation; a real-editor round trip preserving untouched blocks; populated-directory rename IDs and missing-parent creation; sentinel versus only-subtree deletion, restore and unmap; and named refusal/occupied-target byte preservation. Don's successful everyday use is not represented as execution of each original manual procedure. These are follow-up checks, not outstanding work in the closed workstream.

**Broader qualification remains open:** real provider eviction, physical dual-alias storage, case-sensitive recovery execution, Linux and Windows watchers, separate-network latency, historical executables and distribution signing. App/DMG build success was ad-hoc linker signing, not Developer ID or notarization. Existing zero-files, hash-only move, uncertain-create, ambiguous-intent, race and repeat-archive limits remain.

**Owner and destination:** Don or the next Documents maintainer, in a separately prioritized Documents maintenance/qualification scope. Revisit when actual use or a wider release justifies it; do not automatically restart the closed corrective loop. No further implementation is implied by recording these items.
