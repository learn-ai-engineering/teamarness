# 0030 — Standing local Agent authority and Package-held Agent binding

**Status:** accepted (2026-10-24)
**Supersedes in part:** ADR 0023 (record-log 1.1 clause "Core signs … when, and only when, the calling Package Instance is bound to that Agent"; "one Agent is bound to at most one Package Instance"), ADR 0026 (clause "this decision does not enable a separate Conversations Provider to sign records as that Agent"; Person-only `message.post`)
**Refines:** ADR 0017 (Agent key custody), ADR 0023 (Workspace Agents), ADR 0026 (Host-derived actor context)

## Context

ADR 0023 gave every Workspace Agent a Core-custodied key and a local binding to one Package Instance, and let Core sign only for that Instance. ADR 0026 brokered Ports between Packages with Host-derived actor context, but refused to let a Provider such as Conversations sign as an Agent, deferring the authority decision to the Agent runtime workstream.

The Agent runtime is a generic Package (the Agent Package) that is the harness for one Agent per Instance and participates in other Packages only through their Ports. For the Agent to post a Message, Conversations must append with the Agent's key, yet Conversations is not the bound Package. A per-command grant could carry the bound Package's authority to the Provider, at the cost of a grant table, a new Core append entry point and a handshake for every future Provider. Teamarness runs as one trusted application under one human's authority on one Installation; trusted Package Processes already hold that authority (`context/02-system/05-packages/requirements.md`), and the product is not yet at the stage where per-call authorization earns its friction.

## Decision

1. **An Agent bound on this Installation is a local signer, like the Installation Person.** Core signs a record with a Workspace Agent's key for any Package Instance on this Installation when the Agent is current in Canonical Membership, its Steward's key group contains this Installation's key, and the Agent has at least one Package binding on this Installation. The calling Package's identity is not part of the rule. Anything else fails with `principal_not_bound`. Attribution is unchanged: the signer key resolves to the Agent Principal.

2. **Binding is Package configuration held by Core.** `agent_package_bindings` is keyed by Workspace and Package Identity: one Package Instance maps to at most one Agent; any number of Packages may map to the same Agent. The Steward's Installation alone may bind or unbind, through the new Semantic Capability `teamarness.agent-binding` 1.0, whose grant fixes the calling Package, so a Package configures only itself. Core clears a Package's bindings when this Installation removes the Agent; a peer-removed Agent is simply no longer current. Bindings are never replicated. No Shell control binds.

3. **The Host derives the Agent actor.** When the caller of a Port is bound to an Agent, the Host emits `actor:{kind:"agent", principal, identity}` in place of the Person actor. A Provider that accepts an Agent actor appends with that `principal`; Core applies rule 1. Callers still cannot supply an actor. Conversations 1.1 accepts Agent actors on `message.post` and adds `channel.list`; later Providers adopt the same pattern without a new authority decision.

## Rejected alternatives

- **Per-command actor grant.** Strongest guarantee that a Provider acts only when the bound Package asked, but adds machinery to every Provider for a threat (a trusted local Provider posting as the Agent unprompted) that is a local bug, not an authority breach, in the current single-user model.
- **Host-performed append into the Provider's log.** Bypasses the Provider's validation, projection and policy; breaks Package-owned record scopes.
- **Caller-keyed rule with Conversations bound as a side effect.** Couples Package identities inside Core and reintroduces the one-Agent-one-Package limit.
- **Shell-owned Agent settings.** The Shell is generic; harness configuration is the harness's own concern.

## Consequences

- Any trusted Package on an Installation can act as any Agent that Installation stewards and has bound. This is intended and is not a confidentiality boundary. Revisit when Teamarness gains per-Package authorization (the roadmap's ADR 0006 re-evaluation names Agent Packages as the motivating case) or multi-user Installations.
- Migration 0019 re-keys the binding table; Core gains `bound_agent`, `unbind_agent_package` and removal cleanup. An accepted `RemoveAgentKey` operation clears the Agent's binding rows in the same acceptance transaction; because only the Steward's Installation holds custody and binding rows, that is this Installation's cleanup, and a peer-removed Agent is otherwise filtered out by the current registry.
- The record-log 1.1 wire contract is unchanged; only the acceptance rule for `principal` widens. Package Process 1.0 and `teamarness.package-ports` 1.0 are unchanged; the envelope now has two documented actor kinds and the Rust SDK's `PortActor` gains `Agent`.
- Ontology gains **Agent Package** and **Agent Binding**; `TEAM.SYS.ID-R10`'s last sentence and the Packages and Conversations requirements are revised in the same change set.

## Evidence

- Workstream `agent-package`: alignment, research and design artifacts (see Provenance).
- `crates/teamarness-core/migrations/0017_principals.sql` (prior key), `crates/teamarness-core/migrations/0019_agent_binding_by_package.sql` (Package key), `crates/teamarness-core/src/principals/agent_keys.rs` and `crates/teamarness-core/src/runtime/workspaces.rs` (`bind_agent_package`, `unbind_agent_package`, `bound_agent`), `crates/teamarness-core/src/runtime/records.rs` (standing predicate; prior caller-keyed predicate in history), `crates/teamarness-core/tests/principals.rs` and `crates/teamarness-core/tests/migrations.rs` (qualifying tests), `crates/teamarness-package-host/src/broker.rs` (prior Person-only actor), `crates/teamarness-package-sdk/src/ports.rs` (prior `PortActor`), `packages/conversations-package/backend/src/backend.rs` (`authorize_person`).
- **Weaknesses:** the decision rests on the current trusted-process model and single-user Installation; it has no test that distinguishes an unprompted Provider post from an Agent-initiated one, because the rule makes them equivalent by design.

### Provenance (non-versioned)

- `~/.myflow/repositories/github.com/learn-ai-engineering/teamarness/workstreams/agent-package/{scope,research,design}/`
