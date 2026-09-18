# DELTA-003 — Revocation does not rotate content keys in version 1, and some requirements say it does

**Status:** open (2026-09-18)

## Divergence

Version 1 revocation is a synchronization gate only. The revoked Installation keeps each Content Key it already received, so records it obtains outside the gate remain readable. The vision and roadmap Phase 5 intend Content Key rotation on revocation. Requirements now state the gate (`TEAM.SYS.MEM-R05`, `TEAM.SYS.MEM-R06`, `TEAM.SYS.TRANS-R06`).

## Impact

A revoked Member who obtains a later sealed record, from a Member that has not yet accepted the revocation or from a colluding Member, can read it.

## Resolution plan

Roadmap Phase 5 Content Key Generations: a revocation starts a new generation, so records sealed afterwards do not open with a retained key. Close this delta when rotation ships.

## Tracking

- Review finding CTX-01; ADR 0013
- `TEAM.SYS.MEM-R05`, `TEAM.SYS.MEM-R06`, `TEAM.SYS.CONV-R11`, roadmap Phase 5
