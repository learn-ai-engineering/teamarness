# Resonance

Resonance is a local-first, peer-to-peer team workspace. The current Phase 2 application combines a Rust Core Runtime with a small Tauri shell for creating a Workspace and admitting another Installation through an opaque copied invitation.

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
npm install
cargo fetch --locked
cargo build --workspace --locked
```

Use `npm ci` instead of `npm install` in a clean or automated environment when the committed `package-lock.json` must be reproduced exactly. Both Cargo and npm lockfiles are committed.

## Development

Start the desktop application from the repository root:

```sh
npm run tauri dev
```

An ordinary launch stores private runtime state in Tauri's bundle-specific application-data directory. The JavaScript bridge cannot select or inspect that directory.

For an independent test Installation, pass an absolute private directory to the Rust backend. Relative paths, repeated options, and missing values are rejected:

```sh
npm run tauri dev -- -- --resonance-data-dir /absolute/private/resonance-one
```

After the development executable has been built by the first command, a second instance can be launched without starting a second Vite server:

```sh
target/debug/resonance-desktop --resonance-data-dir /absolute/private/resonance-two
```

Each directory owns a distinct Installation identity and `core.sqlite3`. Do not share one directory between running processes. The production runtime uses Iroh's default relay configuration; launching can complete while offline, but creating an invitation requires relay readiness.

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

The Core Runtime public API is `resonance_core::CoreRuntime`. Identity secrets, raw key bytes, SQLite handles, Iroh transport handles, canonical membership bytes, and private membership adapters remain private. The Tauri bridge carries only owned Installation, Workspace, membership, invitation, and stable-error DTOs.

## Current-platform bundles

Build unsigned local macOS application and disk-image bundles with:

```sh
npm run tauri build -- --bundles app,dmg
```

The default outputs are:

- `target/release/bundle/macos/Resonance.app`
- `target/release/bundle/dmg/Resonance_0.1.0_aarch64.dmg` on Apple silicon

These artifacts are for local verification. Signing, notarization, distribution, auto-update, tray, and native-menu infrastructure are intentionally out of scope.

To run two packaged Installations with separate absolute directories, launch the application bundle twice:

```sh
open -n target/release/bundle/macos/Resonance.app --args --resonance-data-dir /absolute/private/resonance-one
open -n target/release/bundle/macos/Resonance.app --args --resonance-data-dir /absolute/private/resonance-two
```

Invitation values are sensitive bearer admission material. Do not store them, private keys, raw secret bytes, or private database contents in logs or verification notes.

## Core Runtime behavior

See [`crates/resonance-core/README.md`](crates/resonance-core/README.md) for the asynchronous API, Workspace onboarding and recovery contract, bootstrap limits, database ownership, declared-time history warning, and explicit exclusions.

The runtime exclusively owns one private `core.sqlite3`. Uncertain commit outcomes require dropping and reopening the runtime so authenticated replay can recover durable state. `was_member` is informational author-declared history and must not authorize writes.

## Repository intent

- `context/` is the canonical requirements, ontology, roadmap, and decision tree.
- `crates/resonance-core/` contains the Core Runtime library, API documentation, and tests.
- `apps/resonance-desktop/` contains the typed Tauri adapter and minimal TypeScript shell.
- `p2p-local-first-user-management.md` is the supporting P2P membership research baseline.
