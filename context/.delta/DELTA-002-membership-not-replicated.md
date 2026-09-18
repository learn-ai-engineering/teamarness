# DELTA-002 — Canonical Membership does not replicate after onboarding, and sync does not gate the remote peer

**Status:** closed (2026-09-18). Canonical Membership replicates, and package synchronization is gated on current membership.

## Divergence

`TEAM-R04`, `TEAM.SYS.MEM-R01`, `TEAM.SYS.MEM-R06`, and `TEAM.SYS.CONV-R03` assume every Member eventually knows every admission and revocation, and that only Members receive records. Membership reached another Installation only through the one-shot onboarding bootstrap (ADR 0005), and the synchronization acceptor did not check that the connecting peer was a current Member.

## Impact

In a Workspace with three or more Members, an earlier Member never learned about a later admission, so those two Members silently never exchanged Conversations. A revocation stayed local to the Installation that authored it. A peer that was not a Member but knew a Workspace and Package identity could request records.

## Resolution

Resolved by ADR 0013 (architecture-review remediation plan phase 3). Canonical membership replicates over `teamarness/membership-sync/1` (`crates/teamarness-core/src/membership_sync/`, `crates/teamarness-core/src/membership/replication.rs`), driven by durable per-Workspace wakes and the synchronization worker's membership pass. Package synchronization runs on `teamarness/package-sync/2` and serves nothing until both Installations are current Members. `crates/teamarness-core/src/runtime_tests/membership_replication.rs` and `crates/teamarness-core/src/runtime_tests/sync_authorization.rs` cover convergence, offline catch-up, revocation reach, revocation-closure disclosure, and refusal on every synchronization ALPN.

## Tracking

- Review findings ARC-01, SEC-01/SYN-01, SYN-03, SYN-05
- `TEAM.SYS.MEM-R01`, `TEAM.SYS.MEM-R06`, `TEAM.SYS.MEM-R10`, `TEAM.SYS.TRANS-R06`, `TEAM.SYS.CONV-R03`
