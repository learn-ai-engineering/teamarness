# DELTA-022 — Unresolved Host deadline test failure

**Status:** open (2026-10-31). Don accepted deferring further diagnosis to the backlog on 2026-10-31. Owner: Host/Core test-reliability maintenance. This disposition stops the investigation; it does not establish a fix or a passing check.

## Observed failure

During auto-update qualification on `059de5d`, `capability_call_deadline_fires_while_core_is_busy` failed at `crates/teamarness-package-host/tests/capabilities.rs:1522`. After an intentional first-call deadline, the retry's `durability` was absent rather than `committed`. The original assertion did not capture the complete retry response, so its actual error and cause remain unknown.

The test file was unchanged from local main. No causal link to the updater, compiler, disk pressure, or concurrent workloads is established. No production failure is demonstrated.

## Subsequent evidence

An initial package-default focused run passed but did not match the root feature graph. Two later captures used the exact 17 dependency libraries retained from the failed root build, with all original assertions and 200ms capability deadlines preserved:

- An isolated diagnostic copy passed. Its retry committed in 5.537ms.
- The same copy ran with all 29 Host capabilities tests and default concurrency. All 29 passed; its retry committed in 5.861ms.

Neither capture reproduced the original failure. Passing captures do not prove the failure harmless or clear the historical failed full check. No retry-until-green, deadline extension, ignored test, or durable correction was applied.

## Follow-up

If the failure recurs, capture the retry's safe response classification and execution/delivery phase before changing behavior. Preserve the deadline, uncertainty semantics, and one-record idempotency assertion. A diagnostic-only assertion improvement can be planned separately; it is not a fix.

The successful captures used an external test copy. Those timing statements are not shipped production logging. The Host returns structured error codes, but complete persistent phase/timing diagnostics for this failure have not been demonstrated. Do not assume a production recurrence can be diagnosed from the existing logs alone. Broader local observability remains a separate scope.

## Qualification disposition

Don accepts carrying this unresolved reliability finding while planning updater manual verification. Do not make further investigation of this finding an automatic prerequisite to that planning. Keep final-source automated results and manual gates honest: no test suppression, passing Verify, merge, push, publication, or installation is authorized by this note.

## Tracking

- `context/roadmap.md`, stable application releases and updates.
- MyFlow workstream `auto-update`: `verify/20261031_host-deadline-blocker.md`, `research/20261031T105055Z_host-root-equivalent.md`, and `research/20261031T105800Z_host-suite-result.md`.
- The workstream's raw captures retain commands, exact dependency fingerprints, hashes, response fields, and timings without identity or record contents.
