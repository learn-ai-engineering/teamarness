# DELTA-001 — VRS intent tree under construction

**Status:** open (reviewed 2026-09-14)

## Divergence

The VRS intent tree is being established as Phase 0 of the project. Subsystem requirements and specifications for several `02-system/` subdirectories are placeholder-only. The branch table in `context/spec.md` declares the intended structure, but some leaf nodes contain only summary descriptions rather than authored requirements.

## Impact

- Requirements for conversations, repositories, observability, and delivery are not yet authored.
- These placeholder nodes do not yet provide detailed traceability from the root requirements to subsystem behavior.

## Resolution plan

1. Author identity, membership, and transport requirements. Complete (2026-09-12).
2. Author Shell requirements for Desktop Shell and Workspace onboarding. Complete (2026-09-13).
3. Author Package requirements for Package Foundation. Complete (2026-09-14).
4. During Phase 4, author Conversations requirements before implementation begins.
5. Author repository, observability, and delivery requirements during their owning workstreams.
6. Close this delta when every leaf node declared in the branch table has at least one authored requirement.

## Tracking

- Leaf-node count: 11 declared in the branch table, 7 with authored requirements and 4 with placeholder summaries
- Root requirements: 13 authored (`TEAM-R01`–`TEAM-R13`)
- Root decisions: 7 authored (`0001`–`0007`)
- Root deltas: 1 (this file)
- Open questions: 8 recorded (`TEAM-DQ1`–`TEAM-DQ8`), with 1 unresolved