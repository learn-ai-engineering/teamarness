# 0024 — Workspace Manager co-manages Person key groups

**Status:** accepted (2026-10-21)
**Supersedes:** ADR 0023 §2, sole-manager clause and Manager-uninvolved Agent removal clause only
**Refines:** ADR 0005 (claim binding to authenticated genesis)

## Context

ADR 0023 makes a Person a nested group managed only by its Steward, yet requires the Workspace Manager to remove an individual Agent. p2panda requires manage authority in the target group to remove a key. Revoking the Person removes every Agent, and asking its Steward to sign is not independent Manager removal.

## Decision

A Person group's create operation names its Steward and the Workspace creator as **individual** managers (once when both are the same key). The Workspace group still has only its creator as manager. The signed `create person` action includes that creator's key. The join claim binds the group create to its signed token's inviter, and membership replay checks the nominated creator against authenticated Workspace genesis. An unattached group gives no Workspace membership.

The Steward independently adds or removes its own Agent keys. The Workspace creator may remove a currently admitted, specifically named Agent from any admitted Person group without the Steward's secret. Teamarness rejects a creator-authored Agent add to another Person's group even though p2panda would allow it. Agent keys cannot author membership operations; removal does not permit readmission. The group is attached to the Workspace with write access, never manage, so its members receive no Workspace manage role.

## Consequences

The Manager can remove one Agent while its Steward and sibling Agents remain. Causal replay, StrongRemove, historical membership, snapshots, invitation commit points and bounded bootstrap retain their existing rules. Pre-production operation bytes are not migrated. ADR 0023's Principal attribution, custody and contracts remain in force. No separate deny registry, ticket or proxy Steward signature is introduced.

## Evidence

- `crates/teamarness-core/src/membership/adapter.rs`: direct p2panda two-individual-manager removal probe and nested-group access assertions.
- `crates/teamarness-core/src/membership/authority.rs`: authenticated genesis co-manager check and action-specific add/remove authority.
- `crates/teamarness-core/tests/membership_projection.rs`: authority matrix, concurrent Manager removal and Steward addition.
