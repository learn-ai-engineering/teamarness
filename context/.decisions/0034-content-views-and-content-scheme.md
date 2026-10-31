# 0034 — Content views are served through a Shell-owned content scheme

**Status:** accepted (2026-10-25)
**Supersedes:** –

## Context

Each Package UI Mount runs in an `<iframe sandbox="allow-scripts">` whose document policy the Shell sets (ADR 0015). That policy denies every nested frame with `frame-src 'none'`.

Some Packages present multi-file web content that they do not write themselves, for example a set of HTML, CSS, script, image and media files that an Agent produced. That content runs scripts, references its own files by relative URL and navigates between its own pages. It must not reach the Package's bridge, the Shell or other Packages.

The Host Semantic Capability `teamarness.content-views` 1.0 lets a Package open a view over files in its own Package Data Directory. The Host verifies the size and SHA-256 of each listed file, assigns the media type from a shared extension allowlist, and returns the URL `teamarness-content://localhost/<token>/<entry>`. The token is 128 random bits and lives as long as the view. The Host command `ReadContent {token, path}` returns the bytes and the media type of one listed file, and refuses everything else with `content_not_found`. When a Package declares the family, the Host reports `nestedContent: true` on that Package's opened UI mounts.

WKWebView exposes `window.webkit.messageHandlers.ipc` to every frame on a registered custom scheme, and only the Tauri invoke key rejects posts from a frame (ADR 0015).

## Decision

The Shell registers a second custom scheme, `teamarness-content:`, next to `teamarness-package:`. Its handler applies the same request rules as the Package resource scheme: GET or HEAD only, host `localhost`, no query, a token segment and a normalized relative path with no traversal and no encoded separator. It passes the token and the decoded path to `ReadContent`. A Host refusal of any kind, an invalid token and an unavailable Host are all a `404` with no body.

**The Shell owns the response policy.** Every response, including every refusal, carries `X-Content-Type-Options: nosniff`, `Cache-Control: no-store`, `Access-Control-Allow-Origin: *` and this Content-Security-Policy:

```text
sandbox allow-scripts; default-src 'none';
script-src teamarness-content: 'unsafe-inline' 'wasm-unsafe-eval';
style-src teamarness-content: 'unsafe-inline';
img-src teamarness-content: data: blob:; font-src teamarness-content: data:;
media-src teamarness-content: blob:; connect-src teamarness-content:;
frame-src 'none'; worker-src 'none'; object-src 'none'; form-action 'none'
```

**The CSP `sandbox` directive is the Shell's guarantee.** Content runs with an opaque origin and scripts only, even when the Package UI creates the frame without a `sandbox` attribute. Content cannot read its parent or the top window, cannot use origin-scoped storage, and receives no Package bridge port.

The handler answers a single `bytes=a-b`, `bytes=a-` or `bytes=-n` range with `206` and `Content-Range`, and an unsatisfiable range with `416`, so WebKit can play media. A multiple-range or malformed `Range` header gets the full `200` response.

**Nesting is opted into per mount.** Declaring `teamarness.content-views` is the only opt-in. The Shell records the mount's `nestedContent` flag beside its resource token, and every response for that mount, including the Shell-owned frame document, carries the base frame document policy with `frame-src teamarness-content:` in place of `frame-src 'none'`. No other directive changes. Every other mount keeps `frame-src 'none'`. A Package UI Frame still loads no network resource and runs no Package code in the Shell realm.

**Tokens are the only scope.** A content document can load files from another open view if it knows that view's token. This is accepted: tokens are unguessable and unlisted, and a per-view host name is deferred.

**IPC exposure is unchanged.** A content frame is on a registered custom scheme, so it can reach `webkit.messageHandlers.ipc` exactly as a Package UI Frame can. Only the invoke key rejects its posts. The packaged-app smoke keeps a database-backed probe for content frames, as it does for Package UI Frames.

**Network denial is not yet complete.** `connect-src teamarness-content:` denies `fetch`, XHR and WebSocket to other schemes, but navigation and WebRTC are not yet controlled. This policy, a navigation handler for content frames, and WebRTC denial are the one place where network denial will be tightened later. Network behaviour is not tested in this decision.

## Rejected alternatives

- **The Package Backend serves the bytes.** Every request would cross the Package Process Protocol, and every large file would be chunked through 1 MiB frames.
- **The Shell reads a Package's storage layout directly.** The Shell would learn Package storage, which breaks the generic Shell rule.
- **`srcdoc`, `blob:` URLs or a reserved path on `teamarness-package:`.** `srcdoc` and `blob:` break relative navigation. A reserved path would widen `frame-src` for every Package and share the Package frame's policy.
- **A scheme named after one Package.** The Shell and Host would name one Package, and a second user would need a rename.
- **`frame-src teamarness-content:` for every Package.** Nesting changes the isolation surface, so it is opted into by declaration. This differs from ADR 0031, which allows `'wasm-unsafe-eval'` in Package UI Frames and changes no boundary.
- **Relying on the embedding frame's `sandbox` attribute.** A Package UI could omit it. The CSP `sandbox` directive comes from the Shell on every response.

## Consequences

- ADR 0015 is extended, not superseded: its frame document policy is now the base policy, and a mount that reports nested content varies only in `frame-src`.
- A Package that declares `teamarness.content-views` can show content it did not write, isolated from its own frame, without serving bytes itself.
- Content can run inline script and inline style, and compile WebAssembly. It cannot use `eval` or `new Function`, workers, plugins, forms or nested frames.
- A file that changes after a view is opened fails Host verification and is a `404`. Showing new content needs a new view.
- The packaged-app smoke's Exhibits stage checks content frames with the isolation probe, in the Exhibits viewer and in a Package frame created without a `sandbox` attribute. It is re-run on every Tauri upgrade, together with ADR 0015's probe.
- A Windows or Linux port re-verifies this policy, because custom schemes there resolve as `http://<scheme>.localhost`.
- Network denial for content frames stays open until navigation and WebRTC are controlled.

## Evidence

- `apps/teamarness-desktop/src-tauri/src/content_protocol.rs` serves the scheme, holds `CONTENT_POLICY`, the range handling and the refusal mapping, and tests each of them.
- `apps/teamarness-desktop/src-tauri/src/package_protocol.rs` holds the shared request rules (`parse_resource_request`) and the per-mount policy builder (`frame_document_policy`), and tests that the nested variant differs from the base policy only in `frame-src`.
- `apps/teamarness-desktop/src-tauri/src/lib.rs` registers `teamarness-content` and records `nested_content` beside each mount token.
- `crates/teamarness-package-host/src/content_views.rs` and `crates/teamarness-package-host/tests/content_views.rs` hold the Host side: open bounds, verification, token lifetime and `ReadContent`.
- `crates/teamarness-package-contract/src/media_types.rs` holds the shared extension allowlist and content path rules.
- `contracts/package-capabilities/v1/content-views.schema.json` is the capability schema.

**Known weaknesses.** The unit tests check the response headers and the policy text, not WebKit's enforcement of them. A throwaway packaged-app spike showed that a nested opaque-origin frame on a custom scheme renders, loads relative assets, runs module and dynamic imports and WebAssembly, and navigates between its own pages, while it cannot read parent or top, has no Tauri globals, cannot use storage or `ipc://` fetch, and its message-handler posts created no Workspace. The spike used a different scheme name and allowed nesting for every Package, and it did not check network access. The packaged-app smoke's Exhibits stage is the authoritative check of WebKit's enforcement. Its frame without a `sandbox` attribute is nested in a Package UI Frame, which the Shell sandboxes, and a child frame inherits its parent's `sandbox` flags. So the smoke proves the end-to-end result, content that stays opaque-origin without the attribute, but it cannot attribute that result to the CSP `sandbox` directive alone. The `content_protocol.rs` tests prove the directive at header level.

### Provenance (non-versioned)

- Workstream `exhibits-package`: `research/20261024T201145Z_viewer-sandbox-and-blob-transport.md`, `prototype/20261024T210823Z_viewer-spike.md` (local branch `spike/exhibits-viewer`, `3994051`), `design/20261024T220106Z_exhibits-package.md` §1–§2, `plan/20261024T222308Z_exhibits-slice-1.md` Phase 2.
