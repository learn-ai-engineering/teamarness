# 0006 - Rust sidecar Package Backends

**Status:** accepted (2026-09-13)

## Context

ADR 0003 placed trusted Package Backends in the Teamarness process behind crate boundaries. That decision fit the earlier bundled-only scope. Phase 3 now requires the same Package to run from either application resources or an explicitly registered local Package Directory.

A prototype compared a native dynamic library, a WebAssembly component, and a Rust sidecar against one fixture. All three handled lifecycle calls, scoped durable storage, typed operations, ordered events, cancellation, restart recovery, and declared failures.

The native library failed the configured-local path under ordinary macOS hardened-runtime library validation. Loading it required either compatible publisher signing or the `com.apple.security.cs.disable-library-validation` entitlement. WebAssembly provided strong execution limits, but its WASI Preview 2 and WIT model did not prove the Tokio and Iroh background work required by a future Conversations Package. The Rust sidecar loaded from bundled and local paths, preserved ordinary Rust dependencies, and let Teamarness recover after Package aborts and stalled calls.

## Decision

Each Package Instance SHALL run its Rust Package Backend in a host-supervised Package Process. One Package Instance exists for one Package Identity, Workspace, and Installation.

Bundled and local Package Sources SHALL use the same Package Manifest, backend protocol, lifecycle, Semantic Capabilities, storage contract, UI contract, and Package-owned code. Their Package Registrations and distribution metadata may differ.

The Package Process SHALL communicate with Teamarness through a versioned, typed semantic protocol. Teamarness SHALL broker host-owned capabilities. The protocol SHALL NOT carry private keys, raw secret bytes, database handles, Core Runtime values, or raw Iroh handles.

Phase 3 Package Backends remain Rust-only. This decision does not establish a language-neutral Package SDK.

Teamarness SHALL grant Package Trust during Package Admission. A Package Manifest cannot grant trust to its own Package. Phase 3 admits application-bundled Packages and explicitly registered installation-local Packages as trusted. It does not admit hostile or unreviewed code.

A Package Process exit or missed execution deadline SHALL NOT terminate the Shell or Core Runtime. Teamarness may recover the Package by starting a replacement process or dispose it. This process boundary does not sandbox filesystem or network access and does not promise recovery from operating-system resource exhaustion or hostile behavior.

This decision supersedes only ADR 0003's in-process Package Backend execution and crate-level isolation consequences. ADR 0003's domain ownership, narrow Core Runtime boundary, Semantic Capability rules, and Package-owned UI, data, transport, and lifecycle remain accepted.

## Rejected alternatives

### Native dynamic libraries

Rejected as the standard Package Backend model. They require an unsafe stable ABI and share the host's abort, memory, and deadlock failures. More importantly, separately developed local libraries do not load under ordinary macOS hardened-runtime library validation unless Teamarness weakens library validation or controls Package signing.

### WebAssembly components

Deferred rather than prohibited. WIT typing, traps, and execution limits are valuable, but the prototype did not prove the Package-owned Tokio and Iroh work required by Conversations. Making WebAssembly the only Phase 3 model would commit Package authors to that unproved programming model.

### Rebuilding local Packages into Teamarness

Rejected. Rebuilding would make local development use a different integration and packaging path from bundled distribution. It would also turn Teamarness into a build orchestrator rather than a Package host.

## Consequences

- Teamarness needs a Package Process supervisor, not only a Package loader.
- The supervisor must define framing, version negotiation, correlation, cancellation, ordered events, payload limits, backpressure, deadlines, graceful shutdown, forced termination, restart limits, crash-loop handling, diagnostics, and orphan reconciliation.
- Tauri's bundled sidecar support may package application sources, but Teamarness still owns configured-local validation, admission, launch, and source conflict handling.
- Package-scoped storage and other host capabilities cross a typed process boundary.
- A Package Process has the current user's ambient operating-system authority unless a later decision adds platform sandboxing.
- WebAssembly remains available for a later Package class if Teamarness needs stronger execution limits.
- Target compatibility, code-signing diagnostics, and complete bundle behavior require target-specific verification.

## Evidence

- `.myflow/workstreams/package-foundation/prototype/20260913T131901Z_runtime-execution-model.md`
- `.myflow/workstreams/package-foundation/prototype/runtime-model/results/summary.md`
- `.myflow/workstreams/package-foundation/research/20260913T123921Z_runtime-loading-models.md`
- `context/.decisions/0003-core-runtime-vertical-packages.md`
