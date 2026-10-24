# DELTA-013 — Documents editor edge cases accepted as tech debt

**Status:** open (2026-10-25). Confirmed by the fourth Verify of the rendered Document editing workstream; the developer accepted them as tech debt, not fixed.

## Divergence

`TEAM.SYS.DOCS-R09` and `TEAM.SYS.DOCS-R13` expect the editor to keep every local change, undo and redo within one record before it commits. These cases need pathological content or rare timing, and break that expectation:

1. **The size estimate ignores mark attributes and inline runs (SF-301, CR-301).** Below the 32 KiB measuring threshold, `estimateNodeBytes` charges a flat 24 bytes per mark and ignores attribute values, and `estimateStepBytes` counts a mark step once per text block, not once per run. Pasted link addresses are not bounded, and a pasted `javascript:` address is not refused. A paste of many runs with long links, a link set over many paragraphs, or bold over many runs can produce an update over 160 KiB. The Backend refuses it (R05), and the session reports the lost changes. An undo of such content can also exceed one record.
2. **The undo text budget is a running maximum (CR-302).** Each stack's text growth only grows, and resets only when the stack is empty at the next undo or redo. In a Document of more than about 110 KiB of text, after the undo of a very large change, later ordinary undos can be refused as `document_too_large` until the stack empties.
3. **Concurrent open leaks a session.** `openDocumentById` has no guard after it awaits `closeDocument()`. A second open during the close window starts a session that is never disposed, and the earlier open wins. No edits are lost.
4. **Retained review findings (P2).**
   - SM-302 and SF-304: the UI treats "holds a doc" as `loro !== null`, which is wider than the recorded definition. No incorrect behaviour is shown.
   - SM-303: no test pins `estimateRevertBytes` as an upper bound (related to item 1).
   - SM-304: revert measurement is duplicated in `undo.ts` and `guard.ts`; the `session.test.ts` fake Backend uses a looser resume rule; the seeded `random` helper is duplicated.
   - SM-305: unused `personPace` scaffolding, review IDs in test names, and a hard-coded excluded origin in one test.
   - SM-306: R13 names `LoroUndoPlugin` rather than the guarded undo and redo commands.
   - SF-303: the session model test's fake lag never drops events, so it cannot catch a reopen that omits missed updates.

## Impact

Items 1 and 2 need content that ordinary editing does not produce. The Backend still enforces the record bound, so no oversized record is appended, and a lost change is reported, not silent. Item 3 needs a click during the short close window and loses nothing. Item 4 has no observed user effect.

## Follow-up

Bound and count mark attributes per run in the estimate, bound pasted link addresses with the same rules as the link prompt, and add a test that the estimate bounds the real update for marked content. Recompute the undo text budget from the steps still on each stack. Guard `openDocumentById` against a newer open after the close. Close this delta when those changes and tests pass.

## Tracking

- `TEAM.SYS.DOCS-R09`, `TEAM.SYS.DOCS-R13`; ADR 0031
- `packages/documents-package/ui/src/editor/{guard,undo}.ts`, `packages/documents-package/ui/src/controller.ts`
- `rendered-document-editing` workstream: the fourth Verify report and its review artifact
