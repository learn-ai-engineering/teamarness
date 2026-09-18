# 02-system/02-membership — Membership Requirements

Prefix: `TEAM.SYS.MEM`

## TEAM.SYS.MEM-R01 — Canonical Membership Log

The system SHALL maintain a signed, causally ordered, append-only membership operation DAG. Each `MembershipOperation` SHALL have a content-derived `MembershipOperationId` and SHALL link to every causal head known to its author. One immutable genesis operation SHALL anchor each workspace. The resolved DAG SHALL be the sole authority for workspace membership.

## TEAM.SYS.MEM-R02 — Membership Operations

The system SHALL support two post-genesis membership operations: `admit` and `revoke`. Each operation SHALL be signed by an existing workspace member with appropriate capability. Operation payloads SHALL include the target `PublicIdentity`, an author-declared timestamp, and content-hash links to their causal dependencies. The runtime SHALL verify the canonical content hash, signature, workspace genesis descent, and dependency integrity before authorization processing.

## TEAM.SYS.MEM-R03 — Single-Use Invitations

The system SHALL generate cryptographically signed, single-use Invitation tokens. A token SHALL carry the inviter's identity, the Workspace identity, an expiry timestamp, and a signature from the inviter. The system SHALL reject expired unused tokens and tokens spent by another Installation. A valid, unused token SHALL authorize exactly one Installation to join. An exact retry by that already admitted Installation MAY recover the same committed admission after expiry and SHALL NOT create another admission.

## TEAM.SYS.MEM-R04 — Membership Intervals

The system SHALL model membership as intervals: `[admitted, revoked?)`. A member's interval SHALL begin at admission and end at revocation. Phase 1 SHALL reject causal re-admission after revocation, so a member cannot have gaps. The system SHALL support historical queries: `was_member(identity, at_timestamp)`. Historical timestamps are signed author-declared event time and SHALL NOT authorize new writes.

## TEAM.SYS.MEM-R05 — Epoch Advancement

The system SHALL advance a workspace-local membership epoch once for every committed post-genesis `admit` or `revoke`, whether the operation was authored locally or received from another Installation. The epoch SHALL be a durable monotonically increasing notification cursor and SHALL NOT be treated as distributed membership state or as a Content Key Generation. Each transition SHALL include the content-derived `MembershipVersion` of the accepted operation set, whether it admitted or revoked, the subject `PublicIdentity`, and the author. The system SHALL let subscribers recover every transition after a supplied epoch. Synchronization authorization SHALL follow the current accepted membership (`TEAM.SYS.TRANS-R06`). Version 1 does not rotate Content Keys on membership transitions (ADR 0008); starting a new Content Key Generation on revocation is roadmap Phase 5 work.

## TEAM.SYS.MEM-R06 — Revocation Consequences

Revoking a member SHALL end their current membership interval, and the revocation SHALL reach every Member through membership replication (`TEAM.SYS.MEM-R10`). Once an Installation has accepted the revocation, it SHALL NOT serve records to, accept records from, or keep synchronization sessions open with the revoked Installation, and the revocation SHALL reach its local Packages as one membership transition. The revoked member SHALL retain access to data they previously received. In version 1 this synchronization gate is the only protection for future content: the revoked Installation keeps each Content Key it already holds, so a record it obtains outside the gate, for example from a Member that has not yet learned the revocation or from a colluding Member, remains readable to it. Content Key rotation on revocation, which protects future content cryptographically, is scheduled for roadmap Phase 5 (DELTA-003).

## TEAM.SYS.MEM-R07 — Membership Projection

The system SHALL project the canonical membership log into a current member snapshot. The snapshot SHALL be queryable by packages. The projection SHALL be deterministic — replaying the same log from genesis SHALL produce the same snapshot.

## TEAM.SYS.MEM-R08 — Manager-Only Membership

The initial implementation SHALL use manager-only membership: only members with appropriate capability may admit or revoke members. Free delegation (any member may delegate onward) is deferred. The membership log schema SHALL support future capability models without migration.

## TEAM.SYS.MEM-R09 — Local Workspace Join

A joining installation SHALL NOT report membership until it has validated and durably stored the workspace identity, genesis, canonical membership history required for validation, and its own accepted admission operation. The admitting installation SHALL validate the invitation claim, spend the invitation once, and commit the admission before reporting success. Both installations SHALL derive the same current membership from the accepted operation set. Join retries after interruption SHALL be safe and SHALL NOT create duplicate admission operations. A retry by the installation already admitted through that invitation MAY recover the accepted result, but another installation SHALL NOT reuse the invitation.

## TEAM.SYS.MEM-R10 — Membership Replication

Every Member SHALL eventually receive, from any reachable current Member, every accepted membership operation it lacks, independent of whether any Package has synchronized records. Replication SHALL be idempotent, SHALL apply an operation only after its causal dependencies, SHALL validate and authorize each received operation exactly as a locally authored one, and SHALL resume after restart or disconnection without a manual refresh. An Installation MAY accept membership operations pushed by a peer it does not yet know as a Member, because operations are self-authenticating, but SHALL NOT create a Workspace from them. A former Member MAY receive only the operations that establish its own revocation. A peer with no membership history SHALL receive no membership operations.
