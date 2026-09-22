# DELTA-008 — Conversations delivery does not meet its qualification targets

**Status:** open (2026-09-18)

## Divergence

`TEAM.SYS.CONV-R16` requires a new Message within 5 seconds for a reachable peer, catch-up on 1,000 queued records within 30 seconds, and those targets in a Workspace of 25 Members. The shipped qualification (`tests/system/tests/conversations_two_profile_flow.rs`) proves 10 seconds, 100 queued records, and two or three Members; nothing measures 25 Members.

## Impact

The delivery targets are stated intent, not measured behaviour. A Workspace larger than three Members has no performance evidence at all.

## Resolution plan

A performance qualification under `roadmap.md` Later (unscheduled) raises the test targets to the requirement and adds a 25-Member measurement. Close this delta when the qualification meets `TEAM.SYS.CONV-R16`.

## Tracking

- `TEAM.SYS.CONV-R16`
- The architecture-review workstream (see `roadmap.md` Current work), remediation plan phase 2 — context integrity, authored the requirement text
