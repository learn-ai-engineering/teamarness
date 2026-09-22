# 0015 — Package UIs run in sandboxed frames

**Status:** accepted (2026-09-18)
**Supersedes:** ADR 0003 (line 48 "Package mounting and disposal in Tauri webviews"; line 95 "The Tauri webview boundary provides process-level isolation for package UIs"); ADR 0002 (line 17, the clause "Packages have TypeScript UIs that mount into Tauri webviews"; line 51, the Package UI part of how Package UIs reach the Tauri bridge, left open by ADR 0012)

## Context

A Package UI is TypeScript web content written by a Package author. The Shell is the only holder of the Tauri command surface: 19 commands that create Workspaces, issue invitations, and start, suspend and dispose Package Instances.

Tauri gates every command call on an invoke key that lives in a closure in the Shell's main frame. WKWebView exposes the raw `window.webkit.messageHandlers.ipc` entry point to every frame on a registered custom scheme, and Tauri classifies such a frame as a local origin with the Shell's own window and webview labels. A capability cannot therefore distinguish a subframe from the Shell. The one thing that keeps the key out of Package reach is that no Package code and no Package-controlled string executes in the Shell's JavaScript realm.

A sandboxed frame without `allow-same-origin` gets its own opaque origin, its own realm and its own storage partition, and cannot read its parent or the top window. A frame served from a custom scheme loads under a document Content-Security-Policy that the protocol handler sets, and the Shell's own policy does not propagate into it.

A frame shares the Shell's web content process and its main JavaScript thread. It separates realms, not threads.

## Decision

Each Package UI Mount runs in its own `<iframe sandbox="allow-scripts">`. The sandbox carries that one token: no same-origin access, no popups, no top-level navigation.

The frame document, its stylesheet and its runtime are Shell-owned. The Package resource protocol serves them from the reserved `__teamarness/` path of the mount's token URL, before the Package Host is asked for anything, and answers every response with a document policy that denies network access, nested frames, form submission, inline code and dynamic code.

The frame runtime is the Shell's own code. It builds the `PackageMountContext` inside the frame — a Shadow Root in the frame document, the Package and Workspace presentation, the theme token names, and a mount-scoped `PackageClient` — then loads the Package entrypoint and calls `mount(context)`. The Package-facing contract is unchanged.

The Shell and the runtime talk over one `MessagePort` per mount, transferred to the frame after its first load and to nothing else. The Shell derives mount identity from the port a message arrived on, never from message content; the frame never learns the mount id, the resource token or any host request id. The Shell validates every message against a strict schema with the Package Host's own payload bounds, and calls the Tauri `package_*` commands itself.

**No Package code and no Package-controlled string executes in the Shell's JavaScript realm.** The Shell loads no modules dynamically, installs no `window` listeners for Package events, and frames Packages rather than loading their scripts, styles, fonts or images.

Frame isolation separates realms, DOM, storage and Tauri IPC. It does not contain a Package UI that blocks the shared web content thread, and it is not process isolation.

## Rejected alternatives

### A child Tauri webview per Package UI (option B)

Rejected for now. A child webview does contain a runaway Package, because it gets its own web content process, and its command set can be scoped per webview. It also holds a complete Tauri IPC surface, so the access-control list becomes the only wall; it needs an unstable Tauri feature, native geometry synchronization, per-Package capability generation at runtime, and it cannot overlay the Shell. It remains the path if hang containment becomes required.

### Keeping the shared realm with an accepted risk

Rejected. A Package UI in the Shell realm can read the invoke key by wrapping `fetch`, and can call every Shell command. That is the finding this decision closes.

### A Package-provided frame document, or a frame runtime bundled into each Package

Rejected. Both make the private bridge protocol part of the Package's own bundle. Generated Packages vendor the SDK at creation time (ADR 0010), so every locally registered Package would pin a wire-protocol version and a Shell upgrade could break it. A Package-provided document would also make each author responsible for a correct policy and a correct bridge bootstrap.

### A per-Package host name on the Package scheme

Rejected as unnecessary. Without `allow-same-origin` every frame document already has a unique opaque origin, so storage, DOM and cookies are separate whatever the host name says.

## Consequences

- A Package UI reaches Teamarness only through its mount-scoped bridge. It has no Tauri command, no Shell DOM, no browser storage and no network; `fetch`, XHR and WebSocket are denied by the frame policy.
- The Shell-realm rule is permanent. Tauri's invoke key is the only gate a frame cannot pass, so any future change that evaluates Package-provided content in the Shell realm reopens the whole surface.
- The isolation property depends on Tauri and WKWebView internals. `npm run verify:packaged-app` runs a Package UI Frame isolation probe inside the real packaged application and checks the Core database afterwards; it is re-run on every Tauri upgrade.
- A Package UI that blocks its thread still freezes the Shell.
- A Windows or Linux port re-verifies the property: custom schemes there resolve as `http://<scheme>.localhost`, and the sandbox ordering around form submission differs outside WebKit.
- A Package asset declared under `__teamarness/` is shadowed by the Shell-owned frame resources.
- `uiHostVersion` stays `1`: the author contract is unchanged.

## Evidence

- `apps/teamarness-desktop/src/package-mount.ts` creates one sandboxed frame per mount and transfers one port after its first load.
- `apps/teamarness-desktop/src/package-port-host.ts` holds the Shell half of the bridge, with the validation rules and the bounds.
- `apps/teamarness-desktop/src/package-frame/` holds the private protocol, the port client and the Shell-owned frame runtime. It imports neither Tauri nor Shell modules.
- `apps/teamarness-desktop/src-tauri/src/package_protocol.rs` serves the reserved frame resources and the frame document policy, and asserts that the embedded runtime carries no Tauri surface.
- `apps/teamarness-desktop/src-tauri/tauri.conf.json` holds the Shell policy: `script-src 'self'` and `frame-src teamarness-package:`.
- `apps/teamarness-desktop/src-tauri/capabilities/default.json` scopes the Shell command set to the `main` webview.
- `scripts/ui-isolation-probe/` is a local probe Package whose UI checks Tauri globals, parent and top access, origin, storage, `ipc://` fetch, theme delivery and bridge errors from inside a real frame, and posts two `create_workspace` commands through the WKWebView message handler.
- `scripts/smoke-packaged-app.mjs` runs that probe as a third launch and then reads `core.sqlite3` directly: no Workspace named `ui-isolation-breach%`, and the smoke's own Workspace still present.
- `scripts/package-foundation-qualification.test.mjs` holds the Shell realm free of dynamic imports, window listeners and widened sandbox tokens, and holds the author trees free of Tauri imports.

**Known weaknesses.** The messageHandler entry point stays reachable from a frame; only the invoke key rejects its posts, and that is Tauri-internal behaviour rather than a documented guarantee. The probe proves the property for one Tauri version on macOS. Nothing here contains a hang, and nothing here makes hostile Package code safe to run.

### Provenance (non-versioned)

- `.myflow/workstreams/architecture-review/prototype/20260917_ui-isolation-prototype.md`
- `.myflow/workstreams/architecture-review/design/20260917T233000+1200_phase-5-package-ui-isolation.md`
- `.myflow/workstreams/architecture-review/plan/20260917T233000+1200_phase-5-package-ui-isolation.md`
