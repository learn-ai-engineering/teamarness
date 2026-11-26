# 0049 — Installation-local Workspace rename

**Status:** accepted (2026-11-27)
**Supersedes in part:** ADR 0005 (Decision, "The name becomes immutable local metadata after import", as applied to the local Installation's own label)
**Refines:** ADR 0005 (import-time Workspace name)

## Context

A Workspace's name is fixed at create or import and is stored as local, unsigned metadata (ADR 0005). Don creates Workspaces to test with and has no way to rename them. The `workspace-settings-ui` workstream adds a rename control to Workspace settings.

Don's eventual intent is a name that synchronizes to every Member. ADR 0005 rejected signing the name into canonical membership and left durable signed Workspace metadata to a later model. This workstream does not build that model.

## Decision

1. **Rename is Installation-local.** Workspace settings lets a Member change the Workspace name stored on their own Installation. The name follows the existing Workspace name rules. Nothing is signed or sent to other Members, and their labels are unchanged.
2. **Invitations carry the current local name.** Invitations issued after a rename carry the issuer's current local name, as they carry the stored name today.
3. **The new name is visible at once locally.** The Shell and Core Workspace list show the new name at once; Package Instances observe it no later than their next start.
4. **Desktop only.** Rename is a Shell control. Core exposes the operation as a library call; the headless application does not offer it yet.

## Implementation notes

- Migration 0020 narrows `workspaces_no_update` to the trust-anchor columns, so only the local name may change.
- Re-importing a stored Workspace no longer compares its name with the inviter's, so a join retry survives a rename on either side.

## Rejected alternatives

- **Synchronized rename now:** needs the signed Workspace metadata model ADR 0005 deferred. Don chose Installation-local rename for this workstream; a later decision may replace it.

## Consequences

- `TEAM.SYS.SHELL-R04` gains the rename control.
- Two Members can know one Workspace by different names. Package UIs and external clients that show a Workspace name show the local one.
