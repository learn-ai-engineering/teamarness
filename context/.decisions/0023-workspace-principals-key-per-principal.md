# 0023 — Workspace Principals and key-per-Principal attribution

**Status:** accepted (2026-10-21)
**Supersedes:** ADR 0022 (one Workspace Person per Installation; names as the only Person data; no cross-Package Person data beyond names; the `teamarness.person-names` capability)
**Refines:** ADR 0004 (what the membership group contains), ADR 0005 (what the invitation claim carries), ADR 0017 (which secrets Core custodies)

## Context

A Workspace needs participants that are not Installations. An AI agent runs as a Package on a Member's machine, has no Installation of its own, and must be attributed, named and listed like a person without being able to act as one. Every record today is attributed to the Installation key that signed it: the record envelope's author is derived from the local secret, the idempotency key and per-author log are keyed by that author, and Packages verify that a payload's declared author equals the Core-authenticated signer. The membership group holds Installation keys only, and only the Manager can add anything to it.

The project is pre-production. There are no deployed Workspaces to migrate and no external Package authors to keep compatible, so a contract can be replaced rather than versioned beside its predecessor.

## Decision

### 1. Principals

A **Workspace Principal** is a participant recognized within one Workspace: either a **Workspace Person** or a **Workspace Agent**. Every Principal is identified by a Workspace-local opaque Principal ID and has exactly one current signing key. Membership authorizes keys. Attribution of any record, membership operation or name edit is the signer key resolved to its Principal; no record envelope, Package payload or capability request carries an acting-Principal claim. "Author" keeps its meaning of record signer.

A Workspace Agent has exactly one **Steward**, which is a Workspace Person. The Steward's Installation generates and custodies the Agent's Ed25519 key beside its own Installation key, under the same custody rules as ADR 0017. The Agent key signs records only; it never issues membership operations or name edits. The Agent key is never exported, logged, displayed or carried in an invitation in version 1.

### 2. A Person is a key group

A Workspace Person is a p2panda-auth group nested in the Workspace group. The Person's Installation creates the group with itself as its sole manager. A Workspace Manager admits a Person by adding its group to the Workspace group with write access, and revokes a Person by removing the group. The Person adds and removes its own Agent keys inside its group with its own authority; the Workspace Manager is not involved. The Workspace creator stays an individual manager of the Workspace group, because p2panda forbids a group from holding manage access, and also holds a Person group for its Agents.

The joiner signs its Person group's create operation and embeds it in the invitation claim, so the issuer commits the group create and the group add in the same admission transaction (ADR 0005's admission commit point). The creator commits its own group create beside genesis. Teamarness replay accepts an operation when its author holds the authority the group requires for that action, not only when the author is the creator.

A Person's ID is derived from its group create operation. An Agent's ID is derived from the operation that first added its key, under a separate domain tag. Removing a Person removes its Agents with it. The no-gaps and no-readmission rules of membership apply to Agent keys.

### 3. Contracts

One Semantic Capability family, `teamarness.principals` 1.0, delivers Principals to Packages: rows keyed by Principal ID with kind, steward, current keys, name and fingerprint, plus lookup by key and by Principal ID, and the same bounded resumable snapshot stream as names had. It replaces `teamarness.person-names`, which is removed. `teamarness.membership` 1.0 is unchanged and remains the authorization view: a Package that needs kind or steward resolves it through the principals family.

`teamarness.synchronization.immutable-record-log` 1.1 adds an optional `principal` field to the append request. Core signs with that Principal's key when, and only when, the calling Package Instance is bound to that Agent on this Installation; otherwise it refuses. The binding is local Installation state, not replicated, and one Agent is bound to at most one Package Instance.

Names are Principal names. A Person's name edit is signed by the Person's key; an Agent's name edit is signed by its Steward's key. Only the Steward renames an Agent; the Steward or a Workspace Manager removes one.

## Rejected alternatives

- **Acting-Principal claim in the Core record envelope.** Core would vouch for a Principal it cannot cryptographically verify, every envelope and idempotency key would need a new version, and the Installation key would still be the only thing a peer can check.
- **Acting-Principal claim in the Package payload.** No Core change, but every Package repeats the "steward of signer" check, the record contract and CONV-R09 lose their "declared author equals signer" simplicity, and attribution becomes a Package-level assertion instead of a signature.
- **A Teamarness-level Agent registry beside the membership group, admitted by a Manager-issued ticket.** Reintroduces the Manager into every agent add, needs a replicated single-use ledger that the issuer-local nonce table cannot provide, and duplicates authority that p2panda nested groups already model.
- **Extending `teamarness.person-names` with a kind field.** A closed vocabulary makes it a new major anyway (ADR 0018), and the `identity` column would change meaning; a replacement family is cleaner while nothing depends on the old one.
- **Carrying kind in `teamarness.membership`.** Membership stays the authorization view; presentation data belongs with Principals.

## Consequences

- A Member is any key currently admitted directly or through a Person's group. The membership snapshot, roles and replay rules generalize from "Installation" to "key"; Shell and Package DTOs that assumed one Person per Installation are replaced.
- The Agent key is custodied like the Installation key: unencrypted in `core.sqlite3`, readable by trusted Package Backends, never returned by any API. Key export, steward transfer, hosted or shared agents, multi-Installation Persons and agent-specific permissions are left open for later decisions; the group model leaves room for all of them.
- The invitation claim grows by one signed group create operation; token layout and the two onboarding commit points are unchanged.
- Conversations and every future Package attribute by signer key and resolve presentation through `teamarness.principals`. CONV-R09 is unchanged; CONV-R17 names the new family.
- A Package Instance bound to an Agent is the only thing that can act as it. How that Package is configured, invoked, or given cross-Package reads is a separate decision.

## Evidence

- `crates/teamarness-core/src/synchronization/mod.rs` — author derived from the local secret; no caller-supplied author.
- `crates/teamarness-core/migrations/0006_immutable_record_log.sql` — idempotency keyed by author key.
- `crates/teamarness-core/src/membership/adapter.rs`, `membership/projection.rs` — Installation-only group members; replay accepts only creator-authored operations.
- p2panda-auth 0.7.1 `src/group/crdt/mod.rs` — nested groups, transitive members, `ManagerGroupsNotAllowed`, unauthorized-add rejection.
- `crates/teamarness-core/src/person_names.rs`, `src/runtime/person_name_sync.rs` — Person anchoring and name replication being generalized.
- `context/.decisions/0022-workspace-person-names.md` — the clauses replaced.
- **Weaknesses:** nested-group behaviour was verified by reading p2panda-auth's tests, not by running Teamarness replay against it. Agent key custody inherits every ADR 0017 limitation.

### Provenance (non-versioned)

- Workstream `agent-invite`: alignment `scope/20261019T065934Z_workspace-principals.md`, research `research/20261019T070643Z_principal-seams.md`, design `design/20261021T002739Z_workspace-principals.md`.
