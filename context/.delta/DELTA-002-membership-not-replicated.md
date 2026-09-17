# DELTA-002 — Canonical Membership does not replicate after onboarding, and sync does not gate the remote peer

**Status:** open (2026-09-18)

## Divergence

`TEAM-R04`, `TEAM.SYS.MEM-R01`, `TEAM.SYS.MEM-R06`, and `TEAM.SYS.CONV-R03` assume every Member eventually knows every admission and revocation, and that only Members receive records. Membership reaches another Installation only through the one-shot onboarding bootstrap (ADR 0005), and the synchronization acceptor never checks that the connecting peer is a current Member.

## Impact

In a Workspace with three or more Members, an earlier Member never learns about a later admission, so those two Members silently never exchange Conversations. A revocation stays local to the Installation that authored it. A peer that is not a Member but knows a Workspace and Package identity can request records.

## Resolution plan

The architecture-review workstream (see `roadmap.md` Current work), remediation plan phase 3 — membership replication and synchronization authorization, with a three-Installation convergence test.

## Tracking

- Review findings ARC-01, SEC-01/SYN-01, SYN-03, SYN-05
- `TEAM.SYS.MEM-R01`, `TEAM.SYS.MEM-R06`, `TEAM.SYS.CONV-R03`
