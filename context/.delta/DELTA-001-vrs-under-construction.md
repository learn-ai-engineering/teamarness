# DELTA-001 — VRS intent tree under construction

**Status:** open (2026-09-12)

## Divergence

The VRS intent tree is being established as Phase 0 of the project. Subsystem requirements and specifications for several `02-system/` subdirectories are placeholder-only. The branch table in `context/spec.md` declares the intended structure, but some leaf nodes contain only summary descriptions rather than authored requirements.

## Impact

- Subsystem-level requirements for conversations, packages, repos, and observability are not yet authored.
- Cross-references between root requirements (`TEAM-R01`–`TEAM-R13`) and subsystem requirements use speculative prefixes that may change during detailed subsystem scoping.

## Resolution plan

1. During Phase 0, complete authored requirements for identity, membership, and transport subsystems. ✅ Done (2026-09-12)
2. During Phase 1 (Core Runtime), author detailed requirements and specs for identity and membership before implementation begins.
3. During Phase 2 (Shell and Package Lifecycle), author package and shell subsystem requirements.
4. During Phase 3 (Conversations), author conversations subsystem requirements.
5. Close this delta when every leaf node declared in the branch table has at minimum a `requirements.md` with at least one authored requirement.

## Tracking

- Subsystem count: 10 declared in branch table, 4 with authored requirements (identity, membership, transport, shell), 4 with placeholder summaries, 2 with minimal requirements
- Root requirements: 13 authored (`TEAM-R01`–`TEAM-R13`)
- Root decisions: 3 authored (0001–0003)
- Root deltas: 1 (this file)
- Open questions: 6 (`TEAM-DQ1`–`TEAM-DQ6`)