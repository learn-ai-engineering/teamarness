# 02-system/07-shell — Shell Requirements

Prefix: `TEAM.SYS.SHELL`

The Tauri Shell owns Workspace controls, the desktop window lifecycle, Package UI Frame mounting, and Workspace-level theming. It communicates with the Rust runtime through typed, secret-free Tauri commands and events.

Phase 2 delivered the desktop window and Workspace onboarding between two Installations. Phase 3 added Package mounting, Package Process lifecycle coordination, theming, and bounded failure handling for trusted Packages from application-bundled and installation-local Package Sources. Workspace leave, Member removal controls, manager administration, system tray behavior, native menus, and auto-updater behavior are unscheduled.

## TEAM.SYS.SHELL-R01 — Tauri Desktop Host

The system SHALL use Tauri v2 as the desktop shell. The shell SHALL manage the main window. System tray, native menus, and auto-updater are unscheduled; see `roadmap.md` Later (unscheduled). Application behavior SHALL live in the Rust runtime, not in the shell layer.

## TEAM.SYS.SHELL-R02 — Package UI Frame Mounting

Beginning in Phase 3, the Shell SHALL mount Package UIs, and it SHALL mount each Package UI in its own sandboxed frame inside the Shell webview, with scripts allowed and same-origin access, popups and top-level navigation denied (ADR 0015). The frame document and its runtime SHALL be Shell-owned. No Package code or Package-controlled string SHALL execute in the Shell's JavaScript realm. Each Package UI SHALL render into a Shell-owned right-pane Package UI Mount. Package navigation and Workspace context changes SHALL use generic contracts without product-specific branches. Hiding a Package UI or selecting another Workspace SHALL NOT itself suspend that Package Instance's Backend. Frame isolation separates realms, DOM, storage and Tauri IPC access; it does not contain a Package UI that blocks the shared web content thread.

## TEAM.SYS.SHELL-R03 — Theming

Beginning in Phase 3, the Shell SHALL deliver Workspace-level theme token values to each Package UI Mount through its frame bridge, and the frame runtime SHALL define them as CSS custom properties on the frame document root; Packages SHALL receive updated values without receiving theme events. `@teamarness/package-ui-sdk` SHALL be the sole authority for the exact frozen token-name list and its derived TypeScript token-name type, and the Shell SHALL provide that shared list in every Package mount context. The SDK MAY install its optional, class-scoped CSS kit inside a Package Shadow Root; the kit SHALL use only the public theme tokens, SHALL leave Package composition opt-in, and SHALL provide overlapping-safe idempotent cleanup. The Shell SHALL NOT hard-code Package-specific styles.

## TEAM.SYS.SHELL-R04 — Workspace Controls

The shell SHALL provide workspace creation, join, leave, member-list, invitation, and revocation controls backed by the core runtime's public interfaces. Phase 2 SHALL provide workspace creation and listing, invitation creation and copying, invitation acceptance by paste, local workspace join, and current member display. Workspace leave, member removal controls, and manager administration are deferred. The shell SHALL NOT reimplement identity or membership rules.

## TEAM.SYS.SHELL-R05 — Generic Runtime Bridge

The Shell SHALL expose only generic, typed contracts over Tauri invoke and event channels. It SHALL NOT expose secrets, database handles, Core Runtime types, or raw transport handles. Package UI requests, responses, subscriptions, and events SHALL use Package-owned contracts routed through the generic bridge without product-specific branches. A Package UI SHALL reach Package-owned contracts only through a mount-scoped frame bridge whose identity comes from the Shell-held channel, and SHALL NOT reach Tauri invoke or event channels.

## TEAM.SYS.SHELL-R06 — Package Error Containment

Beginning in Phase 3, a returned Package Backend error, lifecycle task failure, Package Process exit, missed execution deadline, failed mount, terminal Package UI stream error, or JavaScript exception in a trusted Package SHALL NOT terminate the Shell or Core Runtime. A terminal Package UI stream error owned by Teamarness SHALL cross the generic desktop bridge, close the affected stream, and cause the Shell to present Package UI Mount recovery without stopping or recovering the Package Backend. A stream that the Package Backend completes or closes with a Package-owned error code SHALL be delivered to the subscribing Package UI, with that code, and SHALL NOT by itself cause Package UI Mount recovery. For other failures, the Shell SHALL present the failure and allow Teamarness to recover or dispose the Package Instance according to its lifecycle state. This requirement does not promise safe execution of hostile code, an operating-system sandbox, or recovery from system-wide resource exhaustion.
