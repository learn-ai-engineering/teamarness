# DELTA-024 — Flaky Documents folder sync qualification

**Status:** open (2026-11-19). Don accepted ignoring the test to unblock the v0.1.6 release. Owner: Documents and Host test reliability. Ignoring the test removes it from the required checks; it does not fix the failure.

## Observed failure

The v0.1.6 Stable release run (`44447607009`) failed on both attempts at `npm run check` on `macos-15`. Each attempt failed in `recursive_mappings_preserve_identity_consent_and_nested_changes` in `tests/system/tests/documents_folder_sync.rs`, at a different point:

- Attempt 1: `alice.markdown(&plan)` returned `None` after Bob joined.
- Attempt 2: `documents.document.create` for "Risks" returned `StaleMount`.

PR and `main` checks run `npm run check:fast` on Ubuntu. They do not run this macOS-only test, so these were its first CI runs since it was rewritten in `07ae3db`.

The failure reproduces locally on macOS without CI load. About 1 in 4 to 1 in 7 runs fail, at several points: the two above, a `close` of a session subscription, a negative assertion that Bob does not flatten `Nested.md`, and poll deadlines such as "restore recreates missing parents on both Installations".

## Evidence from diagnosis

Temporary instrumentation (since reverted) established these facts:

- In one `StaleMount` failure, the Host's writer rejected its own outgoing `documents.document.create` request with `duplicate_id`. That failed the generation with `ProtocolViolation` and stopped the Documents backend.
- `ProcessClient::cancel` retires a request in the classifier before its request frame is written. A cancel that runs while the frame is still queued therefore makes the later write look like a duplicate. `InstanceActor::unmount` and `fail_mount` issue such cancels for every in-flight request on the mount.
- A UI event route ended with `EventLagged` in one run, and the Host then failed Alice's mount. `UiEventRoute` maps both a full channel and a closed receiver to `EventLagged`. The test's `session` helper dropped its event receiver while the subscription was still open. The test now keeps the receiver until `close`, but this change alone did not stop the failures: 5 of 20 runs still failed.
- The cause of the `markdown` → `None` failure after Bob joins is not established.

## Follow-up

1. ~~Fix the Host cancel-before-write race.~~ Done (2026-11-27). `ProcessClient::cancel` and the request-deadline path now withdraw a request the writer has not observed yet, instead of retiring it. The writer drops a withdrawn request frame, and no `cancel` is queued for it, so the Package never sees either. The check and the writer's observation run under the classifier lock, so they cannot interleave. A deadline on a withdrawn request is a certain `deadline_exceeded`, because nothing reached the wire. Host tests `a_request_cancelled_before_the_writer_reaches_it_is_withdrawn` and `a_request_that_times_out_before_the_writer_reaches_it_is_withdrawn` in `crates/teamarness-package-host/tests/lifecycle_races.rs` cancel and time out a queued request behind a blocked writer. The same race failed the v0.1.8 Stable release run (`45845889941`, attempt 2) in `real_sdk_consumer_lists_reads_revises_and_observes_documents` with `StaleMount`. The SDK retires a timed-out capability call the same way, but only after its 10-second deadline, so its writer would have to stay blocked behind the Host's reader for that long. It is not changed here.
   After the fix, on Linux, `documents_ports_proof` passed 20 of 20 runs. The ignored folder-sync test passed 5 of 10 with no `StaleMount`. All 5 failures were the poll "zero eligible files marks the mapping unavailable" after a mapped subtree is removed. Unmodified `main` fails the same poll (2 of 6 runs), so that failure predates the fix and is a separate Documents watcher problem.
2. Decide whether a closed UI event receiver should fault the whole mount, or only end that subscription.
3. Reproduce the post-join `markdown` failure with the request error captured.
4. Check whether the negative `Nested.md` assertion exposes a placement-ordering bug in legacy flat mappings.
5. Re-enable the test in the required checks once it passes repeatedly on a cold macOS build.

## Tracking

- Teamarness Backlog: *Documents › DELTA-024: flaky Documents folder sync qualification (test ignored)*
