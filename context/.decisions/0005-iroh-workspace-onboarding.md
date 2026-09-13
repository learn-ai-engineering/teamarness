# 0005: Iroh workspace onboarding with recoverable atomic import

**Status:** accepted (2026-09-13)

## Context

Phase 1 can issue an invitation, create an invitee claim, and redeem that claim on the issuing Installation. It cannot send public connection information with the invitation, transfer a Workspace trust anchor and canonical membership history, or create the Workspace in the invitee's database.

Onboarding crosses two independent SQLite databases. The issuer may commit an admission and spend the invitation before the joiner receives or stores the bootstrap. The protocol therefore needs two explicit commit points and a retry rule. It cannot use one distributed transaction.

Iroh 1.2 supplies authenticated Endpoint identities, default relays, ALPN routing, bidirectional QUIC streams, copyable endpoint tickets, and awaited Router shutdown. Iroh authenticates transport peers but does not authorize Workspace membership.

## Decision

### The Core Runtime owns onboarding

The Core Runtime binds one Iroh Endpoint from the persisted installation `SecretKey` during asynchronous runtime opening. It retains the onboarding Router and closes it through an awaited, idempotent shutdown operation.

The public interface exposes semantic operations for creating an opaque `WorkspaceInvitation` and joining a Workspace. It returns owned Workspace and membership values plus structured errors. Endpoint, Router, connection, stream, ticket, canonical-envelope, SQLite, and secret-key types remain private.

Low-level invitation issue, claim, and redemption mechanics become crate-private. Callers do not coordinate transport, admission, and import themselves.

### The copied value wraps the existing invitation

`InvitationToken` version 1 remains unchanged. A Teamarness-owned, bounded, versioned `WorkspaceInvitation` wraps the signed token and an Iroh `EndpointTicket`.

The outer parser enforces text and decoded-byte limits, accepts only the Phase 2 IP and relay address forms, limits address count, and requires the ticket Endpoint ID to equal the signed token inviter before dialing. The production path waits up to five seconds for default-relay readiness before it issues a fixed 15-minute invitation.

The copied value contains no private key material. It is still bearer admission material until one Installation claims it.

### The protocol binds both authenticated peers

Workspace onboarding uses ALPN `teamarness/workspace-join/1`, a fully handshaken connection, no 0-RTT, and one bounded request and response over one bidirectional stream.

The joiner requires the authenticated remote Endpoint ID to equal the token inviter. The issuer requires the authenticated remote Endpoint ID to equal the invitee embedded in the signed claim before it reads or changes invitation state.

The success response carries the Workspace trust anchor, canonical genesis, every currently accepted canonical post-genesis envelope in durable transition order, the recovered or new admission operation ID, and the final `MembershipVersion`.

The authenticated Iroh connection protects the Workspace name during import. The name becomes immutable local metadata after import, but Phase 2 does not store a durable signature for it. Canonical membership remains the sole membership authority.

Phase 2 limits a bootstrap to 8 MiB and 1,024 canonical operations. Every Phase 2 admission and revocation must preflight the resulting current bootstrap before commit so a current admitted Installation can finish a later retry.

### Admission and local join are separate commit points

The issuer transaction is the admission commit point. It commits the admission, dependencies, identity subject, derived membership, transition evidence, and spent invitation together. Once this transaction commits, the invitation is spent even if response delivery fails.

The joiner transaction is the local join commit point. Before writing, the joiner authenticates every envelope, verifies all peer, invitation, Workspace, genesis, and admission bindings, replays every transition prefix, and derives transition evidence and the final `MembershipVersion` itself.

An absent Workspace imports in one transaction. An existing Workspace must have the same immutable trust anchor and name. A matching local prefix may append the response suffix, while a response that is already a prefix of newer local state is an idempotent no-op. Changed canonical bytes, divergent transition order, or incomparable histories fail without replacement.

The shell reports the Workspace as joined only after a certain local commit or successful reopen confirms compatible durable state. An uncertain commit poisons the active runtime until reopen.

### Retry recovers one admission and completes against current state

The issuer checks for a spent nonce before applying expiry. An exact retry must match the stored token, nonce, Workspace, referenced admission, canonical claim, invitee, and authenticated peer. It recovers the same admission after expiry without creating another operation. A different claimant receives `AlreadyUsed`; contradictory durable rows are corruption.

After fresh admission or exact recovery, the issuer exports its complete current accepted operation set and transition order under one runtime consistency boundary. The joiner therefore receives the same member set and `MembershipVersion` held by the issuer at that boundary. If the recovered invitee is no longer a current member, the issuer returns `AdmissionNoLongerCurrent` and sends no bootstrap.

The issuer does not store a duplicate response blob. Exact recovery uses the spent record and the canonical claim embedded in the referenced immutable admission. Append-only triggers protect spent invitation bindings.

This is a bounded one-shot completion step, not continuous synchronization. It has no polling, background updates, arbitrary version negotiation, or package data transfer. A later chunked synchronization protocol can replace the temporary whole-bootstrap limit without changing invitation identity or the two commit points.

### The shell selects storage before opening the runtime

An ordinary Tauri launch uses the bundle-specific application data directory. Development and packaged two-instance checks may pass `--data-dir <absolute-path>` to the backend. Relative paths are rejected. JavaScript cannot select or inspect arbitrary storage paths.

The Tauri bridge owns serializable DTOs and stable error codes. It never receives private keys, raw secret bytes, SQLite handles, canonical envelopes, or raw Iroh types.

### Shutdown gates transport work

Shutdown rejects new invitation creation and join operations, stops accepting onboarding streams, and awaits Router and Endpoint closure. Existing local Core Runtime commands and queries are outside this transport lifecycle gate. Tauri stops dispatching all commands during application exit.

## Rejected alternatives

### Store every successful response

Rejected. It would duplicate up to 8 MiB of immutable canonical data for each admission and could become stale before retry.

### Return only the admission-time transition prefix

Rejected. A later admission or revocation could let the joiner report success with a member set that no longer matches the issuer. A retry after the invitee's revocation must not restore a stale local join.

### Import operations one at a time

Rejected. The joiner does not yet have a Workspace trust anchor, and startup requires complete transition evidence. Partial import could expose a false local join.

### Put Iroh in the shell

Rejected. The shell would need transport and installation-key knowledge and would expose a shallow cross-layer interface.

### Add addressing to InvitationToken version 1

Rejected. The token already signs admission authority. Public addressing changes independently and belongs in the outer transfer value.

### Sign the Workspace name in canonical membership

Rejected for Phase 2. The authenticated inviter connection is sufficient for import-time name authentication. Durable signed Workspace metadata needs its own later model.

## Consequences

- `teamarness-core` adds exact `iroh = "=1.2.0"` and `iroh-tickets = "=1.0.0"` dependencies plus an exact compatible Tokio dependency selected during implementation.
- Runtime opening becomes asynchronous. Existing tests, examples, and documentation must change.
- The issuer and joiner use separate durable transactions with network delivery between them. Recovery, not rollback, closes the gap.
- No runtime mutex guard may cross a network await. SQLite work runs away from the Tauri UI thread.
- Invitation retry remains available after expiry while its admission and spent record remain durable and the invitee remains a current member.
- Every Phase 2 membership mutation must keep the complete bootstrap within 8 MiB and 1,024 operations. Later chunked synchronization can remove this temporary constraint.
- A future synchronization protocol can start from the imported canonical operation set and `MembershipVersion` without changing this onboarding contract.
- Two app processes require separate private data directories to represent separate Installations.

## Evidence

- `.myflow/workstreams/shell-package-lifecycle/design/20260913T002055Z_workspace-onboarding-protocol.md`
- `.myflow/workstreams/shell-package-lifecycle/research/20260912T231307Z_iroh-workspace-onboarding-protocol.md`
- Final design review `1656ccd6-33f6-4e67-aaa6-396eff4e057b` and passing follow-up `bdb22a71-d724-43ef-bcb6-81b257f1ad7d`
- `context/02-system/02-membership/requirements.md`
- `context/02-system/03-transport/requirements.md`
- `context/02-system/07-shell/requirements.md`
- `crates/teamarness-core/src/invitation.rs`
- `crates/teamarness-core/src/membership/projection.rs`
- `crates/teamarness-core/src/runtime.rs`
- `crates/teamarness-core/src/workspace.rs`
- `crates/teamarness-core/migrations/0002_workspaces_membership.sql`
- `crates/teamarness-core/migrations/0003_invitations.sql`
- `crates/teamarness-core/migrations/0004_transitions.sql`
