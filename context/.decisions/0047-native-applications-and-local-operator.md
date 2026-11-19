# 0047 — Shared native applications and local operator authority

**Status:** accepted (2026-11-18)
**Supersedes in part:** ADR 0036 (Decision 6, “Only the Shell webview may install, reload, remove or write overrides”)
**Refines:** ADR 0007 (native Package trust), ADR 0017 (private data custody), ADR 0040 (desktop terminal restart ordering)

## Context

Core Runtime and Package Host are libraries independent of Tauri. Eager Package Instances operate across joined Workspaces without visible Package UIs. A graphical runtime is unnecessary for an always-on replica, but Installation storage, process recovery and Package admission need the same guarantees in either application.

Native Packages retain the operating-system user's ambient authority. An owner-only local operator interface is not a new security boundary against same-user code. Agent and External Gateway remain independently distributed Personal Packages, not prerequisites for synchronization or local administration.

## Decision

1. One Rust headless application composes the existing Core Runtime and Package Host. Desktop retains its Tauri Shell, frontend, updater and release tooling. No Package-specific dispatch, synchronization copy or second Package protocol is introduced.
2. Production applications opt into exclusive Core opening. Core validates and prepares its private directory before creating a stable owner-only `application.lock`, then acquires a nonwaiting kernel lock before database recovery or network startup. The lock is never routinely unlinked. Symlinks, nonregular files, foreign owners, additional hardlinks and nonprivate permissions are refused without changing the lock target. The descriptor is close-on-exec. Retirement explicitly unlocks the open file description before closing it, so a temporary descriptor duplicate during spawn cannot postpone handoff. Library opens remain explicitly nonexclusive by default for transactional callers.
3. Ownership lasts through application cleanup. Desktop closes admission, drains admitted operations, and successfully shuts down Host then Core before explicitly retiring ownership immediately before replacement startup. Core retirement fences subsequent local commands. Failed, timed-out or uncertain cleanup retains ownership until process exit; a late successful cleanup cannot authorize restart. This accounts for Tauri starting a replacement before the old process exits, without depending on Arc destruction or a startup delay.
4. The headless foreground server owns the Installation. Short-lived clients use a private, same-UID Unix socket, never open a parallel Core Runtime or initialize a missing profile. The operator has the local person's full application authority, separate from gateway credentials and restricted Port access. Explicit expected identity and native-code consent remain mandatory for Personal installation. This extends ADR 0036's application admission authority, not Package capabilities. Team overrides remain restart-bound; new operator override controls are not required.
5. UI-free Package configuration uses ordinary admitted request contracts and generation-bound Host request leases, without rendering a frame. One operator request owns each Instance through dispatch, response and cleanup, even if its client disconnects. Metadata comes from admitted contracts; invocation compares the caller's explicit contract digest before execution. No retry of uncertain mutation, private configuration-file editing, new capability family or Package wire-version change is authorized.
6. Each build selects exactly one native application and target: Apple Silicon desktop, Apple Silicon headless, or Ubuntu 26.04 x86_64 GNU/Linux headless. Cross-compilation and Linux desktop are excluded. Host derives its Backend target from Cargo's compiled target, not a caller claim. Package assembly writes only the selected Backend declaration and verifies final bytes; macOS signing precedes hashing. Explicit resource roots isolate headless output from desktop staging. Checked-in source manifests remain templates, not falsely relabeled built artifacts.
7. Linux recovery uses boot identity, kernel process start ticks, canonical executable, executable digest, dedicated process group and exact launch-token argument. Missing process evidence clears obsolete leases; mismatched or unreadable evidence remains unresolved and blocks relaunch. No PID-only reclamation is permitted. macOS retains its existing platform evidence adapter.

## Rejected alternatives

- Hidden Tauri window or virtual display: preserves a graphical runtime dependency.
- Separate Core or Package implementations: duplicates authority and synchronization policy.
- PID files or deleting leases on startup: neither proves ownership of a surviving process.
- Releasing the lock during ordinary Core shutdown: Host or admitted application work may still use storage.
- Waiting for an Arc destructor before desktop restart: replacement startup can precede old-process exit.
- Gateway as administrator or required bundle member: couples application bootstrap to an independently owned Package.
- Backend-only manifests or headless-specific Package identities: unnecessarily forks admission and author contracts.

## Consequences

The private data directory remains readable by same-user processes, including trusted Package Backends. Account isolation and encrypted disks/backups remain the custody assumptions; the kernel ownership lock is not a sandbox.

The existing desktop build/release path remains independent. Linux desktop, launchd integration, public headless signing/publication, Agent Linux behavior and gateway Linux behavior remain separate work. Native Linux execution and real service/logout/reboot observations are mandatory qualification, not inferred from Mac tests.

Implementation is staged: native prerequisites precede the server, operator commands, admitted schema adapter, explicit application selector, and final sync-only qualification. This decision records accepted intent, not a claim those later components already ship.

## Evidence

- `crates/teamarness-core/src/runtime/{mod.rs,ownership.rs,types.rs}` and `tests/application_ownership.rs`: opt-in ownership, unsafe lock refusal, stable identity, explicit retirement, process death, exec noninheritance and macOS duplicate-descriptor retirement.
- `apps/teamarness-desktop/src-tauri/src/lifecycle.rs` and `lifecycle/tests/restart.rs`: ordinary/updater terminal handoff after drain and cleanup, including failed cleanup.
- `crates/teamarness-package-host/{build.rs,src/values.rs,src/supervisor.rs,src/supervisor/linux_identity.rs}` and `tests/{orphan_reconciliation,macos_process_identity,linux_process_identity}.rs`: compiled target and platform lease evidence.
- `scripts/{package-directory,build-package,build-application-packages}.mjs` and their tests: final-byte digests, target selection, signing order and isolated resource roots.
- Weaknesses: Linux-specific tests require a native Linux runner; Mac execution does not qualify `/proc`, Linux locks or service behavior. Full desktop UI and installed restart observations remain separate from deterministic terminal tests.
