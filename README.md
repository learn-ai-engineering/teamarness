# Teamarness

Teamarness is a local-first, peer-to-peer team workspace. The desktop combines a Rust Core Runtime, a Tauri Shell, and supervised Package Backends. It can create a Workspace, admit another Installation through an opaque copied invitation, and run the same Package Directory from the application bundle or as a Personal Package installed from the Shell.

## Prerequisites

- [Rust installed with rustup](https://rustup.rs/), using the repository's pinned 1.99.1 toolchain
- Node.js 24.20.0, with npm
- A C/C++ build toolchain supported by Rust; SQLite is built from the bundled source
- On macOS, Xcode Command Line Tools for the current-platform app and DMG bundles

From the repository root, install the pinned toolchain without changing your global default:

```sh
rustup toolchain install 1.99.1 --profile minimal --component rustfmt --component clippy
```

`rust-toolchain.toml` selects Rust 1.99.1, rustfmt, and clippy for this checkout. The workspace's `rust-version` matches that tested baseline. Confirm the tools are available:

```sh
rustc --version
cargo --version
node --version
npm --version
```

## Setup

From a fresh clone, install the locked JavaScript dependency graph and fetch the locked Rust dependencies:

```sh
npm ci
cargo fetch --locked
cargo build --workspace --locked
```

Both Cargo and npm lockfiles are committed. Direct third-party dependencies use exact versions.

### Toolchain and dependency upgrades

Prefer current stable tools and dependencies that are compatible with the application. Make upgrades deliberately, pin the tested versions, and keep the workspace baseline, repository-local toolchain, setup guidance, and CI selections aligned. Do not blanket-refresh unrelated dependencies.

Qualify an upgrade with the complete repository checks, locked workspace build, and actual app/DMG build below. Framework changes must also preserve the architecture and trust gates, including the real packaged-app isolation check for Tauri upgrades (ADR 0015). Report specific build or application failures before choosing an older version or a workaround. A successful fixture or compiler check does not replace real application observations.

## Development

Start the first named development Installation from the repository root:

```sh
npm run dev -- alice
```

The launcher stores its private runtime state under the ignored `.teamarness-dev/alice` directory and creates the profile directory with owner-only permissions on platforms that support them. Profile names may contain 1 to 64 letters, numbers, underscores, or hyphens and must start with a letter or number.

After the first command builds the debug executable, start a second Installation without another Vite server:

```sh
npm run dev:instance -- bob
```

Each profile owns a distinct Installation identity and `core.sqlite3`. When creating or joining a Workspace, choose a Workspace-specific Person name. You can rename your Person in Workspace settings, and rename the Workspace itself; a Workspace name is local to your Installation, so other Members keep their own name for it. The Installation that created a Workspace can delete it from Workspace settings while no one else is a Member, after typing its name: this erases the Workspace and all of its data on that Installation, which makes test Workspaces easy to remove. Add a Workspace Agent by name in the same settings screen; Core creates and keeps its signing key locally. The Agent appears with its Steward in the member roster. A Steward can rename or remove their Agent, and the Workspace creator can remove another Person's Agent without removing that Person. The creator cannot rename someone else's Agent.

Conversations shows remote authors' current Principal names and updates displayed Messages after a rename. A missing or duplicate name keeps an inspectable author-identity fallback; your own Messages remain **You**. An Agent posts in Conversations through the optional Personal Agent Package; see [Run a Workspace Agent](#run-a-workspace-agent). Reuse the same profile name to verify restart behavior, and never share one profile between running processes. The production runtime uses Iroh's default relay configuration; launching can complete while offline, but creating an invitation requires relay readiness.

An absolute directory is available as an expert override. Arbitrary relative paths are rejected:

```sh
npm run dev -- /absolute/private/teamarness-one
npm run dev:instance -- /absolute/private/teamarness-two
```

The lower-level `npm run tauri dev` command remains available for an ordinary launch using Tauri's bundle-specific application-data directory. The JavaScript bridge cannot select or inspect that directory. The direct debug binary also remains available for troubleshooting:

```sh
target/debug/teamarness-desktop --data-dir /absolute/private/teamarness-two
```

## Run a Workspace Agent

The optional **Agent** Personal Package (`dev.teamarness.agent`) runs one of your Workspace Agents against an OpenAI-compatible chat-completions endpoint and replies in Conversations when a Member addresses the Agent by name. Configuration lives in the Package's own UI on the Steward's Installation. The Shell has no Agent settings, and binding is never replicated.

**Current interface:** the Host now offers only `teamarness.package-ports` 2.0. Rebuild the standalone harness's capability declaration and SDK before installing it. Bound-Agent Conversations access defaults to disabled, including manifest-declared Ports. Explicitly enable Conversations through the harness's own controls, then load its guide. The Host retains that selection across rebind and restart; disabling it ends Agent subscriptions without cancelling already-authorized command effects. This repository has not qualified a fresh external harness build, its current UI controls or model endpoint. The earlier reply-flow instructions below are not evidence that an old harness works with 2.0.

First build the standalone Agent project outside this repository. It is not shipped in the application bundle. Install its built Package Directory from the Shell: on the Workspace page, select **+** (**Add Personal Package**) in **Package Instances**, choose the folder, check the preview, and select **Trust and install**. The Agent starts at once, with no restart. After a rebuild, select **Reload** on the Agent's Package page. [Local Package registration](docs/local-package-registration.md) describes both steps. Keep `dev.teamarness.agent` unchanged across rebuilds and reinstalls: its settings, key and progress remain in the same Package Data Directory.

1. In Workspace settings, add an Agent by name under **Add agent**. You are its Steward.
2. Select **Agent** under **Personal Packages**. In the **Agent** section, choose the Agent and select **Use this Agent**. Only Agents you steward on this Installation are offered.
3. In **Model endpoint**, enter the base URL (default `https://openrouter.ai/api/v1`; the Package appends `/chat/completions`), the model slug your provider expects (for example a DeepSeek model on OpenRouter), an optional system prompt, and how many earlier Messages a reply sees (0 to 64, default 20). Select **Save settings**.
4. In **API key**, paste the key and select **Store key**. The key is written once and never shown again; the UI reports only whether a key is stored.

The **Status** section shows one of five states:

| State | Meaning |
|---|---|
| `unconfigured` | The endpoint, model, or key is missing. |
| `no_agent` | No Agent is bound, or the bound Agent was removed from the Workspace. Choose another Agent. |
| `ready` | The Agent replies when a Person writes its current name as a whole word, with or without a leading `@`, in any letter case. |
| `replying` | A completion is in flight. |
| `error` | The code identifies the failure: `agent.endpoint_unreachable` (connection, timeout, or 5xx), `agent.endpoint_rejected` (401 or 403), `agent.model_error` (invalid completion), `agent.post_failed` (refused or uncertain post), `agent.capability_unavailable`, `agent.storage_failed`, or `agent.progress_invalid` (lost projection continuity). Ordinary errors can clear after configuration changes or a successful reply. Unsafe dispatch and invalid progress have stricter recovery limits below. |

The Agent replies once per mention, only in the Channel it was addressed in, with at most one reply in flight per Channel. It does not answer Messages from before the Package first ran, Messages from other Agents, or a mention whose reply failed or was cancelled. A failed or uncertain reply is never retried by the Agent. Settings, the binding, and per-Channel progress survive a restart, so the Agent resumes without re-entry. Other Installations need no setup: they show the reply attributed to the Agent with its Steward, like any Agent-authored Message.

Conversations 1.2 supplies revision-based catch-up. The Agent saves each Channel's projection epoch and acknowledged revision, so an offline Message is still processed when it arrives before older Messages in display order. Each pass examines at most four batches of 32 changes per Channel and schedules further catch-up without discarding the backlog. Temporary read failures and pending content are safely queried again; a settled model/post attempt is never retried. Invalid saved progress stops the harness with `agent.progress_invalid` rather than silently replaying history. Changing endpoint settings does not repair that lost continuity.

Pending posts refuse binding changes with `agent.post_failed` rather than risk attributing a reply to another Agent. A failed or uncertain post, or an uncertain binding write, can stop further dispatch and binding changes. Settings edits and suspend/resume do not clear that safety state. Restart the full application process for conservative recovery; do not delete progress or profile data. Backend-only replacement requires the Host's generation safeguards: queued obsolete authority work rejects, and running Core authority work finishes before replacement initializes. Stuck Core work can delay replacement of that Instance. This does not retract arbitrary effects already dispatched to another Package.

The Package keeps three files in its Package Data Directory, `packages/data/v1/<installation>/<workspace>/<SHA-256 of dev.teamarness.agent>/` under the profile's private data directory: `settings.json`, `api-key` (the raw key, file mode `0600`), and `progress.json` (per-Channel projection epoch and acknowledged revision). The key is protected only by that file mode and the private data directory, so any process running as your user can read it. Select **Clear key** before you share or delete a profile. The Package sends network traffic only to the configured endpoint, over TLS with the platform's trust roots for `https://` URLs; `http://` is accepted for local test endpoints. Completion redirects are not followed. A completion is limited to 60 seconds and 256 KiB. Streaming replies, tool use, and unprompted posts are not supported.

## Generic Agent Port proof

Run the source-built, test-only generic consumer against real Conversations:

```sh
cargo test -p teamarness-system-tests --test agent_port_discovery --locked
```

The consumer has no Provider-specific manifest or adapter. The script discovers complete metadata, explicitly enables Conversations, validates and loads its guide, posts with Host-derived Agent attribution, and reconciles after reconnect. Host safety tests exercise both access views. This proves repository interfaces, not a real-model loop or the standalone Agent UI. The optional `agent_reply_flow` tests require a freshly rebuilt external Package Directory; their default ignored status is not a pass. See the [author guide](docs/package-author-guide.md#act-as-a-workspace-agent).

## Standalone External Gateway qualification

The External Gateway is a separate Personal Package, not an application Package or a
source dependency of this repository. It uses package-ports 3.0 as the current unbound
Person; no Agent Package, binding or model configuration is needed. Major-2 consumers
remain supported alongside it, with no removal scheduled by this workstream.

From this repository, explicitly supply a **clean, committed** gateway source project:

```sh
node scripts/verify-external-gateway.mjs --project /absolute/path/to/external-gateway
cargo test -p teamarness-system-tests --test person_port_discovery --locked
cargo test -p teamarness-system-tests --test agent_port_discovery --locked
```

The runner verifies SDK ancestry and exact canonical vendor trees, runs the complete
locked gateway checks/build and validates assembled bytes, then executes all ignored
`external_gateway_flow` cases with `TEAMARNESS_EXTERNAL_GATEWAY_PACKAGE_DIR`. Missing
artifacts/environment, zero executed tests or any failed check fail qualification.
Default Host tests deliberately ignore these external cases; that is not a pass.
Only the supplied directory is installed, into disposable loopback-only test profiles.
No normal profile or sibling Agent repository is used. Provenance records source,
lockfile, vendor and artifact digests, never bearer values or Package data contents.

Real processes qualify Backlog paging/reads/guide/errors; live unknown multi-contract
Providers and schema changes; HTTP catalog discovery across more than 32 offered
contracts, following exact returned cursors past an empty filtered page to completion
without rebuilding the gateway; major-2 compatibility; membership and caller authority;
and listener/token isolation, pause/recovery, restart/reload, removal/reinstall and port
conflicts. Canonical membership loss stops an idle listener and all existing route
connections, including static guidance and primed schemas; retained enabled setup does
not restart while membership is absent. A separate bound-caller control still proves
Person refusal without actor fallback. The gateway observes the existing membership
1.0 capability; this does not change other Packages’ lifecycle. Controlled reads
separately prove unchanged Host overload and its earlier
SDK timeout, disconnect ownership and no replay. The gateway's twelve slots count
outstanding SDK operations, not twelve simultaneous Provider executions. SDK terminals
release slots; a timeout does not establish that native Provider work stopped.

Gateway revocation applies at final local admission, not SDK writer enqueue. A read
admitted before revoke, regeneration or a permission change may still wait inside the
SDK, hand off and finish afterward. Later admissions use current policy. Use the
external agent's own stop control to stop its work; revocation is not cancellation.

These automated proofs do not replace Don's native settings/token/lifecycle observations
or an actual outside coding agent following onboarding. Run manual checks only in a
named development profile (`npm run dev -- external-gateway-verify`), using local secret
handling; never retain bearer values in screenshots, commands or verification artifacts.

## Checks

Run all repository checks from the repository root:

```sh
npm run check
```

It runs, in order: `npm ci`, `npm run build`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, `cargo test --workspace --locked`, `npm run lint`, `npm run typecheck`, and `npm test`. The build must come before the Cargo checks because `teamarness-desktop` bundles the Package Directories that `npm run build` stages under `target/teamarness-packages`. For quick Rust-only checks without an npm build, exclude the desktop crate:

```sh
cargo test --workspace --exclude teamarness-desktop --locked
cargo clippy --workspace --exclude teamarness-desktop --all-targets --all-features --locked -- -D warnings
```

The Documents rendered editor has real-browser scenarios that are not part of `npm test`:

```sh
npm run verify:documents-editor
```

It builds the Documents UI and the Rust Markdown authority (`packages/documents-package/backend/examples/frame_scenarios.rs`), installs the Playwright WebKit and Chromium browsers if they are missing, and runs `packages/documents-package/ui/frame-scenarios/`: two or more views mounted in sandboxed `allow-scripts` frames under the Shell's frame policy (read from `package_protocol.rs`), relayed between Installations through an in-memory record log with offline control. Pass `webkit` or `chromium` to run one browser, and `--only "<scenario>"` to run one scenario.

The bundled Packages are listed once in `packages/bundled-packages.json`. After editing it, run `node scripts/bundled-packages.mjs --write` and `npm run build`; `npm test` fails while the Tauri bundle configuration or the application index disagrees with the list.

The Core Runtime public API is `teamarness_core::CoreRuntime`. Identity secrets, raw key bytes, SQLite handles, Iroh transport handles, canonical membership bytes, and private membership adapters remain private. The Tauri bridge carries only owned Installation, Workspace, membership, Principal, invitation, and stable-error DTOs. Agent keys and invitation material do not cross the bridge as Agent-management responses.

## Explicit application builds

Select one native application and target. Both selectors are required; unsupported or non-native combinations fail before building or removing output:

```sh
npm run build:app -- --app desktop --target aarch64-apple-darwin
npm run build:app -- --app headless --target aarch64-apple-darwin
npm run build:app -- --app headless --target x86_64-unknown-linux-gnu
```

Run only the command matching your intended application and native host. There is no cross-compilation or build-all fallback. `npm run build` remains the desktop-oriented compatibility path. A selected headless build never builds the desktop frontend or Tauri; it produces `target/teamarness-applications/headless/<target>/` with its executable and complete Team Package resources. Keep that directory together. Neither Agent nor External Gateway is required.

See [Run a headless Teamarness Installation](docs/headless/README.md) for relocation, foreground startup, private onboarding and Package requests, artifact smoke, and the manually installed Linux user service. Mac headless uses foreground operation, not launchd. The [canonical verification runbook](docs/headless/verification.md) covers source-current sync-only loopback automation, final Mac M, native Ubuntu N, and human M1–M5. Native Ubuntu and actual production mesh/service/logout/reboot/desktop observations remain pending; Mac automation is not Linux evidence. Independent [Agent](docs/headless/follow-ups/agent.md) and [gateway](docs/headless/follow-ups/external-gateway.md) briefs distinguish generic Host evidence from unrun product compatibility.

## Current-platform bundles

The selected desktop build delegates to the existing ordinary local macOS application and disk-image command (production updates Disabled), which also remains directly available:

```sh
npm run tauri build -- --bundles app,dmg
```

The default outputs are:

- `target/release/bundle/macos/Teamarness.app`
- `target/release/bundle/dmg/Teamarness_<desktop-version>_aarch64.dmg` on Apple silicon

These ordinary artifacts are for local verification, not production update distribution. Stable release/update tooling is implemented locally (ADR 0040); hosted publication, final native isolation and real installed updates are not yet qualified. See [Releasing and update recovery](docs/releasing.md) for protected workflow setup, actual release/dry-run commands, key custody and the required A→B gates. Source, binaries and metadata are intended to be public in `learn-ai-engineering/teamarness`; visibility/disclosure changes remain maintainer-owned. Existing binaries require manual DMG bootstrap; ordinary local use remains offline and requires no GitHub login. Developer ID signing, notarization, tray and native menus remain deferred.

### Local release artifact qualification

Use exact Rust 1.99.1 and Node 24.20.0 on native Apple Silicon macOS. Desktop versions have one contract; independent Package versions are not changed:

```sh
node scripts/release/version.mjs check --tag v0.1.0
node scripts/release/version.mjs set 0.1.1  # when deliberately preparing the next desktop version
```

The explicit release configuration opts into `production-release`, updater archives, mandatory signed versions and macOS ad-hoc signing. It preserves the base identifier, resources, frame CSP and main-only capability. Supply a separate reviewed public-key JSON overlay with `plugins.updater.pubkey`; the tracked release config intentionally has no key. A non-debug production build with a missing/malformed key fails with a redacted error. Debug/all-features checks need no key. The public key and fixed public GitHub endpoint compile into the client, with no runtime override.

```sh
# PRIVATE_SIGNING_KEY_FILE/PASSWORD are supplied only to this signing step.
# Never echo their values or retain private material with build outputs.
TAURI_SIGNING_PRIVATE_KEY="$PRIVATE_SIGNING_KEY_FILE" \
TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$PRIVATE_SIGNING_KEY_PASSWORD" \
npm run tauri build -- --config src-tauri/tauri.release.conf.json --config /private/path/public-key-overlay.json --bundles app,dmg
npm run verify:release-artifacts -- --directory target/release/distribution --tag v0.1.1 --public-key /private/path/public-key.txt --collect
```

The release staging path signs temporary Backend copies **before** generating Package size/digest declarations. It bundles those exact Package Directories as resources; redundant Tauri sidecar copies are omitted only in the release configuration. Never re-sign or otherwise edit the resulting app. Rerun qualification before any later publication. `CI=true` may be used with the pinned Tauri CLI when Finder Automation is unavailable; it skips only DMG cosmetic layout, not artifact checks.

`--collect` copies the exact version-derived DMG, `Teamarness.app.tar.gz`, and its complete `.sig`, then generates bounded `latest.json` with both macOS target keys. Without `--collect`, the same command revalidates existing output. Optional `--notes FILE` accepts at most 4 KiB of plain text. No arbitrary output directory or extra assets are accepted. Qualification uses the pinned official Rust verifier on real archive bytes for both targets, safely extracts the archive, mounts the DMG readonly, compares both whole application trees against the built app, and checks every declared Package's bytes/digests/modes, arm64 executable signatures, and real admission/Backend handshake. Missing staged Packages fail; there is no Accessibility dependency.

Only the four distribution assets belong in a future public release. `qualification.json` is a **CI-only**, source-SHA/dirty-source/digest record, never a public asset. No private key, log, or runtime data belongs in `target/release/distribution`. This command publishes nothing. A passing local qualifier does not prove Gatekeeper/quarantine behavior, real WKWebView isolation, cleanup, or installed update continuity. Don must still run the packaged smoke against the release-style app, open the DMG, and launch a copied writable app; record those observations separately. Ad-hoc signing and updater cryptographic signatures are not Developer ID signing or notarization.


After the bundle command succeeds, run the automated packaged-app smoke on Apple silicon macOS:

```sh
npm run verify:packaged-app
```

The smoke mounts the DMG for inspection, compares every bundled Package Directory with its staged bytes, launches the `.app` twice with private temporary data, creates a Workspace, checks that every bundled Package Backend launches from the application bundle, selects the bundled reference Package through macOS accessibility, checks its admitted UI and protocol response, and verifies process-group reaping after each graceful exit. Its Exhibits stage registers the test-only Exhibits fixture consumer, publishes Drafts through the real Exhibits Ports, and checks that the Draft renders and navigates in the Exhibits UI, that the open Draft changes to its next revision without being reselected, and that its content, framed with or without a `sandbox` attribute, cannot read the Package UI or the Shell and reaches no Tauri command. The frame without the attribute is nested in the sandboxed Package UI Frame and inherits its flags, so the CSP `sandbox` directive itself is proven by the `content_protocol.rs` tests. It requires Accessibility permission for the launching terminal. The developer-owned app and DMG walkthroughs remain separate Verify checks.

To try Exhibits by hand in a development profile, register the same test-only fixture consumer. Run `npm run build` first, because it stages the Exhibits Package:

```sh
npm run build
node scripts/exhibits-fixture-consumer/prepare.mjs exhibits
npm run dev -- exhibits
```

`prepare.mjs` builds the consumer with Cargo and adds it to the profile's local Package Registrations, keeping the others. It also accepts an absolute data directory for a packaged-app launch with `--data-dir`. The consumer appears as **Exhibits fixture consumer** under **Personal Packages**. Its buttons publish fixture Drafts that **Exhibits** under **Team Packages** then lists. To watch an open Draft change, click **Publish revision 2 in 30 s**, then open **Exhibits** and select **Exhibit fixture (revision 1)** within 30 seconds: the Shell shows one Package at a time, so the delayed publish lands while Exhibits is shown. The consumer never ships in the application bundle.

To run two packaged Installations with separate absolute directories, launch the application bundle twice:

```sh
open -n target/release/bundle/macos/Teamarness.app --args --data-dir /absolute/private/teamarness-one
open -n target/release/bundle/macos/Teamarness.app --args --data-dir /absolute/private/teamarness-two
```

Invitation values are sensitive bearer admission material. Do not store them, private keys, raw secret bytes, or private database contents in logs or verification notes.

Teamarness stores the Installation secret and Content Keys unencrypted in `core.sqlite3` inside the private data directory (ADR 0017). Any process running as your user, including trusted Package Backends, can read them. Use FileVault and encrypted backups, and keep development profiles out of cloud-synced folders. The Core Runtime refuses a data directory that is a symbolic link, belongs to another user, or holds unrelated files.

The runtime refuses a data directory that still holds retired version-1 protection controls. If a disposable development profile is refused, reset it with `rm -rf .teamarness-dev/<profile>`. For a disposable packaged-app profile, reset `~/Library/Application\ Support/dev.teamarness.desktop`. Back up any data you need before removing a profile.

The Documents record format changed incompatibly when Document bodies became structured trees (ADR 0032): a `document.create` record now carries a body genesis, and plain-text body records are no longer read. A development data directory that holds Documents from before that change must be reset in the same way. There is no migration, because no released build wrote those records.

## Documents folder mappings and upgrades

Documents Collections support nested folder paths and local two-way Markdown mappings. New mappings include eligible nested lowercase `.md` files after preview and confirmation. Existing mappings stay flat until you select **Enable subfolders** in Documents **Configuration**, review the incremental report, and confirm. Imports are shared with the Workspace. Each Installation consents independently; another Member cannot enable your ignored subfolders. Unmapping leaves files on disk.

Forward migrations 0011 and 0012 preserve Document bodies and file associations but prevent older builds from opening the upgraded local database. Older peers on their own databases still exchange ordinary body edits. They ignore folder placement until upgrade; format generation 2 recovers retained placement without redelivery. This is distinct from the historical incompatible body-format change described above and requires no data reset.

A file move preserves identity only with an unambiguous unchanged-hash match. Moving and editing together, or moving a file across mapped roots, does not guarantee identity. Use **Move to Collection** in the app for an identity-preserving Collection move. Hidden paths, temporary files, and extensions other than lowercase `.md` are excluded; unsafe or unrepresentable folder paths are refused rather than renamed silently.

A missing or unreadable root archives nothing. Deleting the only populated subtree also marks the mapping unavailable and archives nothing. Deleting a safely scanned subtree archives its Documents when eligible files remain elsewhere; restore recreates files and parents. Repeating archive after restore as the same author remains an accepted limitation. Filesystem race limits and other retained debt are documented in [DELTA-023](context/.delta/DELTA-023-preserved-blocks-accepted-tech-debt.md).

Local macOS automation covers recursive sync and the folder controls. Don observed subfolders, filesystem edits across app instances, renames and archives working, and accepted the folders-first alphabetical UI after creating new Documents and folders. The final UI-only change passed all 162 Documents UI tests; Don approved scoped testing and manual acceptance instead of another full validation round. Directory replacement by a regular file and case-only directory moves on case-sensitive storage remain accepted limitations. Broader manual, platform and storage checks are deferred in [DELTA-023 group 12](context/.delta/DELTA-023-preserved-blocks-accepted-tech-debt.md#12-slice-c-accepted-limitations-and-follow-up-checks). See [Document folders](context/.decisions/0043-document-folders.md) for contracts and evidence. This local acceptance is not release qualification.

## Publish a stable release

A stable release starts when a `vX.Y.Z` tag is pushed. The tag must match the desktop version in the tagged commit, and that commit must be on `main`. Do not sign the commit or the tag. The release workflow signs the update artifacts with the key in the `release` environment.

1. From an up-to-date `main`, set the next version on a branch, then commit the changed version and lockfiles:

   ```sh
   git switch main && git pull
   git switch -c release/vX.Y.Z
   node scripts/release/version.mjs set X.Y.Z
   ```

2. Merge that branch through a pull request.
3. Tag the merged commit and push only that tag:

   ```sh
   git switch main && git pull
   node scripts/release/version.mjs check --tag vX.Y.Z
   git tag vX.Y.Z origin/main
   git push origin vX.Y.Z
   ```

4. In GitHub Actions, approve the `signed-build` and `publish` jobs for the `Stable release` run.

Repository rules prevent moving or deleting a pushed `v*` tag. If a release fails because of its source, correct the source and release a higher version. For example, `v0.1.3` failed preflight, so the next release is `v0.1.4`.

If preflight fails with `Unsupported release event/repository/ref/source SHA`, check that the desktop version in the tagged commit matches the tag.

See [Stable releases and application update recovery](docs/releasing.md) for approvals, failure handling, and recovery.

## Package development

Start with [Build your first Package with an agent](docs/build-your-first-package.md). It begins with the finished Tutorial, explains the boundary between the Teamarness Core Runtime and a Package, then walks through generating, building, registering, and using an external Emojart project. Generate Emojart outside the Teamarness repository with `npm run create:package -- --output <directory> --identity dev.teamarness.emojart --display-name Emojart`. Read the [Package author guide](docs/package-author-guide.md) for the full Package Directory, Backend, UI, contract, and application-distribution rules. Read the UI SDK's [Package UI theme and CSS kit](packages/teamarness-package-ui-sdk/README.md) guidance for the exact semantic tokens, opt-in classes, cleanup contract, and Package-owned header and navigation model. Run `npm run validate:packages` to apply Package Admission's manifest and UI contract rules to every bundled Package's source and staged directory, with a Backend smoke launch, without starting the application; a generated project runs the same validator as `npm run validate`. Read [Local Package registration](docs/local-package-registration.md) to install, reload, and remove a Personal Package from the Shell, override a Team Package, write the registration file by hand, and keep Package Data Directory continuity. A local registration does not grant Workspace enablement or synchronize, share, network, or collaborate Package code or data.

The application build composes the Reference, Tutorial, Conversations, Repo-aware, Documents, Exhibits, and Backlog Packages into one verified index. Agent is built independently and installed as a Personal Package. Run `npm run build` to build every bundled Package's Backend, UI entrypoint, declared assets, and manifest digests; it preserves the installation-local Reference Package copy used by development.

Package Process Protocol 1.0 remains supported. Opt-in process 1.1 confirmed
subscriptions return only after Provider listener setup and Host Consumer
registration; legacy nonconfirming Providers refuse clearly. Existing enqueue-only
SDK methods do not acquire this guarantee. Source-built Backlog, Documents, and
Conversations declare 1.0–1.1 support. External Providers must rebuild with the
compatible SDK and declare 1.1 before offering confirmed streams. This upstream
prerequisite does not yet deliver the standalone gateway's full-access HTTP surface.

The protocol and supported Rust tooling have separate contracts:

- [`contracts/package-process/v1/README.md`](contracts/package-process/v1/README.md) is the normative language-neutral process protocol.
- [`crates/teamarness-package-protocol/README.md`](crates/teamarness-package-protocol/README.md) describes its Rust adapter.
- [`crates/teamarness-package-host/README.md`](crates/teamarness-package-host/README.md) describes the host facade and lifecycle.
- [`crates/teamarness-package-sdk/README.md`](crates/teamarness-package-sdk/README.md) describes the Rust Backend SDK.

Package trust is broad. Package Processes have the current user's ambient filesystem, network, and process authority. Process separation is a recovery boundary, not a sandbox. A Package UI runs in its own sandboxed frame with an opaque origin, so it reaches neither the Shell's DOM nor any Tauri command; it talks to Teamarness only through its mount-scoped bridge. Frame isolation separates realms, DOM, storage, and Tauri access, not threads: a Package UI that blocks its thread still freezes the Shell. Only install or bundle Package code you trust.

## Core Runtime behavior

See [`crates/teamarness-core/README.md`](crates/teamarness-core/README.md) for the asynchronous API, Workspace onboarding and recovery contract, bootstrap limits, database ownership, declared-time history warning, and explicit exclusions.

The runtime exclusively owns one private `core.sqlite3`. Uncertain commit outcomes require dropping and reopening the runtime so authenticated replay can recover durable state. `was_member` is informational author-declared history and must not authorize writes.

## Repository intent

- `context/` is the canonical requirements, ontology, roadmap, and decision tree.
- `crates/teamarness-core/` contains the Core Runtime library, API documentation, and tests.
- `crates/teamarness-package-host/`, `crates/teamarness-package-protocol/`, and `crates/teamarness-package-sdk/` contain the Package runtime and supported Rust tooling.
- `packages/reference-package/`, `packages/tutorial/`, `packages/conversations-package/`, `packages/repo-aware/`, `packages/documents-package/`, `packages/exhibits-package/`, and `packages/backlog-package/` are the bundled Packages, and `packages/teamarness-package-ui-sdk/` is the TypeScript UI SDK.
- `apps/teamarness-desktop/` contains the typed Tauri adapter and Shell.
- `p2p-local-first-user-management.md` is the supporting P2P membership research baseline.

### Application update consent and frame isolation probe

Production update policy stays native; only the main Shell can check or request
restart. Later keeps the running application usable and retains an Update ready
button for this session. Restart now can discard unsaved Package UI edits.
After shutdown starts, recovery offers Quit and reinstall/open guidance, not an
installation retry. Ordinary local builds remain update-disabled.

For the maintainer-owned real WKWebView update-command isolation gate, build the
**disposable, nonproduction** probe (public fixture key only):

```sh
CI=true npm run tauri build -- --features ui-isolation-probe --config src-tauri/tauri.probe.conf.json --bundles app,dmg
npm run verify:packaged-app -- --update-probe
```

Run the smoke from an Accessibility-authorized terminal. It requires a verified
Ready candidate, checks that missing/fake-key raw IPC and forged Package bridge
attempts enter none of the four update handlers, then uses main-Shell consent as
a positive control. The instrumented terminal deliberately fails installation
without executing the fixture installer or restarting; recovery Quit remains
real. This instrumentation cannot build with a non-debug production release.
It does not qualify real replacement, Gatekeeper, hosted access or A→B continuity.
Record those separately, along with keyboard access, Later/retained affordance,
and Updating/recovery layout observations. Rebuild the ordinary local app after
this probe; never distribute the probe as a release:

```sh
CI=true npm run tauri build -- --bundles app,dmg
npm run verify:packaged-app
```

`CI=true` skips only Finder disk-image cosmetics; it supplies no GUI evidence.
