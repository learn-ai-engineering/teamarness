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

The system SHALL advance a workspace-local membership epoch once for every committed post-genesis `admit` or `revoke`. The epoch SHALL be a durable monotonically increasing notification cursor. Each transition SHALL include a content-derived `MembershipVersion` that identifies the accepted operation set. The system SHALL let subscribers recover every transition after a supplied epoch. Package-specific keys and authorization SHALL rotate in response to versioned membership transitions.

## TEAM.SYS.MEM-R06 — Revocation Consequences

Revoking a member SHALL end their current membership interval. The revoked member SHALL retain access to data they previously received but SHALL NOT read or write future content. The revocation SHALL propagate to all packages within one epoch transition. Package key rotation on revocation SHALL protect future content; past content remains readable to the revoked member.

## TEAM.SYS.MEM-R07 — Membership Projection

The system SHALL project the canonical membership log into a current member snapshot. The snapshot SHALL be queryable by packages. The projection SHALL be deterministic — replaying the same log from genesis SHALL produce the same snapshot.

## TEAM.SYS.MEM-R08 — Manager-Only Membership

The initial implementation SHALL use manager-only membership: only members with appropriate capability may admit or revoke members. Free delegation (any member may delegate onward) is deferred. The membership log schema SHALL support future capability models without migration.

## TEAM.SYS.MEM-R09 — Local Workspace Join

A joining installation SHALL NOT report membership until it has validated and durably stored the workspace identity, genesis, canonical membership history required for validation, and its own accepted admission operation. The admitting installation SHALL validate the invitation claim, spend the invitation once, and commit the admission before reporting success. Both installations SHALL derive the same current membership from the accepted operation set. Join retries after interruption SHALL be safe and SHALL NOT create duplicate admission operations. A retry by the installation already admitted through that invitation MAY recover the accepted result, but another installation SHALL NOT reuse the invitation.