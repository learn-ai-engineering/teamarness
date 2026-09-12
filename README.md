# Resonance

Resonance is a local-first, peer-to-peer team workspace. This repository is at the core-runtime foundation stage: it contains the Phase 1 Rust library and the intent and design records that govern its implementation.

## Prerequisites

- [Rust installed with rustup](https://rustup.rs/), version 1.96 or newer
- Node.js 24 or newer (required by the later TypeScript package and Tauri tooling)
- A C/C++ build toolchain supported by Rust; SQLite is built from the bundled source

Confirm the tools are available:

```sh
rustc --version
cargo --version
node --version
```

The dependency lockfile is committed. Do not replace exact core dependency pins or add the full `iroh` endpoint crate without an accepted design change. Phase 1 uses only `iroh-base` key support; transport binding belongs to a later roadmap phase.

## Setup

From a fresh clone:

```sh
cargo fetch --locked
cargo build --workspace --locked
```

No service, database server, Node package install, or network endpoint is needed for the current library.

## Checks

Run the repository's Rust checks from the repository root:

```sh
cargo build --workspace --locked
cargo test --workspace --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

To audit the Phase 1 dependency boundary:

```sh
cargo tree -p resonance-core -e features
cargo tree -p resonance-core -i iroh
```

The inverse `iroh` query should report that no matching package exists. The core crate's public API is `resonance_core::CoreRuntime`; identity secrets, raw key bytes, SQLite handles, Iroh transport handles, private membership adapters, and mutable p2panda state remain private.

## Core runtime behavior

See [`crates/resonance-core/README.md`](crates/resonance-core/README.md) for the Phase 1 API, the declared-time history warning, database ownership and recovery model, and explicit exclusions. In particular, `was_member` is informational author-declared history and must not authorize writes. The runtime exclusively owns one private `core.sqlite3`; uncertain commit outcomes require dropping and reopening the runtime so authenticated replay can recover durable state.

Phase 1 is single-manager and has no networking or membership synchronization. Multi-manager conflicts, distributed invitation spending, human identity, package behavior, package-key rotation, transport, and UI remain deferred to their accepted later phases.

## Repository intent

- `context/` is the canonical requirements, ontology, roadmap, and decision tree.
- `crates/resonance-core/` contains the Phase 1 runtime library, its API documentation, and tests.
- `p2p-local-first-user-management.md` is the supporting P2P membership research baseline.
