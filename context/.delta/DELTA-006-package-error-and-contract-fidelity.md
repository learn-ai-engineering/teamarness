# DELTA-006 — Package error codes and some contract promises do not reach users

**Status:** open (2026-09-18)

## Divergence

`TEAM.SYS.OBS-R01` and `TEAM.SYS.SHELL-R06` promise stable, observable errors. The Package Host replaces every Package-owned error code, `retryable` flag, and detail with a generic `backend_error`, and `subscribe` failures lose their code. Related gaps:

- Repo-aware accepts an existing relative path instead of rejecting it, because `canonicalize` resolves it against the Backend's working directory, so `repo_aware.path_not_absolute` is unreachable (`TEAM.SYS.REPO-R02`).
- Repo-aware cannot cancel a running snapshot: the Backend creates a fresh cancel flag that is never set (`TEAM.SYS.REPO-R09`).
- The Conversations UI offers Hide to Members who did not create the Channel, and their attempt then fails (`TEAM.SYS.CONV-R06`).
- The Conversations contract declares availability values that are never emitted, and omits the unread count the projection already computes (`TEAM.SYS.CONV-R13`, `TEAM.SYS.CONV-R14`).
- The Conversations Backend decoder accepts `--` inside a remote Channel slug, which the UI contract pattern forbids (`TEAM.SYS.CONV-R05`).

## Impact

Users and Package UIs cannot tell a retryable failure from a permanent one, or show accurate messages.

## Resolution plan

The architecture-review workstream (see `roadmap.md` Current work), remediation plan phase 6 — Package contract fidelity and authoring experience, after remediation plan phase 5.

## Tracking

- Review findings AUT-02, AUT-14, AUT-04/CQ-02, SYN-14
- `TEAM.SYS.OBS-R01`, `TEAM.SYS.SHELL-R06`, `TEAM.SYS.REPO-R02`, `TEAM.SYS.REPO-R09`, `TEAM.SYS.CONV-R05`, `TEAM.SYS.CONV-R06`, `TEAM.SYS.CONV-R13`, `TEAM.SYS.CONV-R14`
