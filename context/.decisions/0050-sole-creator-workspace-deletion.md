# 0050 — Sole-creator Workspace deletion

**Status:** proposed (2026-11-26)
**Refines:** `TEAM-DQ9` (Package Instance lifecycle, for deletion only)

## Context

Don creates Workspaces to test with and has no way to remove them. The `workspace-settings-ui` workstream adds a delete control to Workspace settings.

Deleting a Workspace with other Members would need a membership action, Content Key handling, and a decision on Package Instance lifecycle for leave and revocation (`TEAM-DQ9`, roadmap Phase 5). Deleting a Workspace that only its creator belongs to needs none of those.

## Decision

1. **Only the creator deletes, and only when alone.** Deletion is offered only on the creator's Installation, and only while canonical membership has no current Person other than the creator. The creator's own Agents do not block deletion and are deleted with it. Revoked former Members do not block deletion. Core checks this rule in the same transaction that erases the Workspace and refuses with a distinct error if it no longer holds, for example after an admission that arrived while the confirmation was open.
2. **Deletion erases the whole Workspace on this Installation.** Core removes every row that belongs to the Workspace: genesis, membership, Principals and names, invitations, records, Content Keys, Agent bindings and synchronization state. Outstanding invitations die with it. The Package Host stops and disposes the Workspace's Package Instances and removes their Package Data Directories. Deletion is not sent to any other Installation.
3. **Only Teamarness-owned data is removed.** Files outside Teamarness's own directories are never deleted. In particular, folders and files mapped by Documents folder sync stay on disk; only the mapping, which lives in the Package Data Directory, goes away.
4. **Deletion is deliberate and recoverable if interrupted.** The Shell requires the Person to type the exact Workspace name before deleting. Deletion cannot be undone. If the application stops part way, the next start finishes the deletion rather than leaving a partly deleted Workspace or orphaned Package Data Directories.
5. **Desktop only.** Delete is a Shell control. Core and Host expose the operation as library calls; the headless application does not offer it yet.

## Implementation notes

- Migration 0020 adds `workspace_deletions`. Each append-only `*_no_delete` trigger accepts a delete only while its own Workspace has a marker row. Core's erase writes the marker, deletes every row children-first, refuses to commit if any table with a `workspace_id` column still holds the Workspace, and removes the marker in the same transaction, so no marker outlives it.
- The Package Host owns the cross-store order. Under its tier and reconciliation locks it checks Core's rule, records a `pending_workspace_deletions` entry, disposes and removes every Instance of the Workspace, asks Core to erase it (rechecking the rule), then removes the Package Data Directories named by its mappings, without following links, and the Workspace's Host rows. A refusal or failed disposal drops the entry and recreates the Instances. `PackageHost::open` settles any entry before the first activation: it is dropped when Core still lists the Workspace and completed when Core does not, so both desktop and headless finish an interrupted deletion.

## Rejected alternatives

- **Delete with other Members present:** would remove a shared Workspace from under other Members, or need the Phase 5 leave and revocation design.
- **Local "forget" of a Workspace by any Member:** that is Workspace leave, which stays in Phase 5.
- **Confirmation by a simple OK dialog:** too easy to trigger by mistake for an irreversible erase.

## Consequences

- `TEAM.SYS.SHELL-R04` gains the delete control.
- `TEAM-DQ9` is answered for deletion only: the Host disposes the Instances and removes their Package Data Directories. Leave and revocation remain open in Phase 5.
- A previously revoked Installation keeps whatever copy it already had; deletion cannot reach it.
- Implementation must order Host disposal and Core erase so that a crash at any point leaves either the whole Workspace or none of it, and must prove the sole-Member check against a concurrent admission.
