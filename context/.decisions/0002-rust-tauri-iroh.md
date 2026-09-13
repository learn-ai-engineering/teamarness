# 0002 — Rust, Tauri, and Iroh for the full stack

**Status:** accepted (2026-09-12)

## Context

Teamarness needs a desktop shell, a core runtime, a P2P transport layer, and an identity foundation. These choices are interdependent and effectively irreversible — changing the shell or transport after packages exist would be a rewrite. The decision must be made before any code is written.

## Decision

### Rust + Tauri for the desktop shell and runtime

- **Rust** for the core runtime, all backend logic, and the native desktop layer.
- **Tauri v2** for the desktop shell — window management, system tray, native menus, auto-updater. Tauri provides the thin native bridge; all application behavior lives in the Rust runtime.
- **TypeScript** for package UIs only — the web content rendered in Tauri webviews. The package SDK and semantic capability contracts are TypeScript, matching the existing Teamarness contract model.

The runtime is a Rust crate (or small set of crates). Packages have TypeScript UIs that mount into Tauri webviews. The runtime-to-package bridge uses Tauri's invoke/event system with strict typed contracts, matching the runtime-facade-shell-bridge pattern from the reference design.

### Iroh for all P2P transport

- **Iroh** (`iroh` crate, currently 1.2.0) for all peer-to-peer connectivity: QUIC transport, hole-punching, relay fallback, and peer discovery via DNS/pkarr.
- Iroh provides Ed25519-based `EndpointId` as the peer identity primitive. Each device/endpoint has a unique `SecretKey` whose public key is its network identity.
- Iroh explicitly does NOT provide membership, authorization, or group management. That is a clean seam — we build the membership layer on top of Iroh's transport and identity primitives.

### Future data capabilities remain open

Iroh blobs and Iroh docs are candidates for future content-addressed or collaborative data features, but this decision does not adopt either one. A later package design will choose the storage and synchronization mechanism for each feature.

Packages must consume those features through capability contracts exposed by the core runtime rather than depend directly on Iroh. The runtime may use Iroh blobs, Iroh docs, or another implementation behind those contracts once the relevant trade-offs have been evaluated.

### Why not Electron

Electron was considered but rejected because:

1. **One language for the runtime.** Rust gives us compile-time ownership guarantees, a strong type system, and zero-cost abstractions that TypeScript cannot match. The runtime is security-sensitive (key custody, membership log, cryptographic operations); Rust is the right tool for that boundary.
2. **Iroh is Rust.** Using Iroh from Electron would require WASM compilation or a sidecar process — adding build complexity and a protocol boundary where none is needed. In Rust, Iroh is a direct dependency.
3. **Tauri is lighter.** Tauri binaries are smaller, use less memory, and feel more native than Electron. For a team application that runs alongside IDEs and browsers, resource usage matters.
4. **AI changes the accessibility calculus.** With AI assistance, a developer can be productive in a Rust codebase without deep Rust expertise. Package authors write TypeScript; reading Rust with AI help is sufficient for understanding the runtime.

### Why not Commonware

Commonware was considered for the conversations transport layer (as used in the reference Teamarness V1). Rejected because:

1. **Iroh provides the required transport stack.** QUIC transport, hole-punching, relay fallback, and discovery come from one dependency with a consistent design. Its blobs and docs capabilities remain options for later package features rather than part of this decision.
2. **Documentation and ecosystem.** Iroh has strong documentation, video content, and an active community. A single well-documented dependency reduces the integration burden compared to assembling transport, discovery, and blob storage from separate crates.
3. **Commonware is also Rust, but younger.** Commonware's p2p crate is newer and less documented than Iroh. The reference Teamarness V1 used Commonware because it was already in the stack; this project starts fresh and can choose the more mature option.

## Consequences

- The development environment requires Rust (via rustup) and Node.js (for package UIs and tooling). Students install both.
- The runtime, transport, and shell are all Rust. Package UIs are TypeScript. The boundary is the Tauri invoke/event bridge.
- Membership, invitations, revocation, and key rotation are built on top of Iroh's transport and identity — not provided by Iroh itself.
- Packages do not depend directly on Iroh. Future content-addressed or collaborative data features use core-runtime capability contracts, with their backing implementation decided separately.
- Iroh's relay servers (number 0 by default) provide NAT traversal fallback. We can configure custom relays or use the default public relays.
- The reference Teamarness V1 codebase (Rust + Tauri + Commonware for chat) remains valuable as a design reference but is not directly copied.
