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

1. Fix the Host cancel-before-write race: retire the request only after the writer has observed the request frame, or drop a cancelled frame that is still queued. Add a Host test that cancels a queued request.
2. Decide whether a closed UI event receiver should fault the whole mount, or only end that subscription.
3. Reproduce the post-join `markdown` failure with the request error captured.
4. Check whether the negative `Nested.md` assertion exposes a placement-ordering bug in legacy flat mappings.
5. Re-enable the test in the required checks once it passes repeatedly on a cold macOS build.

## Tracking

- Teamarness Backlog: *Documents › DELTA-024: flaky Documents folder sync qualification (test ignored)*
