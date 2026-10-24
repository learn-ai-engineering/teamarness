# 0030 — Package UI Frames may run WebAssembly

**Status:** accepted (2026-10-24)
**Supersedes:** ADR 0015 (Decision, second paragraph: the clause "denies … dynamic code", only as it applies to WebAssembly compilation)

## Context

Each Package UI Mount runs in an `<iframe sandbox="allow-scripts">` whose document policy the Shell sets. The policy denies network access, nested frames, form submission, inline code and dynamic code (ADR 0015). WebAssembly compilation is dynamic code under Content-Security-Policy: a frame without `'wasm-unsafe-eval'` in `script-src` cannot instantiate a module, even from bytes it already holds.

The Documents Package edits its body as a Loro peer in its UI, and Loro's JavaScript runtime is WebAssembly. More Packages are expected to ship WebAssembly components.

The realm, DOM, storage and IPC isolation that ADR 0015 establishes comes from the sandbox token, the opaque origin and the `connect-src`/`frame-src`/`worker-src` denials. It doesn't come from the dynamic-code denial. A WebAssembly module runs in the frame's own realm with only the imports that the frame's JavaScript gives it.

## Decision

The frame document policy adds `'wasm-unsafe-eval'` to `script-src` for every Package UI Frame. No other directive changes:

- `connect-src 'none'`, `frame-src 'none'`, `worker-src 'none'`, `object-src 'none'`, `base-uri 'none'` and `form-action 'none'` remain;
- inline script and `'unsafe-eval'` remain denied, so `eval` and `new Function` stay refused;
- the sandbox keeps the single `allow-scripts` token;
- the Shell's own policy is unchanged.

A Package delivers WebAssembly bytes inside its declared entrypoint or as a declared asset. The frame fetches nothing, and no new manifest field or opt-in is added.

The Shell-realm rule of ADR 0015 is unchanged: no Package code, and no Package-controlled string, executes in the Shell's JavaScript realm.

## Rejected alternatives

- **A per-Package manifest opt-in for WebAssembly.** It adds a manifest field, an admission rule and a per-mount policy variant for a capability that changes no isolation boundary. A WebAssembly module can do nothing that the frame's JavaScript can't.
- **Running WebAssembly in the Package Backend and relaying results to the UI.** A UI that is a CRDT peer would then need a round trip for every keystroke. This is the plain-text model that rendered editing replaces.
- **Keeping the denial and compiling the CRDT to JavaScript.** Loro's JavaScript package is WebAssembly. A non-WebAssembly build would mean maintaining a fork of a pre-1.0 dependency.
- **`'unsafe-eval'`.** It would also allow `eval` and `new Function`, which this decision keeps denied.

## Consequences

- Package UIs can instantiate WebAssembly from bytes they ship. Because WebAssembly runs on the shared web content thread, a runaway module freezes the Shell exactly as runaway JavaScript does (ADR 0015).
- The packaged-app isolation probe also checks that WebAssembly instantiates while `eval`, `new Function`, `fetch`, workers, and parent and top access stay refused. It is re-run on every Tauri upgrade.
- A large WebAssembly payload inlined as base64 grows the entrypoint by about a third. The manifest's 8 MiB entrypoint bound and 32 MiB UI total still apply.
- A Windows or Linux port re-verifies this policy together with the rest of ADR 0015.

## Evidence

- `apps/teamarness-desktop/src-tauri/src/package_protocol.rs` — `FRAME_DOCUMENT_CSP`.
- `scripts/ui-isolation-probe/` and `scripts/smoke-packaged-app.mjs` — the in-app isolation probe.
- `crates/teamarness-package-contract/src/manifest.rs` — UI entrypoint and total size bounds.
- Feasibility spike (branch `spike/rendered-document-editing`, `packages/documents-package/prototype-rendered-body/web/`). Under the production frame policy, `loro-crdt` 1.16.2 failed to instantiate. With only `'wasm-unsafe-eval'` added, it instantiated and edited in WebKit and Chromium, while `fetch`, XHR, `Worker`, `eval`, `new Function`, parent and top access, injected `<style>` and `style=` attributes all stayed refused.
- **Weaknesses.** The spike served frames from a local harness, not the real WKWebView `teamarness-package:` scheme. The packaged-app probe is the authoritative check.

### Provenance (non-versioned)

- Workstream `rendered-document-editing`: `scope/20261024T054907Z_rendered-document-editing.md`, `prototype/20261024T062700Z_option-c-feasibility-spike.md`, `design/20261024T070649Z_rendered-document-editing.md`.
