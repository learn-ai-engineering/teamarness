# DELTA-010 — The Conversations UI presents Messages newest-first

**Status:** open (2026-09-18)

## Divergence

`TEAM.SYS.CONV-R08` requires the newest Message at the bottom of the Channel, older Messages above it, and a Channel that opens scrolled to the bottom. The Conversations UI does not present Messages newest-at-bottom, default-scrolled-to-bottom as `TEAM.SYS.CONV-R08` requires.

The Backend returns a page ordered `ORDER BY m.order_key DESC` (`packages/conversations-package/backend/src/store/mod.rs`), and the controller appends each older page to the end of that array. The view renders the array in order into an `<ol>` with no reversal and no `column-reverse` layout, so the newest Message renders at the top. The view does scroll the history element to its bottom on first render, which lands on the oldest Messages rather than the newest.

## Impact

Reading order is inverted relative to ordinary messaging applications and relative to the stated requirement. Opening a Channel shows the oldest Messages of the newest page, so a Member does not see the latest Message without scrolling up. Convergent order itself is correct: every Installation derives the same sequence from Core order keys.

## Resolution plan

The architecture-review workstream (see `roadmap.md` Current work), remediation plan phase 6 — Package contract fidelity and authoring experience. The ordering and the default scroll position are fixed in the same change that types the Conversations UI requests from the contract. Presentation polish stays with the Shell and Package UI workstream; this is a contract-fidelity gap, not visual polish.

## Tracking

- `TEAM.SYS.CONV-R08`
