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

Each profile owns a distinct Installation identity and `core.sqlite3`. Reuse the same profile name to verify restart behavior, and never share one profile between running processes. The production runtime uses Iroh's default relay configuration; launching can complete while offline, but creating an invitation requires relay readiness.

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
npm ci
npm run build
npm run typecheck
npm run lint
npm test
cargo build --workspace --locked
cargo test --workspace --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

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

The smoke mounts the DMG for inspection, launches the `.app` twice with private temporary data, creates a Workspace, selects the bundled reference Package through macOS accessibility, checks its admitted UI and protocol response, and verifies process-group reaping after each graceful exit. It requires Accessibility permission for the launching terminal. The developer-owned app and DMG walkthroughs remain separate Verify checks.

To run two packaged Installations with separate absolute directories, launch the application bundle twice:

```sh
open -n target/release/bundle/macos/Teamarness.app --args --data-dir /absolute/private/teamarness-one
open -n target/release/bundle/macos/Teamarness.app --args --data-dir /absolute/private/teamarness-two
```

Invitation values are sensitive bearer admission material. Do not store them, private keys, raw secret bytes, or private database contents in logs or verification notes.

## Package development

Start with [Build your first Package with an agent](docs/build-your-first-package.md). It distinguishes checked-in Tutorial source, bundled Tutorial, and learner-owned local Emojart. Run `npm run build:application-packages` when building the application distribution, then generate Emojart outside the Teamarness repository with `npm run create:package -- --output <directory> --identity dev.teamarness.emojart --display-name Emojart`. Read the [Package author guide](docs/package-author-guide.md) to construct a Package Directory, build its Backend executable and ESM UI, declare a manifest and UI contract, and promote tested bytes into application distribution. Read [Local Package registration](docs/local-package-registration.md) for the owner-only registration file, restart rules, digest invalidation, and Package Data Directory continuity. Package Source kind does not grant Workspace enablement or synchronize, share, network, or collaborate Package code or data.

The protocol and supported Rust tooling have separate contracts:

- [`contracts/package-process/v1/README.md`](contracts/package-process/v1/README.md) is the normative language-neutral process protocol.
- [`crates/teamarness-package-protocol/README.md`](crates/teamarness-package-protocol/README.md) describes its Rust adapter.
- [`crates/teamarness-package-host/README.md`](crates/teamarness-package-host/README.md) describes the host facade and lifecycle.
- [`crates/teamarness-package-sdk/README.md`](crates/teamarness-package-sdk/README.md) describes the Rust Backend SDK.

Package trust is broad. Package Processes have the current user's ambient filesystem, network, and process authority. Process separation is a recovery boundary, not a sandbox. A same-webview Package UI shares DOM and known Tauri-command authority with the Shell. Its Shadow Root provides style locality, not a security boundary. Only install or bundle Package code you trust.

## Core Runtime behavior

See [`crates/teamarness-core/README.md`](crates/teamarness-core/README.md) for the asynchronous API, Workspace onboarding and recovery contract, bootstrap limits, database ownership, declared-time history warning, and explicit exclusions.

The runtime exclusively owns one private `core.sqlite3`. Uncertain commit outcomes require dropping and reopening the runtime so authenticated replay can recover durable state. `was_member` is informational author-declared history and must not authorize writes.

## Repository intent

- `context/` is the canonical requirements, ontology, roadmap, and decision tree.
- `crates/teamarness-core/` contains the Core Runtime library, API documentation, and tests.
- `crates/teamarness-package-host/`, `crates/teamarness-package-protocol/`, and `crates/teamarness-package-sdk/` contain the Package runtime and supported Rust tooling.
- `packages/reference-package/` is the bundled reference Package and `packages/teamarness-package-ui-sdk/` is the TypeScript UI SDK.
- `apps/teamarness-desktop/` contains the typed Tauri adapter and Shell.
- `p2p-local-first-user-management.md` is the supporting P2P membership research baseline.
