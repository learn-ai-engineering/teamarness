# 02-system/05-packages — Package Requirements

Prefix: `TEAM.SYS.PKG`

The Package system defines Package Manifests, Sources, Registrations, Admission, the Package Host, Package Process Protocol, Semantic Capabilities, lifecycle, SDKs, local data ownership, and trust. All product-domain behavior lives in Packages. Phase 3 delivers the foundation after the desktop Shell and Workspace onboarding path work end to end.

## TEAM.SYS.PKG-R01 — Package Manifest

Every Package SHALL provide a Package Manifest declaring its stable Package Identity, compatibility requirements, Package Backend and Package UI entrypoints, and consumed versioned Semantic Capabilities. A Package Manifest SHALL NOT determine its Package Source, Package Registration Scope, Package Admission, or Package Trust. Teamarness SHALL validate the Package Manifest before Package Admission and SHALL reject invalid or unsupported declarations with an observable error.

## TEAM.SYS.PKG-R02 — Standard Lifecycle

The Package Host SHALL manage Package Instances through explicit `activate`, `suspend`, `recover`, and `dispose` lifecycle operations. A Package SHALL release lifecycle-scoped work during suspension and all owned resources during disposal. Hiding a Package UI or selecting another Workspace SHALL NOT itself suspend a Package Backend in Phase 3. When a Package Backend returns an error from a lifecycle operation, the Package Host SHALL leave the Package Instance in `Failed` and SHALL NOT consume an automatic Package Process restart attempt. Unexpected Package Process exits, uncancellable execution deadlines, and missed health probes SHALL remain eligible for up to three automatic restart attempts after the initial failure. For trusted Packages, returned lifecycle errors, lifecycle task failures, Package Process exits, uncancellable execution deadlines, missed health probes, failed mounts, and JavaScript exceptions SHALL enter a recoverable or disposable state rather than terminate the Shell or Core Runtime. The Package foundation does not promise safe execution of hostile code, an operating-system sandbox, or recovery from system-wide resource exhaustion.

## TEAM.SYS.PKG-R03 — Semantic Capability Contracts

A Package SHALL access Teamarness-owned authority and generic behavior only through declared, versioned Semantic Capabilities. Capability calls and events SHALL be typed and SHALL NOT expose private keys, raw secret bytes, Core Runtime database handles, Core Runtime types, or raw transport handles. Activation SHALL fail clearly when Teamarness cannot fulfill a required capability version. Phase 3 SHALL provide capabilities for canonical Workspace metadata and membership, membership transitions, historical membership checks, and domain-separated signing.

## TEAM.SYS.PKG-R04 — Backend and UI Boundary

A Package Backend SHALL run as a self-contained executable in one host-supervised Package Process for each Package Instance. The versioned, typed Package Process Protocol SHALL be language-neutral and SHALL define handshake, lifecycle, requests, responses, events, cancellation, and shutdown without a Rust ABI or Rust type contract. Phase 3 SHALL ship a Rust Backend SDK and Rust reference Package. A Package UI SHALL be TypeScript web content mounted by the Shell in a Tauri webview. Package UI and Backend traffic SHALL use Package-owned typed contracts routed through the generic Tauri invoke and event bridge.

## TEAM.SYS.PKG-R05 — Storage Ownership and Isolation

The Package Host SHALL assign each Package Instance a durable Package Data Directory keyed by Installation, Workspace, and Package Identity. The Package SHALL own its schemas, migrations, indexes, and stored data and MAY choose SQLite, files, or another local representation. The directory SHALL survive Package Source and version changes for the same Package Identity. No supported Teamarness contract SHALL expose Core Runtime storage or another Package's directory. This contract is not an operating-system filesystem boundary.

## TEAM.SYS.PKG-R06 — Common SDK

Packages from application, installation, and future Workspace registrations SHALL use the same Package Manifest, Package Process Protocol, lifecycle, Semantic Capabilities, UI contracts, data-directory contract, bridge, and theme contract. Phase 3 SHALL ship a Rust Backend SDK and TypeScript UI SDK but SHALL NOT require the Package Process Protocol to be implemented in Rust. Package Registration Scope and Package Trust policy MAY restrict discovery or execution but SHALL NOT create separate programming models.

## TEAM.SYS.PKG-R07 — Trust Boundary

Teamarness SHALL grant Package Trust during Package Admission. A Package Manifest SHALL NOT grant trust to its own Package. Phase 3 SHALL admit application-bundled Package Sources and explicitly registered installation-local Package Sources as trusted. The Package Process boundary SHALL protect the Shell and Core Runtime from ordinary Package exits and missed execution deadlines, but Phase 3 SHALL NOT claim that it safely contains hostile or unreviewed code or restricts ambient filesystem and network authority.

## TEAM.SYS.PKG-R08 — Reference Package

Phase 3 SHALL include one trusted reference Package loaded from both an application-bundled Package Source and an explicitly registered installation-local Package Source. Both SHALL use the same Package-owned code and contracts. The reference Package SHALL exercise Package Manifest validation, Package Admission, capability negotiation, lifecycle transitions, its Package Data Directory, typed calls and events, inherited Workspace theming, and supported failure recovery end to end without introducing product-domain behavior.

## TEAM.SYS.PKG-R09 — Source and Registration

Phase 3 SHALL discover Package Sources through application and installation Package Registrations. An installation-local registration SHALL identify a Package Directory containing one Package Manifest and its Package Artifacts. An installation registration MAY replace an application registration for the same Package Identity only when it carries the expected identity and explicit local trust evidence. No Package Source SHALL win implicitly. Configuration changes MAY require an application restart. Invalid paths, missing or incompatible Package Artifacts, malformed Package Manifests, unsupported versions, and unresolved duplicate Package Identities SHALL fail with stable observable errors without preventing other Packages or Teamarness from remaining usable. Teamarness SHALL NOT fetch remote Packages, resolve Package dependencies, or perform Package Git operations in Phase 3.

## TEAM.SYS.PKG-R10 — Synchronization Capability Boundary

A future Synchronization Capability SHALL provide generic, versioned, Package-scoped replication through Core Runtime-owned transport. The capability MAY support replicated append-only operations, mutable structured state, collaborative documents, or custom transport. A Package SHALL retain ownership of its domain records, schemas, projections, and policy. Phase 3 SHALL establish capability negotiation and routing that can add such a family without changing Package lifecycle, Package Process, or Package UI mounting contracts, but SHALL NOT implement a synchronization engine.

## TEAM.SYS.PKG-R11 — Native Trust Limits

A trusted Package Process MAY use ordinary native filesystem and network access under the current user's authority. Teamarness SHALL identify installation-local Packages and SHALL NOT describe process separation as a sandbox. Teamarness-owned identity, canonical membership, signing, and Iroh transport SHALL remain available only through Semantic Capabilities.
