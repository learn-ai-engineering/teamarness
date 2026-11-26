# 0048 — Backlog Tickets have derived short references and a Cancelled status

**Status:** accepted (2026-11-26)
**Refines:** ADR 0039 (adds a status value to the per-field Ticket model), ADR 0042 (adopts its one-time re-evaluation after a format upgrade, for Backlog records)

## Context

People and Agents refer to Backlog Tickets in conversation. A Ticket is identified by the Record ID of its create record (`TEAM.SYS.BACKLOG-R02`): 64 lowercase hexadecimal characters, the output of Core's domain-separated hash of the record's canonical bytes (`crates/teamarness-core/src/synchronization/mod.rs`, `RecordId::digest`). That is too long to say or type, and Ticket titles are neither unique nor stable.

Installations create Tickets independently and offline, and the record log has no global sequence or compare-and-set (ADR 0039). Any identifier assigned at creation time can therefore collide between Installations that have not seen each other's records.

Ticket status is one of `not_ready`, `ready`, `in_progress` and `done` (`TEAM.SYS.BACKLOG-R05`). There is no way to close a Ticket that will not be done without marking it `done`, which misstates the outcome.

Status travels in the `TBLOG\x01` codec as a one-byte code (1–4). A build that decodes an unknown code fails the whole record, and the store quarantines it as `malformed_record` (`packages/backlog-package/backend/src/domain.rs`, `Status::from_code`; `store/mod.rs`, ingest). A quarantined record is never opened again: `sealed_rows` replays only applied and waiting rows. The sealed record bytes stay in the store.

## Decision

### 1. A Ticket's reference is a prefix of its ID

A Ticket's **reference** is the shortest prefix of its Ticket ID that is at least four characters long and that no other Ticket known to this Installation shares. Every Ticket the Installation has applied counts: archived Tickets and the Tickets of archived Epics included. A reference is shown with a leading `#`, for example `#a3f2`.

The reference is computed from the projection. It is not stored, not carried in any record, and needs no codec, migration or coordination. Existing Tickets have references immediately.

When a newly arrived Ticket shares a Ticket's four-character prefix, both references lengthen until they differ, for example `#a3f2c` and `#a3f27`. Until their records have reached each other, two Installations can show a Ticket's reference at different lengths. They converge with the records.

Epics have no references.

### 2. Looking up a Ticket by reference

`ticket.get` accepts either `ticketId` or `ref`, exactly one. A `ref` is 4 to 64 hexadecimal characters with an optional leading `#`, matched case-insensitively against the start of every known Ticket ID, archived or not.

- One match: the Ticket, as for `ticketId`.
- No match: `backlog.not_found`.
- More than one match: `backlog.ambiguous_ref`, whose details list each candidate's `ticketId`, current `ref` and `title`, at most 16, with a flag when more exist. Nothing is guessed.

`ticket.update` and every other write keep taking the full `ticketId`. A caller resolves a reference with `ticket.get` first, and the update's `baseVersion` already comes from that read.

`ticket.list` summaries and `ticket.get` include `ref`. The UI shows the reference in the navigation list and in the Ticket detail, where it can be copied.

### 3. A Cancelled status

Status gains `cancelled`, codec code 5: the Ticket is closed and will not be done. It lists after `done`. It converges like any other status change (ADR 0039 rule 2).

The status filter gains **Not done**, which matches `not_ready`, `ready` and `in_progress`. With no remembered filter, the status filter is **Not done** and the priority filter is **All**. **All** remains selectable and shows every status. Remembered filters (`TEAM.SYS.BACKLOG-R07`) are unchanged.

### 4. Port Contract 1.1

`dev.teamarness.backlog` 1.1 adds `ref` to Ticket summaries and Tickets, `ref` as an alternative input to `ticket.get`, `backlog.ambiguous_ref`, and the `cancelled` status. The Provider guide explains references, ambiguity and Cancelled. The bump is a minor one (ADR 0018). A 1.0 consumer that treats status as a closed set can now see `cancelled` and must tolerate it.

### 5. Re-evaluating records an older build could not read

The first activation on a build with this decision re-decodes, once, every stored record quarantined as `malformed_record`. A record that now decodes is applied in log order like a newly ingested one; a record that still fails stays quarantined. This is ADR 0042's format-upgrade re-evaluation, applied to the Backlog codec. Each later addition of a codec value triggers it again, keyed to a stored codec generation.

## Rejected alternatives

- **Sequential numbers.** They need a global counter, which the record log does not have. Installations offline from each other would hand out the same number.
- **A random short code chosen at creation, avoiding codes already seen.** It needs a new stored record field and a backfill for existing Tickets. Installations offline from each other can still pick the same code, so it also needs a rule for two Tickets with one code. That rule is the one in Decision 1, with extra state.
- **A fixed-length prefix.** Collisions arrive early. With 4 hexadecimal characters (65,536 values), the chance that two of 100 Tickets share a prefix is about 7%, and about 50% at 300 Tickets.
- **Hashing the Ticket ID again.** The ID is already a hash output. Hashing again changes nothing about uniqueness.
- **A whole new codec version for `cancelled`.** Every older build would quarantine every new record, not only those carrying the new status.
- **Leaving quarantined records quarantined.** An Installation that received a `cancelled` change before it upgraded would never show it.

## Consequences

- A reference written down earlier can become ambiguous when a Ticket with the same prefix arrives. A lookup then lists the candidates instead of returning the wrong Ticket. A reference that is unique stays valid: it is still a prefix of the same ID.
- An Installation on an older build quarantines a record that sets `cancelled`, together with the other fields that record changes, and shows its projection warning until it updates. After the update, Decision 5 applies the record. The release notes must say so.
- A Member could grind create records so that a new Ticket shares another Ticket's prefix. The only effect is a longer reference and an ambiguous lookup, never a wrong Ticket.
- `TEAM.SYS.BACKLOG-R05`, `R08` and `R09` change with this decision: the status list, the default and **Not done** filters, references, and Port Contract 1.1.
- Left open: a **Not done** filter on `ticket.list` for Agents, and references for Epics.

## Evidence

- `packages/backlog-package/backend/src/domain.rs`: Ticket identity is the create record's `RecordId`; `Status::from_code` refuses unknown codes.
- `packages/backlog-package/backend/src/store/mod.rs`: an undecodable record is quarantined as `malformed_record`; `sealed_rows` replays only applied and waiting records; sealed bytes are retained.
- `crates/teamarness-core/src/synchronization/mod.rs`: `RecordId::digest` hashes the domain-separated canonical record bytes, so ID prefixes are uniformly distributed.
- `context/.decisions/0042-preserved-blocks-and-upgrade-reevaluation.md`: the one-time re-evaluation pattern after a format upgrade.
- Collision figures use the birthday approximation 1 − e^(−n²/2N), with N = 16^k for a k-character prefix.
- **Weakness:** nothing in this decision has been implemented or measured. The re-evaluation step assumes the stored sealed bytes are enough to reopen a quarantined record, which holds for applied and waiting rows but is untested for quarantined rows.
