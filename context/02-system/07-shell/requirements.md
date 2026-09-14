# 02-system/07-shell — Shell Requirements

Prefix: `TEAM.SYS.SHELL`

The Tauri shell owns workspace controls and the desktop window lifecycle. It will also own package mounting and workspace-level theming when the package foundation is added. It communicates with the Rust runtime through typed, secret-free Tauri commands and events.

Phase 2 delivers the desktop window and Workspace onboarding between two Installations. Phase 3 adds Package mounting, Package Process lifecycle coordination, theming, and bounded failure handling for trusted Packages from application-bundled and installation-local Package Sources. Workspace leave, Member removal controls, manager administration, system tray behavior, native menus, and auto-updater behavior are deferred beyond Phase 2.

## TEAM.SYS.SHELL-R01 — Tauri Desktop Host

The system SHALL use Tauri v2 as the desktop shell. The shell SHALL manage the main window and, in a later phase, the system tray, native menus, and auto-updater. Application behavior SHALL live in the Rust runtime, not in the shell layer.

## TEAM.SYS.SHELL-R02 — Package Webview Mounting

Beginning in Phase 3, the Shell SHALL mount Package UIs in Tauri webviews. Each Package UI SHALL render into a Shell-owned right-pane Package UI Mount. Package navigation and Workspace context changes SHALL use generic contracts without product-specific branches. Hiding a Package UI or selecting another Workspace SHALL NOT itself suspend that Package Instance's Backend.

## TEAM.SYS.SHELL-R03 — Theming

Beginning in Phase 3, the shell SHALL define workspace-level theme tokens as CSS custom properties on the package mount ancestor. Packages SHALL inherit theme tokens without receiving theme events. The shell SHALL NOT hard-code package-specific styles.

## TEAM.SYS.SHELL-R04 — Workspace Controls

The shell SHALL provide workspace creation, join, leave, member-list, invitation, and revocation controls backed by the core runtime's public interfaces. Phase 2 SHALL provide workspace creation and listing, invitation creation and copying, invitation acceptance by paste, local workspace join, and current member display. Workspace leave, member removal controls, and manager administration are deferred. The shell SHALL NOT reimplement identity or membership rules.

## TEAM.SYS.SHELL-R05 — Generic Runtime Bridge

The Shell SHALL expose only generic, typed contracts over Tauri invoke and event channels. It SHALL NOT expose secrets, database handles, Core Runtime types, or raw transport handles. Package UI requests, responses, subscriptions, and events SHALL use Package-owned contracts routed through the generic bridge without product-specific branches.

## TEAM.SYS.SHELL-R06 — Package Error Containment

Beginning in Phase 3, a returned Package Backend error, lifecycle task failure, Package Process exit, missed execution deadline, failed mount, terminal Package UI stream error, or JavaScript exception in a trusted Package SHALL NOT terminate the Shell or Core Runtime. A terminal Package UI stream error SHALL cross the generic desktop bridge, close the affected stream, and cause the Shell to present Package UI Mount recovery without stopping or recovering the Package Backend. For other failures, the Shell SHALL present the failure and allow Teamarness to recover or dispose the Package Instance according to its lifecycle state. This requirement does not promise safe execution of hostile code, an operating-system sandbox, or recovery from system-wide resource exhaustion.
