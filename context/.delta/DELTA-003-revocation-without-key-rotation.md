# DELTA-003 — Revocation does not rotate content keys in version 1, and some requirements say it does

**Status:** open (2026-09-18)

## Divergence

`TEAM.SYS.MEM-R05`, `TEAM.SYS.MEM-R06`, and the ontology's Revocation row say Package keys rotate on revocation and that future content is protected. ADR 0008 and ADR 0009 freeze one Content Key per scope with no rotation. Revocation only closes future append and synchronization gates, and DELTA-002 weakens those gates too.

## Impact

A revoked Member who obtains a later sealed record can read it.

## Resolution plan

The architecture-review workstream (see `roadmap.md` Current work), remediation plan phase 3 — membership replication and synchronization authorization, rewords `TEAM.SYS.MEM-R05`/`R06` and the ontology to state version 1 gating truthfully. Content Key rotation per membership version is delivered in roadmap Phase 5 with revoke controls. Close this delta when rotation ships.

## Tracking

- Review finding CTX-01
- `TEAM.SYS.MEM-R05`, `TEAM.SYS.MEM-R06`, `TEAM.SYS.CONV-R11`, roadmap Phase 5
