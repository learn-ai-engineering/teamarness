# DELTA-006 — Package error codes and some contract promises do not reach users

**Status:** closed (2026-09-19)

Resolved by ADR 0016 (architecture-review workstream, remediation plan phase 6 — Package contract fidelity and authoring experience).

## Divergence

`TEAM.SYS.OBS-R01` and `TEAM.SYS.SHELL-R06` promise stable, observable errors. The Package Host replaced every Package-owned error code, `retryable` flag, and detail with a generic `backend_error`, and `subscribe` failures lost their code. Related gaps:

- Repo-aware accepted an existing relative path instead of rejecting it, because `canonicalize` resolved it against the Backend's working directory, so `repo_aware.path_not_absolute` was unreachable (`TEAM.SYS.REPO-R02`).
- Repo-aware could not cancel a running snapshot: the Backend created a fresh cancel flag that was never set (`TEAM.SYS.REPO-R09`).
- The Conversations UI offered Hide to Members who did not create the Channel, and their attempt then failed (`TEAM.SYS.CONV-R06`).
- The Conversations contract declared availability values that were never emitted, and omitted the unread count the projection already computed (`TEAM.SYS.CONV-R13`, `TEAM.SYS.CONV-R14`).
- The Conversations Backend decoder accepted `--` inside a remote Channel slug, which the UI contract pattern forbids (`TEAM.SYS.CONV-R05`).

## Impact

Users and Package UIs could not tell a retryable failure from a permanent one, or show accurate messages.

## Resolution

A Package Backend's code, retryability and bounded details cross the Host, the desktop bridge and the Package UI Frame as a nested `packageError`, and reach the subscriber as a typed `PackageRequestError`. A stream the Package completes or closes with its own code is delivered to its subscriber instead of faulting the mount.

Each tracked item, with the workstream step that closed it:

| Item | Closed by |
|---|---|
| Package-owned code, retryability and details (AUT-02) | Steps 3 and 4 |
| Subscription failures keep their code (AUT-14) | Steps 3 and 10 |
| `TEAM.SYS.REPO-R02`, relative paths refused before any filesystem call | Step 10 |
| `TEAM.SYS.REPO-R09`, a snapshot stops when its request is cancelled | Step 10 |
| `TEAM.SYS.CONV-R05`, the decoder's slug grammar equals the contract's | Step 18 |
| `TEAM.SYS.CONV-R06`, Hide is offered only to a Channel's creator | Step 12 |
| `TEAM.SYS.CONV-R13`, `unreadCount` in the contract, the projection and the generated types | Steps 12 to 14 |
| `TEAM.SYS.CONV-R14`, availability values that are emitted | Closed earlier by remediation plan phase 4: the behaviour in its Step 13, the requirement text and DELTA-007 in its Step 15 |

The evidence is in ADR 0016. `TEAM.SYS.CONV-R08` message order was a separate divergence, DELTA-010, closed by Step 14.

## Tracking

- Review findings AUT-02, AUT-14, AUT-04/CQ-02, SYN-14
- `TEAM.SYS.OBS-R01`, `TEAM.SYS.SHELL-R06`, `TEAM.SYS.REPO-R02`, `TEAM.SYS.REPO-R09`, `TEAM.SYS.CONV-R05`, `TEAM.SYS.CONV-R06`, `TEAM.SYS.CONV-R13`, `TEAM.SYS.CONV-R14`
