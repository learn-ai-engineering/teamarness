# 0013 — Membership replication and synchronization authorization

**Status:** accepted (2026-09-18)

## Context

Canonical Membership reached another Installation only through the one-shot onboarding bootstrap (ADR 0005). After onboarding, a later admission or revocation stayed on the manager's Installation. With three or more Members, Installations that had not heard of a new Member rejected its records and aborted whole synchronization sessions, so peer pairs stopped converging.

The package synchronization acceptor served every sealed operation and the Member roster to any authenticated Iroh peer that named a Workspace and Package. A second record ingest path skipped the author rule and stored unauthenticated blobs without a bound. Any Member could bootstrap a scope's content key on a newcomer, contrary to ADR 0008's creator-only first key. Opened records were not bound to the Package and Workspace their author signed.

Protection controls wrapped their content key under a key derived from public inputs, so anyone holding the bytes could unwrap the key, and they were signed with a raw signature rather than through the domain-separated signing interface. This phase puts controls into a session handshake and stores the creator's `create` control on every importer, which then re-shares it, so recipient-bound sealing has to be in place before the first commit that sends a control to a peer.

Version 1 has one content key per Package scope and no rotation (ADR 0008, ADR 0009), so the synchronization gate is the only protection for content written after a revocation.

## Decision

### Membership replicates over its own Core protocol

The Core Runtime replicates accepted canonical membership operations over ALPN `teamarness/membership-sync/1`, independent of Package scopes. One finite session exchanges sorted operation-ID sets, then the operations each side lacks, in causal order. Received operations are applied through the same validation, p2panda-auth replay, bootstrap-limit preflight, and transition path as locally authored ones. Replication is idempotent and bounded to 1,024 operations and 8 MiB per direction.

Every accepted membership operation bumps a durable per-Workspace wake in the same transaction. The synchronization worker runs membership sessions before record sessions: on wakes, on startup, every 30 seconds while idle, and immediately after a Package session is refused because the peer is not known as a Member.

A responder accepts pushed operations from any authenticated peer, because operations are self-authenticating. It then discloses according to the pusher's standing in its updated view: a current Member receives every operation it lacks; a former Member receives only the causal closure of the revocations that target it; anyone else receives nothing. The responder never creates a Workspace from replication.

### Package synchronization is gated on current membership

Package synchronization uses ALPN `teamarness/package-sync/2`. The responder reads one `Hello` of at most 1 KiB and serves nothing, including its author list, unless both itself and the authenticated remote Installation are current Members of the named Workspace. A refused peer receives an explicit refusal. The Installation `PublicIdentity` is the Iroh `EndpointId` authenticated by the QUIC handshake. Accepting a revocation cancels live sessions with the revoked Installation, and the worker re-checks membership immediately before dialling.

ALPN `teamarness/package-sync-batch/1` and record carriage outside LogSync are removed. Protection grants travel inside the package-sync handshake, in either direction.

### One bad operation never aborts a session

Each received operation is applied, recognised as a duplicate, parked, or rejected. Only storage failure or a revocation ends a session. After a rejection, later operations from the same author are skipped for that session and redelivered by a later session.

An authenticated operation whose author has no known membership history, or whose scope has no content key yet, is parked. Parking is bounded per scope (4,096 operations, 64 MiB) and per sending peer within a scope (1,024 operations, 16 MiB), and parked operations expire after seven days. Parked operations are applied when the author's admission or the scope's key is accepted.

### The creator's origin authorizes the first content key

A protection grant carries the Workspace creator's signed `create` control for the scope. An Installation accepts a scope's first content key only when that origin is authored by the Workspace creator and names the same scope and content-key identifier as the grant. Any current Member holding the key may grant it. A `create` control is never accepted as a grant.

### Protection controls are sealed to their recipient

A protection control is a fixed 354-byte value. Its content key is encrypted with HPKE (RFC 9180 Base mode, DHKEM-X25519, HKDF-SHA256, ChaCha20-Poly1305) to an X25519 key derived from the recipient Installation's Ed25519 identity key, so only that Installation's private key opens it. The whole control header — version, kind, scope, Workspace, author, recipient, admission, and content-key identifier — is the associated data of that encryption, and the author signs the header and ciphertext through the domain-separated signing interface with the context `teamarness.protection-control/v2`. Version-1 control bytes are refused on the wire and at rest; there is no dual-read path, and a data directory holding them is refused at startup with a message that says to delete it.

### Records are bound to their signed scope

Opening a sealed record fails unless the record's signed Package Identity and Workspace equal the scope that sealed it.

### Transitions name their change

A Core membership transition reports whether it admitted or revoked, the subject Installation, and the author. The `teamarness.membership` 1.0 Package payload is unchanged.

## Rejected alternatives

### Carry membership in Package synchronization

Rejected. Membership would move only while some Package scope has work, and every Package session would carry or negotiate a membership proof.

### Gossip membership operations

Rejected. It adds a dependency and topic management and still needs anti-entropy for offline Members.

### Gate at connection accept

Rejected as the only gate. The Workspace is not known until the first frame, and membership is per Workspace.

### Abort, or skip without parking, on an unknown author

Aborting wedges peer pairs. Skipping alone is correct but leaves forwarded records unavailable while the forwarding peer is offline.

### Keep the batch ingest path with an author gate

Rejected. It invents author sequences and conflicts with the authenticated LogSync projection.

### Creator-authored grants only; or any Member may create a key

Creator-only grants make a Package unusable for newcomers while the creator is offline. Any-Member creation needs a convergence rule that diverges under incomplete views, or re-sealing that needs rotation.

### Extend the `teamarness.membership` capability to 1.1 now

Rejected for now. Host negotiation supports only one minor version until ADR 0016 adds minor ranges, and roadmap Phase 5 changes the same event again for rotation notifications. Minor-range negotiation is plan phase 6 work; the 1.1 payload stays with roadmap Phase 5.

## Consequences

- Membership synchronization is no longer deferred. Distributed invitation spending across managers remains deferred (ADR 0004); version 1 still has one manager.
- ADR 0005 onboarding is unchanged; replication starts from the imported operation set as ADR 0005 anticipated.
- ADR 0008's "current-holder grants" now require the creator's origin. ADR 0009's forwarding rule still holds for authors with membership history; authors without it are parked instead of rejected, and ADR 0009's isolation of invalid operations now holds at session level.
- Version 1 revocation remains a gate, not cryptography. A revoked Installation keeps every content key it received. A Member that has not yet learned a revocation, or a colluding Member, can still give it new records it can read. Content-key rotation per membership change is roadmap Phase 5 work.
- Protocol changes are a clean break: Installations built before this decision cannot synchronize with those built after it, and Workspaces created before it are unsupported.
- A relayed `create` control and every grant are now opaque to anyone but their recipient, so carrying them in the handshake and re-sharing them adds no exposure.
- Plaintext key storage at rest is unchanged: the Installation secret and content keys sit unencrypted in the private data directory under the ADR 0007 trust model. ADR 0017 records that as an accepted risk with its revisit triggers.
- The single-Manager model is a version-1 scope limit, not the intended model. The canonical operation format admits additive `promote`, `demote`, `transfer`, and `leave` actions with no envelope version change; adding one needs a new `teamarness/membership-sync` ALPN version, because an Installation that cannot decode an action rejects the operation. Roadmap Phase 5 introduces a small group of Workspace administrators.

## Evidence

- `crates/teamarness-core/src/membership_sync/`: membership replication protocol, wire codec, handler, and session.
- `crates/teamarness-core/src/membership/replication.rs`: causal export, want computation, disclosure policy.
- `crates/teamarness-core/src/synchronization/network_handler.rs`: package-sync/2 handshake, gate, outcome classification.
- `crates/teamarness-core/src/synchronization/parking.rs`: parking bounds and promotion.
- `crates/teamarness-core/src/synchronization/protection_control.rs`: recipient-bound sealing, domain-separated signatures, and origin verification.
- `crates/teamarness-core/migrations/0010_membership_replication.sql`.
- `crates/teamarness-core/src/runtime_tests/`: in-process tests with three or more Installations covering late admission, offline catch-up, manager offline, non-member and revoked-member refusal on every synchronization ALPN, forged scope, and parked records.
- `tests/system/tests/conversations_three_profile_flow.rs`: three Conversations profiles over production relays (production-relay qualification, ignored by default, run by `npm run verify:conversations`).
- **Weaknesses:** in-process tests run on loopback with a static address book, not relays or discovery. Replaying membership applies the whole DAG per operation, so very large catch-ups are slow; realistic Workspaces are far below the 1,024-operation limit. No adversarial fuzzing of the new wire formats was done.
