# DELTA-018 — Packagement findings accepted as tech debt

**Status:** open (2026-10-31). Don accepted these dispositions at the Packagement close on 2026-10-31. They are deferred, not fixed. Owner: the follow-up named in each group.

## Divergence

The Packagement workstream (ADR 0036, ADR 0037, `TEAM.SYS.PKG-R09`, `TEAM.SYS.PKG-R13`, `TEAM.SYS.PKG-R19`) passed Verify with these review findings left open. None of them breaks ordinary use. Each group below has a follow-up destination.

### 1. Agent leases and route re-resolution

- **CR-502.** An unhealthy Personal Provider keeps serving Agent subscriptions that are already open. New calls and discovery already exclude it. Publishing a new Port snapshot does not revoke leases, and `revoke_provider` runs only when the Provider's process exits. `docs/local-package-registration.md` describes the exclusion for new calls and discovery.
- **CR-503.** When a Package enters, the Host does not re-resolve that Package's own routes. In a cyclic dependency, its optional route to a Provider that it makes healthy stays unavailable until the next Personal change.
- **Restart with an unresolved command.** A test covers reloading a Personal Agent while it holds an unresolved command: the command is dispatched at most once and never replayed. Per-Package **Restart** (`restart_personal`) uses a different path, and no test holds a command during it.
- **Stale Package Ports 1.0 install.** During the manual check, a Personal Agent built for Package Ports major 1 was installed live, and it failed only at runtime ("Needs recovery"). It is not yet confirmed whether admission on `main` also accepts a 1.0 declaration. If it does, Install should refuse it with a clear message.

Follow-up: Host Port maintenance on `main`.

### 2. Override and message UX

- **CR-201, SF-201, SM-106.** Override flow: a refused override shows "Source: Team"; cancelling a pending override navigates away; the pending-removal note repeats the success message.
- **C5-001.** On a Package page, **Remove** followed by **Cancel** clears the Workspace page's Recover or Dispose message and its Retry.
- **SF-304, SF-005, SM-005, SM-305, SM-202, SM-401, SM-508.** Messages and copy: a timed success can hide the only Restart control; a registration refused at startup shows no capabilities; Remove confirmation wording is duplicated; an unused Dismiss label; branch reachability; near-synonym message-clearing helpers; lifecycle action labels duplicated in `package-install.ts` and `shell.ts`.
- **CR-003.** A failed replacement after retirement can leave the earlier Personal Package stopped. Reload or Remove recovers it. A transactional rollback is a redesign.

Follow-up: an override UX slice, or the `package-builder` workstream.

### 3. Contract and type consolidation

- **SM-003, SM-004, SM-101, SM-102, SM-204.** Team and Personal identity views, the choice of command for `override_required`, the remove-override outcome taken from a second snapshot, and Team-entry classification repeated in the Host, the Rust bridge and TypeScript. One authoritative field would be a contract change.
- **SM-002, SM-007, SM-104, SM-203, SM-510.** Groupings of immutable state, inspected types reused for admitted data, admission result separated from runtime overlay, an optional snapshot parameter, and positional Team and Personal Provider maps in `PortSnapshot::new`.
- **SM-507.** The TypeScript contract test's `expectKeys` checks a subset, not equality. The SM-009 fixtures and the Rust side still pin the shapes.
- **SM-107.** A stale comment and the rename that goes with it.

Follow-up: the next Package bridge contract change.

### 4. Style and test cleanup

- **SM-303, SM-304, SM-013.** Repeated widths, padding and stylesheet values.
- **SM-006 / SM-301.** A shared, tested HTML-escaping helper. No escaping defect is shown.
- **SM-206, SM-302, CR-007.** A test fixture with impossible states, a manual timer helper copied into four test files, and a permission test that does not reach the branch that creates the directory.

Follow-up: the next stylesheet or test change in these files.

### Accepted or declined, not deferred

- Accepted: SF-004 (the authoring template leaves author and homepage empty), SF-302, CR-005 (concurrent hand edits are unsupported and documented), SF-402.
- Declined: SM-008 (wire rename), SM-402 (naming churn).

## Impact

No ordinary install, reload, remove, restart or Agent reply flow is affected. Don's manual checks on 2026-10-31 passed. Group 1 needs either an unhealthy Provider with an open Agent subscription, a cyclic dependency, a restart during a held command, or a stale build. Groups 2 to 4 cover copy, duplication or maintainability.

## Tracking

- ADR 0036, ADR 0037, `TEAM.SYS.PKG-R13`, `context/roadmap.md` (Packagement).
- Workstream `packagement`, in the resolver-selected MyFlow store: `verify/20261031T030000Z_p2-dispositions-ports-2.md` (signed off), `verify/20261031T024307Z_review-ports-2.md`, `verify/20261031T040000Z_review-c5.md`, and `verify/20261030T230150Z_p2-triage.md`.
