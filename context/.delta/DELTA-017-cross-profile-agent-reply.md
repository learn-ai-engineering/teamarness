# DELTA-017 — The external cross-profile Agent reply case fails

**Status:** open (2026-10-31). Accepted by Don on 2026-10-31 as pre-existing on `main`. Owner: a follow-up diagnosis on `main`; the Packagement branch does not fix it.

## Divergence

`TEAM.SYS.PKG-R13` and ADR 0030 let a Workspace Agent bound to a Package Instance read and reply to Conversations as that Agent. A mention from a Member on another Installation should reach the Agent through replicated Conversations events and get one reply.

The external Agent/Host integration target (`tests/system/tests/agent_reply_flow.rs`, ignored cases run with the standalone Agent) does not show this today. Two cases fail at the first cross-profile reply assertion, "the Agent's reply did not reach alice":

- `the_agent_replies_once_when_addressed_across_two_profiles` (main's case)
- `the_live_installed_agent_replies_once_when_addressed_across_two_profiles` (the Packagement live-install variant)

Main's case fails the same way on unchanged `main` `aa0b0ae`, against the same freshly built Agent. The other eight external cases pass. The `agent-port-discovery` closeout recorded all nine cases passing on 2026-10-27, and only documentation changed on `main` between `6073dd0` and `aa0b0ae`.

## Diagnosis so far

These observations come from temporary debug output during the Packagement Ports 2.0 integration. The cause is not confirmed.

- The Agent reports `ready`, with Conversations enabled at major 1.
- Bob's mention is in Alice's `#general`.
- The Agent's progress advanced past the mention, but the stub model endpoint received no connections.
- Afterwards, Alice's `principal_roster` lists Bob as a Person.
- The Agent replies only when the Principal lookup of the mention's author returns a Person. It acknowledges and skips a mention whose author resolves to nothing.

The likely cause is that the Principal lookup did not yet resolve Bob when the Agent processed the event: `workspace_principal` does not apply pending membership maintenance, but `principal_roster` does. Loopback networking in the test sandbox works, so the stub endpoint is reachable.

## Impact

A teammate on another Installation may mention an Agent and get no reply, if the mention arrives before the author's membership is resolved locally. The Agent does not retry a skipped mention. Same-Installation mentions, rebinding, uncertainty and unknown-Provider cases are not affected.

## Follow-up

On `main`:

1. Confirm whether the author's Principal lookup returns nothing for Bob when the Agent processes the mention.
2. If it does, make the lookup apply pending membership maintenance as `principal_roster` does, or make the Agent defer an unresolved author instead of acknowledging the mention.
3. Close this delta when both cross-profile external cases pass.

## Tracking

- `TEAM.SYS.PKG-R13`, ADR 0030, `context/roadmap.md` (Packagement and Agent-controlled Package Ports).
- Workstream `packagement`: `implement/ports-2-integration-checkpoint.md` (Phase C) and Verify `verify/20261031T024307Z_validation-ports-2.md`, in the resolver-selected MyFlow store.
