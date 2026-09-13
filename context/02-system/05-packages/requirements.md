# 02-system/05-packages — Package Requirements

Prefix: `TEAM.SYS.PKG`

The package system defines manifests, semantic capabilities, lifecycle behavior, the TypeScript SDK, storage ownership, and trust boundaries. All product-domain capability lives in packages. The package foundation is scheduled for Phase 3, after the desktop shell and workspace onboarding path work end to end.

## TEAM.SYS.PKG-R01 — Package Manifest

Every package SHALL provide a manifest declaring its stable identity, scope, trust level, backend and UI entrypoints, and consumed versioned semantic capabilities. The runtime SHALL validate the manifest before package activation and SHALL reject invalid or unsupported declarations with an observable error.

## TEAM.SYS.PKG-R02 — Standard Lifecycle

The runtime SHALL manage packages through explicit `activate`, `suspend`, `recover`, and `dispose` lifecycle operations. A package SHALL release lifecycle-scoped work during suspension and all owned resources during disposal. For trusted bundled packages, returned errors, lifecycle task failures, unwind-safe panics caught at host boundaries, failed mounts, and JavaScript exceptions SHALL enter a recoverable or disposable state rather than terminate the shell or core runtime. The package foundation does not promise recovery from process aborts, memory exhaustion, undefined behavior, deadlocks, or hostile in-process code.

## TEAM.SYS.PKG-R03 — Semantic Capability Contracts

A package SHALL access runtime behavior only through declared, versioned semantic capabilities. Capability calls and events SHALL be typed and SHALL NOT expose private keys, raw secret bytes, core database handles, core-internal types, or raw transport handles. Activation SHALL fail clearly when the runtime cannot fulfill a required capability version.

## TEAM.SYS.PKG-R04 — Backend and UI Boundary

A package backend SHALL be a Rust module isolated from core internals by crate boundaries. A package UI SHALL be TypeScript web content mounted by the shell in a Tauri webview. Backend and UI communication SHALL cross the Tauri invoke/event bridge through the package's declared contracts.

## TEAM.SYS.PKG-R05 — Storage Ownership and Isolation

Each package SHALL own its domain schema, migrations, and stored data. The runtime SHALL provide durable package-scoped storage access that prevents a package from reading or mutating core storage or another package's storage. The underlying database layout remains a Phase 3 design decision tracked by `TEAM-DQ3`.

## TEAM.SYS.PKG-R06 — Common SDK

Bundled, team, and individual packages SHALL use the same TypeScript SDK and lifecycle contract. Scope and trust policy MAY restrict how a package is discovered or executed, but SHALL NOT create separate programming models.

## TEAM.SYS.PKG-R07 — Trust Boundary

Phase 3 SHALL support bundled trusted packages. The manifest SHALL declare package trust and scope so later phases can add member-loaded packages without changing the manifest or lifecycle concepts. Phase 3 SHALL NOT claim that its backend execution model safely contains untrusted code.

## TEAM.SYS.PKG-R08 — Reference Package

Phase 3 SHALL include a bundled reference package that exercises manifest validation, capability negotiation, lifecycle transitions, package-scoped durable storage, typed bridge calls and events, inherited workspace theming, and contained failure recovery end to end. It SHALL demonstrate the package foundation without introducing product-domain behavior.
