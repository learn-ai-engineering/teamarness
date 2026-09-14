# 0007 - Language-neutral Package host capabilities

**Status:** accepted (2026-09-13)

## Context

ADR 0003 assigned each Package its transport mesh, delivery, acknowledgement, and recovery. ADR 0006 later selected a supervised sidecar but required every Phase 3 Backend to be a Rust program.

Those choices put two unrelated burdens on Package authors. The Rust-only contract tied host interoperability to the first SDK. Package-owned synchronization would require every Package author to understand Iroh connections, membership changes, retries, convergence, and peer reconciliation.

Phase 3 needs a smaller contract. A Package author should own domain records and policy while Teamarness retains its authority and can later supply reusable synchronization models.

## Decision

The Package Process Protocol SHALL be a versioned, language-neutral executable contract. It SHALL NOT depend on a Rust ABI or Rust types. Phase 3 SHALL ship only a Rust Backend SDK and Rust reference Package. A conforming black-box executable that does not use that SDK remains a valid protocol implementation.

The Core Runtime SHALL provide Teamarness-owned authority and generic behavior through versioned Semantic Capabilities. These capabilities include canonical Workspace metadata and membership, membership transitions, historical membership checks, and domain-separated signing.

Future peer synchronization SHALL enter the Package contract as generic, Package-scoped Synchronization Capabilities. The Core Runtime SHALL own the reusable Iroh transport, replication, retry, convergence, and peer-reconciliation machinery. A Package SHALL own its domain records, schemas, projections, authorization policy, and the mapping between its domain operations and a selected capability.

Phase 3 SHALL establish capability negotiation and routing but SHALL NOT implement a synchronization engine. Candidate future families include replicated append-only operations, mutable structured state, collaborative documents, and a custom transport escape hatch. Their exact semantics remain future design work.

The Package Host SHALL assign each Package Instance a durable Package Data Directory keyed by Installation, Workspace, and Package Identity. The Package owns its local representation and may use SQLite, files, indexes, or another format. Source and version changes for the same identity preserve the directory. This supported contract does not expose Core Runtime storage or another Package's directory.

Package Backends are trusted native processes. They retain the current user's ambient filesystem and network authority. Process separation is a recovery boundary for ordinary exits and missed deadlines, not a sandbox.

A Package Instance's Backend lifecycle SHALL remain independent of Package UI mounting and visibility in Phase 3. Hiding a Package UI or selecting another Workspace does not itself suspend the Backend.

An installation Package Registration MAY replace an application registration for the same Package Identity only through an explicit expected-identity override with local trust evidence. No source wins implicitly.

This decision supersedes these parts of earlier decisions:

- ADR 0006's requirement that every Phase 3 Package Backend be a Rust program and its statement that the decision does not establish a language-neutral contract.
- ADR 0003's assignment of all Package transport, delivery, acknowledgement, and recovery machinery to each Package.
- ADR 0003 and ADR 0006 wording that implies host-proxied storage is the only supported local data model.

ADR 0003's product-domain ownership and narrow Core Runtime authority remain accepted. ADR 0006's supervised sidecar process model and failure limits remain accepted.

## Rejected alternatives

### Rust-only process contract

Rejected because an executable protocol does not need a language restriction. Keeping the protocol neutral avoids a second host interface when another SDK is added. Shipping only the Rust SDK in Phase 3 keeps the implementation scope honest.

### Package-owned peer synchronization

Rejected for common replication models. It would duplicate transport, membership gating, retry, and convergence machinery across Packages and expose Iroh concepts to authors who only need domain synchronization. Direct native networking remains possible under the trusted-code model, but it is not the Teamarness-supported synchronization path.

### Host-proxied local storage

Rejected as the only local storage path. Proxying SQLite or filesystem operations would constrain Package data models without creating a security boundary. The Package Process already has ambient user authority.

### UI visibility controls Backend lifetime

Rejected because background synchronization and notifications may need to continue while another Package or Workspace is visible. A later resource policy may add suspension rules based on measured need.

## Consequences

- Protocol conformance and SDK support are separate claims.
- Phase 3 needs a non-Rust black-box conformance fixture even though production SDK delivery is Rust-only.
- The Package Host needs generic capability negotiation and routing before any synchronization engine exists.
- Phase 4 must design the first Synchronization Capability from Conversations requirements rather than add Conversations code to the Core Runtime.
- Package storage continuity follows Package Identity, Workspace, and Installation rather than Package Source or version.
- The Shell can change Package UI Mounts without using visibility as an implicit lifecycle signal.
- Documentation and diagnostics must state that local Package Processes are trusted native code.

## Evidence

- `.myflow/workstreams/package-foundation/scope/20260913T212911Z_package-host-and-capability-direction.md`
- `.myflow/workstreams/package-foundation/domain-model/20260913T214230Z_package-host-capabilities-and-ui-lifecycle.md`
- `.myflow/workstreams/package-foundation/prototype/20260913T131901Z_runtime-execution-model.md`
- `context/.decisions/0003-core-runtime-vertical-packages.md`
- `context/.decisions/0006-rust-sidecar-package-backends.md`
