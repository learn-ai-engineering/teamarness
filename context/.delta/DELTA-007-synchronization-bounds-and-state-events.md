# DELTA-007 — Synchronization bounds, backpressure, and state events are incomplete

**Status:** open (2026-09-18)

## Divergence

`TEAM.SYS.TRANS-R03` and `TEAM.SYS.PKG-R10` describe bounded sessions, backpressure, and a subscription stream. In fact:

- no global memory bound or backpressure exists on the sync protocols;
- the bounded scheduler components are test-only;
- blocking SQLite and mutex work runs on async worker threads;
- the capability-call timeout cannot fire;
- `state` and `warning` subscription events are never emitted, so Packages infer availability.

## Impact

Memory can grow without limit under load or from unauthenticated peers, and timeouts do not bound Package calls.

## Resolution plan

The architecture-review workstream (see `roadmap.md` Current work), remediation plan phase 4 — synchronization resource bounds and async correctness.

## Tracking

- Review findings SYN-07, SEC-03, SYN-11, CQ-01/SYN-10/SEC-09, CQ-03, CQ-04, SYN-08
- `TEAM.SYS.TRANS-R03`, `TEAM.SYS.PKG-R10`, `TEAM.SYS.CONV-R14`
