# 0025 — Conflicting Principal key claims fail closed

**Status:** accepted (2026-10-22)
**Refines:** ADR 0023 §1–2 (one effective Principal per signing key), ADR 0024 (Steward authority)

## Context

Two admitted Stewards can independently sign Agent additions for the same public key if its holder signs a distinct proof for each group. Both operations can be valid at their causal positions; rejecting whichever arrives second makes peers disagree. The same key can be independently claimed as a Person and an Agent. SQLite's unique Principal key constraint cannot resolve a signed-operation conflict.

## Decision

Retain every authenticated, authorized signed operation in the canonical DAG. Derive an effective-key conflict from two or more independently accepted admission anchors for one key, including Agent/Person collisions and repeated independent adds within one group. Exclude that key from current membership, Principal rows, signer authorization, grants and peer sessions. A conflicted Person's group yields no effective Agent Principals. Conflict is sticky even if one branch is later removed or revoked; causal re-addition remains forbidden. The Workspace creator is the sole Manager and cannot be deactivated by a forged admission. Membership version and epoch still reflect all accepted operations; a conflict-causing admission emits a revocation of the effective subject, with durable subscription recovery reconstructing its effective change from the accepted prefix. The Principal roster wake advances in the same membership transaction.

## Rejected alternatives

- First arrival wins: opposite delivery orders diverge.
- Canonical ID winner: keeps a contested signer active and complicates dependent removals and presentation.
- Discarding a valid concurrent signed operation: loses a historical fact and makes acceptance order authoritative.

## Consequences

A once-visible key can cease to be effective when a second valid claim arrives. Previously delivered records cannot be cryptographically revoked. Historical signed facts remain available for replay, while effective membership and Principal attribution are derived from the complete accepted set. Honest adds generate fresh keys and retain their existing behavior. No migration, public capability change, or general-purpose consensus mechanism is introduced.

## Evidence

- `crates/teamarness-core/src/membership/adapter.rs`: direct p2panda probe processes two signed additions in both orders.
- `crates/teamarness-core/src/membership/{authority,projection}.rs`: causal authority and accepted-set effective projection.
- `crates/teamarness-core/tests/membership_convergence.rs`: two-order acceptance, cross-kind collision, sticky removal, subscription and restart checks.

### Provenance (non-versioned)

Corrective design: agent-invite `design/20261022T095527Z_concurrent-key-and-named-agent-correction.md`.
