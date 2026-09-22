# DELTA-004 — Package UIs are not isolated from the Shell

**Status:** resolved (2026-09-18) by ADR 0015

## Divergence

ADR 0003 said "The Tauri webview boundary provides process-level isolation for package UIs". `TEAM.SYS.SHELL-R02` and `TEAM.SYS.PKG-R04` said the Shell mounts Package UIs "in Tauri webviews". Each Package UI module was loaded with `import()` into the Shell's own page and rendered in an open Shadow Root. It shared the JavaScript realm, the DOM, and the full Tauri command set of the `main` window.

## Impact

Any Package UI could call Shell-only commands, such as creating a Workspace Invitation or disposing another Package, and could freeze the Shell.

## Resolution

ADR 0015 supersedes ADR 0003 lines 48 and 95 and the Package UI clauses of ADR 0002. Each Package UI Mount runs in its own `<iframe sandbox="allow-scripts">` with a Shell-owned document and runtime, reaching the Shell only through a mount-scoped message bridge; no Package code executes in the Shell realm. `TEAM.SYS.SHELL-R02`, `TEAM.SYS.SHELL-R03`, `TEAM.SYS.SHELL-R05` and `TEAM.SYS.PKG-R04` state the isolated model, and the Shell command capability is scoped to the `main` webview.

The Shell realm stays free of Package code by lint rule and by `scripts/package-foundation-qualification.test.mjs`. The bridge contract tests live in `apps/teamarness-desktop/src/`, and `npm run verify:packaged-app` runs a Package UI Frame isolation probe against the real packaged application.

Frame isolation separates realms, DOM, storage and Tauri IPC. It is not process isolation, and a Package UI that blocks its thread still freezes the Shell.

## Tracking

- Review findings SEC-04, ARC-03, CTX-03 (UI part)
- `TEAM.SYS.SHELL-R02`, `TEAM.SYS.SHELL-R03`, `TEAM.SYS.SHELL-R05`, `TEAM.SYS.PKG-R04`, ADR 0015
