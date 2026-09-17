# DELTA-004 — Package UIs are not isolated from the Shell

**Status:** open (2026-09-18)

## Divergence

ADR 0003 says "The Tauri webview boundary provides process-level isolation for package UIs". `TEAM.SYS.SHELL-R02` and `TEAM.SYS.PKG-R04` say the Shell mounts Package UIs "in Tauri webviews". Each Package UI module is loaded with `import()` into the Shell's own page and rendered in an open Shadow Root. It shares the JavaScript realm, the DOM, and the full Tauri command set of the `main` window.

## Impact

Any Package UI can call Shell-only commands, such as creating a Workspace Invitation or disposing another Package, and can freeze the Shell. The README already states this.

## Resolution plan

The architecture-review workstream (see `roadmap.md` Current work), remediation plan phase 5 — Package UI isolation. A sandboxed iframe per mount with a mount-scoped message bridge was decided on 2026-09-17. The ADR 0003 UI-isolation clause is superseded or reaffirmed then.

## Tracking

- Review findings SEC-04, ARC-03, CTX-03 (UI part)
- `TEAM.SYS.SHELL-R02`, `TEAM.SYS.PKG-R04`, ADR 0003
