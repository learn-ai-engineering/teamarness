# DELTA-010 — The Conversations UI presents Messages newest-first

**Status:** closed (2026-09-19)

Resolved by the architecture-review workstream, remediation plan phase 6 Step 14 — Package contract fidelity and authoring experience.

## Divergence

`TEAM.SYS.CONV-R08` requires the newest Message at the bottom of the Channel, older Messages above it, and a Channel that opens scrolled to the bottom. The Conversations UI did not present Messages newest-at-bottom, default-scrolled-to-bottom as `TEAM.SYS.CONV-R08` requires.

The Backend returns a page ordered `ORDER BY m.order_key DESC` (`packages/conversations-package/backend/src/store/mod.rs`), and the controller appended each older page to the end of that array. The view rendered the array in order into an `<ol>` with no reversal and no `column-reverse` layout, so the newest Message rendered at the top. The view did scroll the history element to its bottom on first render, which landed on the oldest Messages rather than the newest.

## Impact

Reading order was inverted relative to ordinary messaging applications and relative to the stated requirement. Opening a Channel showed the oldest Messages of the newest page, so a Member did not see the latest Message without scrolling up. Convergent order itself was correct: every Installation derives the same sequence from Core order keys.

## Resolution

The controller turns every page it receives into oldest-first order (`oldestFirst` in `packages/conversations-package/ui/src/controller.ts`) and prepends an older page above the Messages already shown, so the newest Message is last. The view renders that order into the history list and scrolls to the bottom when a Channel opens or when the reader is already at the bottom, and keeps the reader's position when older Messages are added above (`packages/conversations-package/ui/src/view.ts`).

`packages/conversations-package/ui/src/index.test.ts` pins both halves: `renders the newest message at the bottom` and `opens a channel scrolled to the bottom`, each citing `TEAM.SYS.CONV-R08`.

## Tracking

- `TEAM.SYS.CONV-R08`
- Developer answers 2026-09-18, amendment A1
