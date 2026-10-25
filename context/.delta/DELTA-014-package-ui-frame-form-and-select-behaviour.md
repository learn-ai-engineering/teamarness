# DELTA-014 — Some Package UI controls misbehave under the frame policy

**Status:** open (2026-10-25). Found by the rendered Document editing frame scenarios; not yet fixed.

## Divergence

`TEAM.SYS.DOCS-R04`, `TEAM.SYS.DOCS-R13` and ADR 0015 expect Package UI controls to work inside a sandboxed `allow-scripts` frame under the frame document policy (ADR 0031). Three browser behaviours break that for some controls:

1. **`<select>` under the frame policy (WebKit).** WebKit reports a `style-src-attr` Content-Security-Policy violation whenever a `<select>` element is created in a frame under the policy. The rendered editor's block-style control uses buttons to avoid it. The Documents move control (a `<select>` of Collections) has the same latent issue. No frame scenario reaches it, so its visible effect is unobserved.
2. **Form submission in sandboxed frames (Chromium).** Without the `allow-forms` sandbox token, Chromium does not fire `submit` for a form in the frame, as the HTML specification requires. WebKit still fires it. The link prompt's Apply button and the Comment button act on click, with `submit` kept as a fallback. The other inline forms in the Documents UI, such as create and rename, still rely on `submit`. They work in the packaged WKWebView, but would fail in a Chromium-based frame.
3. **Top navigation (WebKit).** WebKit may refuse a sandboxed frame's top navigation without throwing. The isolation probe's `top-navigation-blocked` check therefore also passes when the frame is still running one second after the attempt. An admitted navigation would replace the Shell document, and the smoke test would then fail on its result timeout instead of on a named check.

## Impact

The packaged app runs on WKWebView, so items 2 and 3 do not affect current users. Item 1 can affect the Documents move control on the shipped platform. A future Chromium-based or other non-WebKit port inherits item 2 for every Package UI that relies on `submit`.

## Follow-up

Replace the remaining `<select>` controls with button or listbox controls that create no `<select>`, and make every Package UI form act on its button's click (or a shared SDK helper), not only on `submit`. Do not add `allow-forms`: `form-action 'none'` and the single `allow-scripts` token are part of ADR 0015's isolation. Add a frame scenario that exercises the Documents move control and an inline create or rename form in Chromium. Close this delta when those scenarios pass in WebKit and Chromium. Item 3 needs no change unless the probe's timeout outcome proves ambiguous in practice.

## Tracking

- `TEAM.SYS.DOCS-R04`, `TEAM.SYS.DOCS-R13`; ADRs 0015 and 0031
- `packages/documents-package/ui/frame-scenarios/` (`npm run verify:documents-editor`); `scripts/ui-isolation-probe/ui/index.js`
- `rendered-document-editing` workstream: Phase 4 and Phase 6 findings in the plan's Context Checkpoint
