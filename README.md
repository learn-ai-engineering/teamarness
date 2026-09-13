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

Start the first named development Installation from the repository root:

```sh
npm run dev -- alice
```

The launcher stores its private runtime state under the ignored `.resonance-dev/alice` directory and creates the profile directory with owner-only permissions on platforms that support them. Profile names may contain 1 to 64 letters, numbers, underscores, or hyphens and must start with a letter or number.

After the first command builds the debug executable, start a second Installation without another Vite server:

```sh
npm run dev:instance -- bob
```

Each profile owns a distinct Installation identity and `core.sqlite3`. Reuse the same profile name to verify restart behavior, and never share one profile between running processes. The production runtime uses Iroh's default relay configuration; launching can complete while offline, but creating an invitation requires relay readiness.

An absolute directory is available as an expert override. Arbitrary relative paths are rejected:

```sh
npm run dev -- /absolute/private/resonance-one
npm run dev:instance -- /absolute/private/resonance-two
```

The lower-level `npm run tauri dev` command remains available for an ordinary launch using Tauri's bundle-specific application-data directory. The JavaScript bridge cannot select or inspect that directory. The direct debug binary also remains available for troubleshooting:

```sh
target/debug/resonance-desktop --resonance-data-dir /absolute/private/resonance-two
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
