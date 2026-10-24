# DELTA-011 — Documents metadata may not appear after an existing Member restarts

**Status:** open, unverified (2026-10-24). Deferred to future Documents Package work.

## Possible divergence

`TEAM-R10` and `TEAM.SYS.DOCS-R02` require offline-created records to synchronize when peers become reachable. In one two-instance observation, Bob was already a Workspace Member and his app was closed. Alice created a Collection and Document while Bob was away. Bob did not see either after relaunch; both appeared after Alice edited a Document. The wait before that edit and the state of Bob's record log, Documents projection, and UI subscription were not captured. This observation does not establish which component delayed delivery.

A temporary loopback Core test admitted Bob, stopped him, appended two opaque Documents metadata records on Alice, restarted Bob, and required both records without a later edit. It passed once. The test did not run the Documents Package or desktop UI and was removed after diagnosis. No full-app reproducer or cause is known.

## Impact

If the observation recurs, an existing Member may need a later edit before seeing Collections and Documents created while their app was closed. Offline restart catch-up is not verified for this case. Current online two-instance checks do not establish it.

## Follow-up

This is backlog work, not a current fix. Reproduce with Bob already joined: close his app, create a uniquely named Collection and Document on Alice, relaunch Bob, and wait without editing. Record the visible status and elapsed time. If both remain absent, compare Bob's redacted Core record presence, Documents snapshot, and UI subscription before another edit. Keep invitation values, keys, and document bodies out of captures. Close this delta only after a full-app regression and repeat manual observation establish automatic catch-up, or after evidence disproves the original symptom.

## Tracking

- `TEAM-R10`, `TEAM.SYS.DOCS-R02`, `TEAM.SYS.DOCS-R03`
- `context/roadmap.md`, Later (unscheduled)
- `workspace-documents` workstream: `verify/20261024T012531Z_offline-collection-catchup-observation.md` and `verify/20261024T033051Z_existing-member-catchup-diagnosis.md`
- No backend or UI change, commit, merge, or push is authorized by this delta.
