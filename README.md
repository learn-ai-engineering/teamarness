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

Each profile owns a distinct Installation identity and `core.sqlite3`. When creating or joining a Workspace, choose a Workspace-specific Person name; members can rename themselves in Workspace settings. Conversations shows remote authors' current names, updates already displayed Messages after rename, and retains an inspectable full Installation Public Identity. Duplicate or missing names keep an identity fallback; your own Messages remain `You`. Two-profile manual rename/restart verification for this incremental workstream is still outstanding. Reuse the same profile name to verify restart behavior, and never share one profile between running processes. The production runtime uses Iroh's default relay configuration; launching can complete while offline, but creating an invitation requires relay readiness.

An absolute directory is available as an expert override. Arbitrary relative paths are rejected:

```sh
npm run dev -- /absolute/private/teamarness-one
npm run dev:instance -- /absolute/private/teamarness-two
```

The lower-level `npm run tauri dev` command remains available for an ordinary launch using Tauri's bundle-specific application-data directory. The JavaScript bridge cannot select or inspect that directory. The direct debug binary also remains available for troubleshooting:

```sh
target/debug/teamarness-desktop --data-dir /absolute/private/teamarness-two
```

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

The bundled Packages are listed once in `packages/bundled-packages.json`. After editing it, run `node scripts/bundled-packages.mjs --write` and `npm run build`; `npm test` fails while the Tauri bundle configuration or the application index disagrees with the list.

The Core Runtime public API is `teamarness_core::CoreRuntime`. Identity secrets, raw key bytes, SQLite handles, Iroh transport handles, canonical membership bytes, and private membership adapters remain private. The Tauri bridge carries only owned Installation, Workspace, membership, invitation, and stable-error DTOs.

## Current-platform bundles

Build unsigned local macOS application and disk-image bundles with:

```sh
npm run tauri build -- --bundles app,dmg
```

The default outputs are:

- `target/release/bundle/macos/Teamarness.app`
- `target/release/bundle/dmg/Teamarness_0.1.0_aarch64.dmg` on Apple silicon

These artifacts are for local verification. Signing, notarization, distribution, auto-update, tray, and native-menu infrastructure are intentionally out of scope.

After the bundle command succeeds, run the automated packaged-app smoke on Apple silicon macOS:

```sh
npm run verify:packaged-app
```

The smoke mounts the DMG for inspection, compares every bundled Package Directory with its staged bytes, launches the `.app` twice with private temporary data, creates a Workspace, checks that every bundled Package Backend launches from the application bundle, selects the bundled reference Package through macOS accessibility, checks its admitted UI and protocol response, and verifies process-group reaping after each graceful exit. It requires Accessibility permission for the launching terminal. The developer-owned app and DMG walkthroughs remain separate Verify checks.

To run two packaged Installations with separate absolute directories, launch the application bundle twice:

```sh
open -n target/release/bundle/macos/Teamarness.app --args --data-dir /absolute/private/teamarness-one
open -n target/release/bundle/macos/Teamarness.app --args --data-dir /absolute/private/teamarness-two
```

Invitation values are sensitive bearer admission material. Do not store them, private keys, raw secret bytes, or private database contents in logs or verification notes.

Teamarness stores the Installation secret and Content Keys unencrypted in `core.sqlite3` inside the private data directory (ADR 0017). Any process running as your user, including trusted Package Backends, can read them. Use FileVault and encrypted backups, and keep development profiles out of cloud-synced folders. The Core Runtime refuses a data directory that is a symbolic link, belongs to another user, or holds unrelated files.

Builds from this branch refuse data directories created before it. Reset a development profile with `rm -rf .teamarness-dev/<profile>`, and the packaged app with `rm -rf ~/Library/Application\ Support/dev.teamarness.desktop`.

## Package development

Start with [Build your first Package with an agent](docs/build-your-first-package.md). It begins with the finished Tutorial, explains the boundary between the Teamarness Core Runtime and a Package, then walks through generating, building, registering, and using an external Emojart project. Generate Emojart outside the Teamarness repository with `npm run create:package -- --output <directory> --identity dev.teamarness.emojart --display-name Emojart`. Read the [Package author guide](docs/package-author-guide.md) for the full Package Directory, Backend, UI, contract, and application-distribution rules. Read the UI SDK's [Package UI theme and CSS kit](packages/teamarness-package-ui-sdk/README.md) guidance for the exact semantic tokens, opt-in classes, cleanup contract, and Package-owned header and navigation model. Run `npm run validate:packages` to apply Package Admission's manifest and UI contract rules to every bundled Package's source and staged directory, with a Backend smoke launch, without starting the application; a generated project runs the same validator as `npm run validate`. Read [Local Package registration](docs/local-package-registration.md) for other registration layouts, restart rules, digest invalidation, and Package Data Directory continuity. A local registration does not grant Workspace enablement or synchronize, share, network, or collaborate Package code or data.

The application build composes the Reference, Tutorial, Conversations, and Repo-aware Packages into one verified index. Run `npm run build` to build every bundled Package's Backend, UI entrypoint, declared assets, and manifest digests; it preserves the installation-local Reference Package copy used by development.

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
- `packages/reference-package/`, `packages/tutorial/`, `packages/conversations-package/`, `packages/repo-aware/`, and `packages/documents-package/` are the bundled Packages, and `packages/teamarness-package-ui-sdk/` is the TypeScript UI SDK.
- `apps/teamarness-desktop/` contains the typed Tauri adapter and Shell.
- `p2p-local-first-user-management.md` is the supporting P2P membership research baseline.
