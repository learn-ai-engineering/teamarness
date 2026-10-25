# Teamarness

Teamarness is a local-first, peer-to-peer team workspace. The desktop combines a Rust Core Runtime, a Tauri Shell, and supervised Package Backends. It can create a Workspace, admit another Installation through an opaque copied invitation, and run the same Package Directory from the application bundle or an explicit installation-local registration.

## Prerequisites

- [Rust installed with rustup](https://rustup.rs/), version 1.96 or newer
- Node.js 24 or newer, with npm
- A C/C++ build toolchain supported by Rust; SQLite is built from the bundled source
- On macOS, Xcode Command Line Tools for the current-platform app and DMG bundles

Confirm the tools are available:

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

Each profile owns a distinct Installation identity and `core.sqlite3`. When creating or joining a Workspace, choose a Workspace-specific Person name. You can rename your Person in Workspace settings. Add a Workspace Agent by name in the same settings screen; Core creates and keeps its signing key locally. The Agent appears with its Steward in the member roster. A Steward can rename or remove their Agent, and the Workspace creator can remove another Person's Agent without removing that Person. The creator cannot rename someone else's Agent.

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

**Current interface:** the Host now offers only `teamarness.package-ports` 2.0. Rebuild the standalone harness's capability declaration and SDK before registering it. Bound-Agent Conversations access defaults to disabled, including manifest-declared Ports. Explicitly enable Conversations through the harness's own controls, then load its guide. The Host retains that selection across rebind and restart; disabling it ends Agent subscriptions without cancelling already-authorized command effects. This repository has not qualified a fresh external harness build, its current UI controls or model endpoint. The earlier reply-flow instructions below are not evidence that an old harness works with 2.0.

First build the standalone Agent project outside this repository and [register its Package Directory locally](docs/local-package-registration.md). It is not shipped in the application bundle. Keep `dev.teamarness.agent` unchanged when switching an existing installation from bundled to local: its settings, key and progress remain in the same Package Data Directory. Restart after registration or a rebuild.

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

## Current-platform bundles

Build unsigned local macOS application and disk-image bundles with:

```sh
npm run tauri build -- --bundles app,dmg
```

The default outputs are:

- `target/release/bundle/macos/Teamarness.app`
- `target/release/bundle/dmg/Teamarness_0.1.0_aarch64.dmg` on Apple silicon

The DMG step styles the disk-image window through Finder, so the launching terminal needs Automation permission for Finder. Without it, the `dmg` bundle fails with `Not authorised to send Apple events to Finder`, and `--bundles app` still builds the `.app`.

These artifacts are for local verification. Signing, notarization, distribution, auto-update, tray, and native-menu infrastructure are intentionally out of scope.

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

## Package development

Start with [Build your first Package with an agent](docs/build-your-first-package.md). It begins with the finished Tutorial, explains the boundary between the Teamarness Core Runtime and a Package, then walks through generating, building, registering, and using an external Emojart project. Generate Emojart outside the Teamarness repository with `npm run create:package -- --output <directory> --identity dev.teamarness.emojart --display-name Emojart`. Read the [Package author guide](docs/package-author-guide.md) for the full Package Directory, Backend, UI, contract, and application-distribution rules. Read the UI SDK's [Package UI theme and CSS kit](packages/teamarness-package-ui-sdk/README.md) guidance for the exact semantic tokens, opt-in classes, cleanup contract, and Package-owned header and navigation model. Run `npm run validate:packages` to apply Package Admission's manifest and UI contract rules to every bundled Package's source and staged directory, with a Backend smoke launch, without starting the application; a generated project runs the same validator as `npm run validate`. Read [Local Package registration](docs/local-package-registration.md) for other registration layouts, restart rules, digest invalidation, and Package Data Directory continuity. A local registration does not grant Workspace enablement or synchronize, share, network, or collaborate Package code or data.

The application build composes the Reference, Tutorial, Conversations, Repo-aware, Documents, and Exhibits Packages into one verified index. Agent is built independently and registered locally. Run `npm run build` to build every bundled Package's Backend, UI entrypoint, declared assets, and manifest digests; it preserves the installation-local Reference Package copy used by development.

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
- `packages/reference-package/`, `packages/tutorial/`, `packages/conversations-package/`, `packages/repo-aware/`, `packages/documents-package/`, and `packages/exhibits-package/` are the bundled Packages, and `packages/teamarness-package-ui-sdk/` is the TypeScript UI SDK.
- `apps/teamarness-desktop/` contains the typed Tauri adapter and Shell.
- `p2p-local-first-user-management.md` is the supporting P2P membership research baseline.
