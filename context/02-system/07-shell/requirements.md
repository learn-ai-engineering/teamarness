# 02-system/07-shell — Shell Requirements

Prefix: `TEAM.SYS.SHELL`

The Tauri shell owns workspace controls and the desktop window lifecycle. It will also own package mounting and workspace-level theming when the package foundation is added. It communicates with the Rust runtime through typed, secret-free Tauri commands and events.

Phase 2 delivers the desktop window and workspace onboarding between two installations. Phase 3 adds package mounting, package lifecycle coordination, theming, and bounded failure handling for trusted bundled packages. Workspace leave, member removal controls, manager administration, system tray behavior, native menus, and auto-updater behavior are deferred beyond Phase 2.

## TEAM.SYS.SHELL-R01 — Tauri Desktop Host

The system SHALL use Tauri v2 as the desktop shell. The shell SHALL manage the main window and, in a later phase, the system tray, native menus, and auto-updater. Application behavior SHALL live in the Rust runtime, not in the shell layer.

## TEAM.SYS.SHELL-R02 — Package Webview Mounting

Beginning in Phase 3, the shell SHALL mount package UIs in Tauri webviews. Each package SHALL render into a host-owned mount point. The shell SHALL coordinate package `activate`, `suspend`, `recover`, and `dispose` operations through the standard lifecycle.

## TEAM.SYS.SHELL-R03 — Theming

Beginning in Phase 3, the shell SHALL define workspace-level theme tokens as CSS custom properties on the package mount ancestor. Packages SHALL inherit theme tokens without receiving theme events. The shell SHALL NOT hard-code package-specific styles.

## TEAM.SYS.SHELL-R04 — Workspace Controls

The shell SHALL provide workspace creation, join, leave, member-list, invitation, and revocation controls backed by the core runtime's public interfaces. Phase 2 SHALL provide workspace creation and listing, invitation creation and copying, invitation acceptance by paste, local workspace join, and current member display. Workspace leave, member removal controls, and manager administration are deferred. The shell SHALL NOT reimplement identity or membership rules.

## TEAM.SYS.SHELL-R05 — Generic Runtime Bridge

The shell SHALL expose only generic, typed runtime contracts over Tauri invoke and event channels. It SHALL NOT expose secrets, database handles, core-internal types, or raw transport handles. When package support is added, the same boundary SHALL expose generic capability contracts without product-specific branches.

## TEAM.SYS.SHELL-R06 — Package Error Containment

Beginning in Phase 3, a returned backend error, lifecycle task failure, unwind-safe panic caught at a host boundary, failed mount, or JavaScript exception in a trusted bundled package SHALL NOT terminate the shell or core runtime. The shell SHALL present the failure and allow the host to recover or dispose the package according to its lifecycle state. This requirement does not promise containment of process aborts, memory exhaustion, undefined behavior, deadlocks, or hostile in-process code.
