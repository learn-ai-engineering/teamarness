# 0019 — Lifecycle verdicts and derived bounds

**Status:** accepted (2026-09-20)
**Refines:** ADR 0007 (the Package boundary), ADR 0016 (Package error fidelity), ADR 0018 (seam versioning)

## Context

The Package Process Protocol is a distributed state machine, and the two peers' views of it legitimately differ for one round trip. A stream close and the credit for events already accepted cross on the wire. A cancel and the answer it races cross on the wire. The contract already requires a receiver to survive both.

The code had no verdict for that. Every frame and every internal condition was sorted into "proceed" or "fatal", so a one-round-trip disagreement was read as the peer misbehaving. Because the tolerance lived in each adapter rather than in the shared state machine, the Host and the Package classified the same race differently, and one file classified a late event and a late acknowledgement for the same subscription two different ways forty lines apart.

The second half is the same mistake at a different altitude. Internal queues and pages were sized without reading the contract constant they serve: a control lane holding 16 frames for the 32 in-flight answers the contract permits, a record page that cannot fit the writer budget. An undersized bound overflows on legal traffic, and the overflow was then reported as the process having exited or the capability being unavailable.

## Decision

### 1. Three verdicts, one classifier

`ContractState.observe` returns `Verdict::{Deliver, Ignore}` or a `ProtocolError`. A frame is fatal only when no conforming peer that had seen everything this receiver sent could have sent it. A frame the contract permits, but whose state the receiver has retired, is ignored. A request that is legal and in budget but cannot be accepted now is answered with `overloaded`, which the contract already makes retryable.

`ContractState` owns that classification. The Host's reader, the Host's writer, the Package SDK's dispatcher and the Package SDK's writer all call the same function over the same frames. No adapter carries its own tolerance, so a new late-frame race cannot be classified two ways.

The verdict is about delivery, so only a reader acts on `Ignore`. A writer observes its own frames for the state they carry and for the one question the classifier answers, then writes them: withholding a frame would break the contract's own guarantee that exactly one `response` or `error` is terminal for a request, and the peer runs the same classifier over what arrives.

### 2. Bounded tombstones, entered when a peer stops serving

Three bounded FIFO sets carry the tolerance: `answered` (a second terminal frame is fatal), `abandoned` (a late first terminal is ignored and moves the ID to `answered`), and `closed_streams` (a late `event`, `credit` or `stream_close` is ignored). Each holds two contract windows of IDs per originating direction.

A peer enters the tombstone at the moment it stops being willing to serve the request or the stream, not when a later grace period expires. A frame naming an ID evicted from a bounded set is out of contract again, because the peer exceeded the window the contract grants.

### 3. Every internal bound is an expression over the constant it serves

A queue, buffer, page or channel that carries contract-governed traffic is declared as a named constant over the protocol constant it serves, with a comment naming the worst case it must admit. Exhausting it is backpressure or `overloaded`, never a fatal or permanent code.

Three mechanisms hold it: a floor test per crate asserting each bound meets its floor, a test that parses the limits table out of the contract README and asserts every constant matches it, and `scripts/check-protocol-bounds.mjs`, which refuses a bare numeric literal in a channel or semaphore constructor and reads the one subscription bound the Shell and the Host both publish.

### 4. The contract states the rule

`contracts/package-process/v1/README.md` states the three verdicts, the tombstone obligation for streams, the moment a receiver enters a tombstone, and the ordering rule that a `stream_close` stays behind the events of its own stream. The 1.0 text is amended in place.

Under ADR 0018 this adds no frame kind, no field and no vocabulary value, so the closed-vocabulary clause is not engaged and this is not a major. It is wider tolerance plus one new obligation on implementers, and ADR 0018 §4 records that there is no install base, no published SDK and no external Package.

## Rejected alternatives

- **Leaving the tolerance in the adapters and patching each defect.** That is how the Host and the SDK came to disagree about the same race, and it would have left seven new special cases behind.
- **Ignoring any frame for an unknown stream or request.** It deletes the `unknown_stream` and `unknown_response` verdicts, which are what separate a peer that is behind from a peer that is inventing. The tombstone is the mechanism that keeps the distinction decidable.
- **Tolerating a frame for an evicted tombstone.** Same objection, reached by a slower path.
- **Reducing the record page instead of fixing the backpressure.** The page is the credit window the contract grants. An ordinary slow reader must stop being reported as a permanent capability failure whatever the page size is.
- **A minor or major version bump for the amendment.** A minor buys nothing when the only two implementations ship together, and a major is wrong: nothing is removed, narrowed, or added to a closed vocabulary.

## Consequences

- `observe`'s signature changed, so the compiler enumerates every place this class of defect can live.
- `overloaded` becomes reachable where `ProcessExited` was. It is retryable by the contract's own table and by the SDK's.
- Malformed and out-of-contract traffic stays fatal. The three state-machine fixtures in `contracts/package-process/v1/invalid/` are unchanged and still fail, and a fourth now pins the line the tolerance stops at: a cancelled request tolerates one late terminal frame, never two.
- Nothing enforces the "comment naming the worst case" half of the bounds rule mechanically. The guard catches a bare literal; only review catches a derived bound whose derivation is wrong.

## Evidence

- `crates/teamarness-package-protocol/src/state.rs` — `Verdict`, `IgnoredFrames`, the three bounded tombstone sets, `retire_request`, `retire_stream`, and the unit tests for each verdict including `an_evicted_tombstone_is_fatal_again`.
- `crates/teamarness-package-protocol/tests/contract.rs` — `every_constant_matches_the_contract_limits_table`, and the golden and invalid corpora.
- `contracts/package-process/v1/fixtures/node-package-process.mjs` — the language-neutral reference implementation of the same rules.
- `crates/teamarness-package-host/tests/lifecycle_races.rs` — the Host's reader and writer under each race, and `every_in_flight_capability_call_is_answered_behind_a_slow_pipe`.
- `crates/teamarness-package-sdk/tests/lifecycle_races.rs` — the Package's side, with a `ContractState` standing in for the Host's reader.
- `crates/teamarness-package-host/tests/ui_event_stream.rs` — `acknowledging_the_last_event_of_a_completed_stream_keeps_the_generation`.
- `apps/teamarness-desktop/src/package-port-host.test.ts` — the Shell's bounded subscription tombstones, and that an id never issued is still a fault.
- `scripts/check-protocol-bounds.mjs` and its test — the bare-literal guard and the shared subscription bound.

### Provenance (non-versioned)

`.myflow/workstreams/architecture-review/design/20260919_lifecycle-race-and-bounds-policy.md` and its developer decisions D1–D5, over `.myflow/workstreams/architecture-review/verify/p1-verification.md`.
