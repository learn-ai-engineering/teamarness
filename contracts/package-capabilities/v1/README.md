# Package Semantic Capabilities version 1

The Package Host negotiates each required capability before process spawn and sends an opaque grant in `initialize`. A grant is valid for one Package Identity, Workspace, Installation, selected capability version, and process generation. Calls carry the grant and method. They do not carry scope selectors.

## Families and versions

A capability identity names a **family**: one contract shape, such as the immutable record log. A family evolves by minor version within a major, and a minor may only add — new optional request fields, new event kinds a Package may ignore, wider bounds. Nothing is removed or narrowed within a major.

The Package declares a family and the versions of it it accepts; the Host offers a major and a range of minors and grants the highest minor both accept. A family the Host does not provide is an unavailable capability, not a version error.

A different shape is a **new family**, declared and negotiated beside the existing ones — document-oriented synchronization would be one — never a new version of an existing family. Adding a family changes no lifecycle operation, no Package Process Protocol version, and no Package UI contract, and a Package that does not declare it is unaffected.

See ADR 0018, Package seam versioning, for the policy, including the deprecation window intended for the first breaking change.

## `teamarness.workspace-context` 1.0

`get` takes no arguments. It returns `workspaceId`, `workspaceName`, `installationIdentity`, and `installationDisplayName`.

## `teamarness.membership` 1.0

`current` returns the canonical member list, durable decimal-string epoch, and content-derived membership version. `was_member` accepts an installation identity and signed decimal-string Unix millisecond. Its result carries `informational_not_authorization` because historical membership cannot authorize a new write.

A Package opens `capability.stream` with its membership grant, method `subscribe`, and `afterEpoch`. Transition events contain the Core epoch, membership version, cause operation ID, and snapshot digest. Stream credit bounds delivery. The host drops its Core subscription when credit or writer capacity runs out and reopens after the last delivered epoch. Core transitions also know the admitted or revoked Installation and the author; the 1.0 event does not carry them. A later minor version will.

## `teamarness.person-names` 1.0

This additive, Package-neutral presentation capability is scoped by its grant to one Workspace. `current` returns `{workspaceId, cursor, persons}`; each Person contains `identity` (source Installation), opaque `personId`, nullable `name`, and an identity fingerprint. `lookup` accepts a current Installation identity and returns `{person}` or `{person:null}` for a nonmember; a missing name is not a membership error. The current roster comes from Core, not from the Package. Names are non-unique, bounded Unicode text; authorization remains in `teamarness.membership` 1.0, which is unchanged.

A `capability.stream` with method `subscribe` and decimal-string `afterCursor` emits the latest bounded snapshot when the independent durable profile generation changes. The cursor is a coalescing invalidation token, **not** a replay ledger, membership epoch or immutable-record cursor: subscribe first, read `current`, and refetch after a gap or restart. A cursor ahead of Core's profile generation closes with `cursor_invalid`. Each event consumes stream credit. The closed schema and limits are in `person-names.schema.json`.

## `teamarness.signing` 1.0

`sign` and `verify` accept a visible ASCII purpose of 1 to 64 bytes and an unpadded base64url payload of at most 262144 decoded bytes. The host uses the fixed Core signing context `package-signing/v1`.

The signed envelope is binary and length-delimited:

1. capability major as unsigned 16-bit big-endian;
2. capability minor as unsigned 16-bit big-endian;
3. Package Identity byte length as unsigned 16-bit big-endian, then UTF-8 bytes;
4. Workspace ID byte length as unsigned 16-bit big-endian, then ASCII bytes;
5. purpose byte length as unsigned 16-bit big-endian, then ASCII bytes;
6. payload byte length as unsigned 32-bit big-endian, then payload bytes.

Signatures and payloads use unpadded base64url. Private keys and signing-context selection never cross the process boundary.

## `teamarness.synchronization.immutable-record-log` 1.0 and 1.1

This generic capability provides one encrypted, append-only record set for the
implicit Package Identity, Workspace, Installation, and Package Process
generation selected by its grant. Requests never repeat those scope selectors.
The unchanged 1.0 contract is `immutable-record-log.schema.json`; the additive 1.1 contract is `immutable-record-log-1.1.schema.json`.

### `append`

```json
{ "purpose": "message.post", "canonical": "<unpadded base64url>" }
```

`purpose` is visible ASCII of 1–64 bytes. `canonical` is 1–262,144 decoded
bytes. Success is returned only after a local durable commit and has the shape
`{recordId, orderKey, author, signature, sealedRecord, durability:"committed"}`.
The operation is content-idempotent within its granted scope, so an uncertain
retry returns the original committed result.

### `subscribe`

`{ "afterCursor": null }` replays from the beginning; otherwise the cursor is
an Installation-local journal checkpoint. The stream is credit-controlled and
at-least-once. Record events include both canonical bytes and the sealed
Teamarness envelope so a Package can persist ciphertext and its own indexes.
State events report availability and a pending record count; warning events
contain only stable non-sensitive codes. Every event consumes credit. A Package
commits the cursor with its projection and may safely deduplicate a replay by
`recordId`.

For a negotiated 1.1 stream starting with `afterCursor:null`, Core captures a journal-scoped fixed high-water while serializing append and subscription. Once bounded pages scan that finite local prefix, the Host sends one separately credited `{kind:"replay_end",cursor,gapFree}` event **after** all earlier records on that same stream. New live appends do not postpone the marker. The cursor includes the journal generation; `gapFree:false` reports unavailable history, quarantined or skipped rows, or an otherwise incomplete prefix, never absence. A resumed cursor, an idle page, `current` health, zero pending count, or remote network progress cannot certify this Installation's from-start prefix. The SDK calls a qualified callback only after successful record ingestion and credit; stream close/cancellation invalidates it. A Package that needs absence proof requires minor 1. A 1.0 stream never receives `replay_end`.

The Host retires a Package Process generation's append admission before replacement activation: queued work is cancelled at Core closure entry; entered work drains through closure exit even if its caller timed out. Incomplete drain leaves that Instance failed, so no new subscription can capture a high-water ahead of a still-running old append. This fence is independent per Instance; the Host-wide limiter only bounds capacity.

The first event of every stream is a `state` event. Afterwards one is sent when
availability changes, when `pendingRecordCount` changes while not
`catching_up`, and at most every 128 records while `catching_up`.

| Availability | Meaning |
|---|---|
| `waiting_for_history_access` | No history can be read yet: the Package's record set has no key on this Installation. |
| `offline` | Nothing is reachable, or this Installation may no longer participate. The warning code says which. |
| `catching_up` | Records are known to be outstanding and peers are reachable. |
| `current` | Everything known to this Installation has been delivered. |

`pendingRecordCount` is the number of records this subscription still owes the
Package: records already committed but not delivered, records held back waiting
for their author or key, and records a running session has announced but not yet
applied.

Warning codes are `record_quarantined` (a record failed authentication and was
skipped), `scope_quarantined` (the whole record set is isolated after a local
integrity failure), and `membership_revoked` (this Installation is no longer a
current Member). `record_quarantined` is sent once per page that skipped a
record; the others are sent once per stream.

A synchronization session exchanges at most 2,048 records or 8 MiB in each
direction; larger backlogs continue in later sessions and are reported as
`catching_up`.

If `afterCursor` belongs to another delivery journal or is ahead of the journal,
the Host closes the stream with code `cursor_invalid`. The Package must clear its
stored cursor and subscribe again from `null`; replayed records are deduplicated
by `recordId`.

### `open`

`{ "sealedRecords": [...] }` accepts 1–32 versioned envelopes and no more than
512 KiB of decoded sealed input. Results preserve request order and are either
`ready` (with canonical bytes) or `waiting_for_history_access`, `wrong_scope`,
or `corrupt`. The smaller bound keeps valid requests and responses below the
Package Process Protocol's 1 MiB frame limit.

`recordId`, `orderKey`, and `cursor` are distinct opaque values. `recordId` is
stable authenticated equality; `orderKey` is a bytewise-sortable causal order;
`cursor` is local delivery progress and is never an order offset. Host validation
bounds decoded canonical input to 262,144 bytes and each open batch to 512 KiB;
the process protocol bounds encoded and decoded frames to 1 MiB. Core retains
raw signing and content-key custody. Packages receive no transport, replication,
private-key, or raw-key types. Version 1 permits one content key, creator-only
bootstrap, current-holder grants, and no protection removal, update, deletion,
or revocation-driven rotation.
