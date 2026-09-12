# 02-system/07-shell — Shell Requirements

Prefix: `RES.SYS.SHELL`

*Requirements for this subsystem will be authored during Phase 2 scoping. The Tauri shell owns package mounting in webviews, workspace controls, workspace-level theming, and the desktop window lifecycle. It communicates with the Rust runtime through Tauri commands and events. The shell supplies generic core capabilities (identity, membership) to packages through the Tauri invoke/event bridge.*

## RES.SYS.SHELL-R01 — Tauri Desktop Host

The system SHALL use Tauri v2 as the desktop shell. The shell SHALL manage the main window, system tray, native menus, and auto-updater. Application behavior SHALL live in the Rust runtime, not in the shell layer.

## RES.SYS.SHELL-R02 — Package Webview Mounting

The shell SHALL mount package UIs in Tauri webviews. Each package SHALL render into a host-owned mount point. The shell SHALL manage package activation, deactivation, and disposal through a standard lifecycle.

## RES.SYS.SHELL-R03 — Theming

The shell SHALL define workspace-level theme tokens as CSS custom properties on the package mount ancestor. Packages SHALL inherit theme tokens without receiving theme events. The shell SHALL NOT hard-code package-specific styles.