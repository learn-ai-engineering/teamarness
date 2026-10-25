# 0031 — Public same-repository releases and consent-driven application updates

**Status:** accepted (2026-10-24)

## Context

Teamarness is a local-first desktop application with application-bundled Packages and separately supervised Package Backends. A desktop upgrade replaces the Shell, Core executable, and bundled Package resources together. Installation Identity, Workspace state, and persisted Package data live outside the application bundle and must survive replacement.

The developer chooses to publish the source repository and use `learn-ai-engineering/teamarness` for source, Actions, releases, and update metadata. Keeping a private source repository and a separate public releases-only repository is no longer a requirement. The visibility change and public-disclosure review remain maintainer actions, not consequences automatically performed by application code.

Users want automatic discovery and download, but control when the application stops. Tauri's macOS installer replaces the bundle before relaunch. Old Package code must not continue loading resources from a replaced bundle.

An updater signature authenticates bytes. Unless the signature also authenticates the announced version, a metadata host can pair a newer version number with an older validly signed artifact. Mandatory signatures alone do not establish the stable-version policy.

## Decision

### Public release contract

Use the public source repository for stable tagged releases and anonymous update downloads. Use a same-repository GitHub Actions token with publication permission only where needed. Do not embed credentials in the application or introduce a separate release host.

Stable tags use `vMAJOR.MINOR.PATCH` and agree with the desktop's embedded version. Keep independently versioned Package manifests independent. Qualify all bundled Packages and Backend executables in a native Apple Silicon build.

Upload required assets and complete metadata to a draft, validate them, and publish only after qualification. Publication is the final mutation. Artifact URLs identify a version tag; only the stable metadata discovery URL uses `latest`. Serialize publication, reject non-increasing stable versions, and do not overwrite published releases from the workflow. Failures before publication leave the previous advertised release unchanged.

### Version-authenticated executable trust

Use the official Tauri updater verifier and installer with version-bearing signatures and mandatory signed-version enforcement. Upgrade the compatible Tauri tooling rather than maintain a second signature scheme around the older stack.

Accept only a strictly newer canonical stable version. Reject prereleases, altered bytes, unversioned signatures, and a mismatch between the announced and authenticated version. A trusted release pipeline must also verify that the packaged application's embedded version matches the tag and signed version.

Keep the signing private key in protected CI secret storage with a maintainer-controlled encrypted backup. Ship only the public key. Client downloads remain unauthenticated HTTPS requests; they do not carry a maintainer credential.

This contract does not provide freeze protection, signing-key revocation, threshold signing, or automatic recovery from signing-key compromise. These limitations are explicit rather than implied guarantees of a static release feed.

### Shell-owned acquisition and application

The Rust Shell owns update policy, scheduling, bounded transfer, verification, pending bytes, and apply admission. Retain at most one verified candidate, in memory for the running session. Separate verification from installation. A completed transfer is not Ready until cryptographic verification succeeds.

The main Shell webview renders status and requests user actions through reviewed typed commands. Package UI Frames receive no update command, installer permission, process-restart permission, or new frame-bridge authority. Development and nonproduction builds do not opt into production self-replacement.

Later leaves the running application and its bundle unchanged. Only Restart now permits the terminal apply operation.

One Shell lifecycle coordinator owns ordinary Quit and update Apply. It rejects new ordinary dispatch, drains admitted work, closes protocol mounts, awaits Package Host shutdown, and then awaits Core shutdown. Concurrent exit requests share the terminal intent and cleanup result rather than starting competing shutdowns.

After successful shutdown, install the same verified candidate, then explicitly restart. Never rely on Tauri restart to wait for the ordinary exit hook. A cleanup error or uncertain outcome prevents installation and restart. An installation failure after shutdown leaves a terminal recovery view with quit and manual-reinstallation guidance, not a falsely usable stopped runtime. Do not promise transactional application rollback.

## Rejected alternatives

### Private source with a public releases-only destination

The developer explicitly supersedes this topology with public source and one repository. Retaining it would add another destination and publisher credential without satisfying a current requirement.

### Signature-only verification with an unsigned announced version

A valid old artifact can be relabeled as a future version by a compromised metadata host. Bind the announced version to the authenticated signature instead.

### Keep the older Tauri stack and add custom trust machinery

Official compatible tooling already signs and verifies versions. The developer selects the dependency upgrade and its native requalification cost over custom security code.

### Background download-and-install, followed by a restart prompt

Installation already changes the macOS bundle. Later would no longer preserve the current application resources, contrary to the intended consent contract.

### Install before shutting down Package Backends

It simplifies some installation-error recovery, but old code can load new resources while shutdown is still pending. Stop first and make the post-shutdown failure mode explicit.

### Give JavaScript updater/process plugins control of application restart

Direct plugin commands do not establish that Teamarness cleanup has completed. They also widen the authority exposed to the webview. Native Shell ownership keeps lifecycle and verification policy local.

### Persist pending downloads or deploy a custom update server

Neither is required for the first stable single-platform flow. Persistence adds quota and cache-trust rules. A custom server adds operations without removing the need for signed-version verification.

## Consequences

- Public source, release archives, binaries, and metadata are expected. Maintainers review disclosure readiness and change visibility separately. This ADR is not evidence that either step has completed.
- Endpoint and public-key continuity become distribution contracts once binaries ship. Changing the repository location or losing the signing key can strand old clients and require manual bootstrap.
- Existing binaries without updater support require manual installation of the first updater-enabled release.
- Release automation needs trusted tags, restricted signing-secret access, least-privilege publication, draft readback, version-order checks, and rerun rules. Public workflows must not expose signing secrets to untrusted pull requests.
- The Tauri upgrade requires full native packaged-app and Package UI Frame isolation qualification, not just TypeScript tests or successful compilation.
- A bounded HTTPS adapter is needed because the examined plugin download path buffers bytes without an exposed size cap. Verification and installation still use the official updater. A failed or oversized download does not stop normal use.
- A candidate is lost on exit and may need downloading again. No persistent cache or resumable transfer is promised.
- Shutdown coordination is Shell-internal. Core and Package public contracts do not change under this decision. If existing shutdown operations cannot establish the required result, return to design and scope before expanding those contracts.
- Restart consent can discard unsaved UI-only drafts. Persisted identity and data must survive; there is no generic Package draft-flush contract.
- Updater cryptographic signatures and macOS ad-hoc signing are distinct from Apple Developer ID signing and notarization. The latter remain deferred, and Gatekeeper, quarantine, permissions, and real two-version update behavior require installed-app evidence.
- Neither a stable feed nor a six-hour polling policy guarantees adoption within one hour. Local-first application use remains available without GitHub.

## Evidence

Repository precedents and current behavior:

- `context/.decisions/0002-rust-tauri-iroh.md` establishes Tauri, including updater intent.
- `context/.decisions/0011-decision-log-conventions.md` requires self-contained decision evidence and the repository template.
- `context/.decisions/0015-package-ui-sandboxed-frames.md` requires main-Shell authority and real packaged isolation probes on Tauri upgrades.
- `apps/teamarness-desktop/src-tauri/src/lib.rs` currently coordinates mounts, Package Host, and Core shutdown, but discards returned errors. Its ordinary exit hook is not an awaited restart protocol.
- `crates/teamarness-package-host/src/host/lifecycle.rs`, `src/instance.rs`, and `src/supervisor.rs` implement shutdown, generation suppression, append-fence retirement, and process-group cleanup. Some lower-level cleanup errors are ignored, so inspection alone is not proof that all old processes are gone.
- `scripts/build-application-packages.mjs`, `scripts/build-package.mjs`, `packages/bundled-packages.json`, and `scripts/smoke-packaged-app.mjs` establish native Package staging and packaged qualification.
- `context/03-delivery/requirements.md` and `context/02-system/07-shell/requirements.md` currently mark updater delivery unscheduled. Implementation must reconcile that wording without claiming unverified distribution.

Official source inspected on 2026-10-24:

- [Updater 2.14.0 source](https://raw.githubusercontent.com/tauri-apps/plugins-workspace/updater-v2.14.0/plugins/updater/src/updater.rs): separate `verify` and `install`; version-bound restoration; built-in download buffers; macOS replaces the bundle before relaunch.
- [Updater workspace](https://raw.githubusercontent.com/tauri-apps/plugins-workspace/updater-v2.14.0/Cargo.toml): Tauri 2.12 dependency.
- [CLI 2.12.4 signing](https://raw.githubusercontent.com/tauri-apps/tauri/tauri-cli-v2.12.4/crates/tauri-cli/src/helpers/updater_signature.rs) and [bundling](https://raw.githubusercontent.com/tauri-apps/tauri/tauri-cli-v2.12.4/crates/tauri-cli/src/bundle.rs): the normal bundle path includes the application version in the signature's authenticated trusted comment.
- [Tauri 2.12.5 application lifecycle](https://raw.githubusercontent.com/tauri-apps/tauri/tauri-v2.12.5/crates/tauri/src/app.rs): exit prevention ignores restart; restart can bypass ordinary event delivery. Explicit pre-restart cleanup remains necessary.
- [GitHub Releases contract](https://docs.github.com/en/rest/releases/releases): draft, prerelease, assets, and latest controls support a publication gate. Client cryptographic verification remains necessary even when publication is gated.

**Known weaknesses:** these are source inspections and developer decisions, not measurements or passing implementation evidence. No dependency resolution, production-key setup, public-visibility change, hosted release, ad-hoc-signed installed update, or process-cleanup observation occurred during Design. No comprehensive repository/history disclosure audit is claimed.

### Provenance (non-versioned)

The `auto-update` workstream in the resolver-selected MyFlow home store contains the accepted public-source scope correction, earlier research, developer choices, and the detailed Design. Those working artifacts are not required to understand this decision.
