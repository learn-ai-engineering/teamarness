# 0004 — p2panda causal DAG for canonical membership

**Status:** accepted (2026-09-12)

## Context

Decision 0002 placed canonical membership in the core runtime and described it as a signed, ordered, append-only log. The first requirements made that order a linear hash chain.

Research for the core runtime found that a linear chain handles concurrent offline managers by rejecting one branch and forcing a retry. That is a poor fit for a local-first workspace. `p2panda-auth` 0.7.1 already models group authority as a dependency DAG and resolves concurrent manager changes with strong removal.

The first core design adopted p2panda but left the old linear-chain wording in place. It also omitted application authentication, a durable genesis anchor, and a distributed state identifier. This decision settles those points.

## Decision

### Canonical membership is a signed causal DAG

Each workspace has one immutable, anchored genesis operation. Every later membership operation links to the causal heads known to its author. The links form a directed acyclic graph rather than one predecessor chain.

Operations are append-only and content-addressed. The operation ID is a BLAKE3 digest of a versioned canonical unsigned encoding. The author signs that encoding with the installation's Iroh-compatible Ed25519 key under a membership-specific domain. Resonance verifies the hash, signature, workspace, genesis descent, dependencies, timestamp rules, and action shape before `p2panda-auth` processes the operation.

`p2panda-auth` with `StrongRemove` resolves concurrent authority changes. Resonance exposes only `admit` and `revoke` in Phase 1. The p2panda adapter and access levels remain private to the core runtime.

This decision supersedes the linear hash-chain and single-predecessor membership wording in decision 0002. Decision 0002's ownership model and narrow core-to-package seam remain accepted.

### Identity adapters preserve one installation key

The Iroh `SecretKey` remains the installation secret. A local tagged identifier type adapts installation public keys and workspace IDs to p2panda's requirement that actors and groups share one `Author` type. The adapter does not create another keypair or another public identity.

### Membership version and epoch have different jobs

`MembershipVersion` is a digest of the workspace genesis and sorted accepted operation IDs. It identifies a membership operation set independent of processing order.

`MembershipEpoch` is a workspace-local durable progress counter used for ordered notifications. Genesis is epoch 0. Every committed post-genesis `admit` or `revoke` advances the counter once. Equal epoch numbers do not imply equal distributed state, so packages use `MembershipVersion` when state identity matters.

Shared package-key generation and distributed finality remain Phase 4 work.

### Historical time is declared event time

Membership operations carry signed author-declared timestamps that increase along causal links. `was_member(identity, at_timestamp)` resolves the causally closed operation set at that declared time. Late or backdated operations can revise historical answers after synchronization, so historical time is not an authorization proof. New writes bind to a membership version and current package key state.

### Invitations use one issuing authority in Phase 1

Only a manager may issue an invitation. The issuing installation is the only installation that may redeem it and author the admission. Redemption atomically records the used nonce and admission in the core database. The admission carries the signed token and invitee proof so later replicas can audit it.

Distributed token spending across managers is deferred with membership synchronization. Phase 1 does not claim cross-database serialization.

### Core persistence uses one database

One core SQLite database owns installation identity, workspace creation state, canonical operations, invitation spends, membership versions, and epoch events. This gives workspace creation and invitation admission one transaction boundary. Package persistence remains separate under decision 0002.

## Evidence

- `p2p-local-first-user-management.md`
- `context/02-system/01-identity/requirements.md`
- `context/02-system/02-membership/requirements.md`
- `p2panda-auth` 0.7.1 `traits::Operation`, `GroupCrdt`, and `StrongRemove` source
- `p2panda-core` 0.7.1 `Hash`, `Author`, and `OperationId` source

## Rejected alternatives

### Linear hash chain

Rejected. It gives simple replay but cannot accept concurrent offline manager operations without choosing one branch and retrying the other.

### Random operation IDs

Rejected. A random ID does not prove that the stored content is unchanged.

### Epoch as distributed state identity

Rejected. Two peers can have the same operation count and different operation sets. The membership version digest carries state identity.

### Separate catalog and workspace databases

Rejected for core state. Workspace creation and invitation admission need atomic changes across catalog, membership, invitation, and epoch records.

## Consequences

- Root requirements, membership requirements, the ontology, and the Phase 1 roadmap use causal-DAG language instead of linear-chain language.
- Every implementation must authenticate operations before p2panda authorization processing.
- The core stores canonical operation bytes and verifies them during replay.
- p2panda version changes stay behind local adapters and integration tests.
- Historical membership answers may change when late operations arrive. They are for history and display, not write authorization.
- Phase 4 must design shared key rotation around membership versions rather than assuming a local epoch number is globally unique.
