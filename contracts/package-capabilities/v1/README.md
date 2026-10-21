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

## `teamarness.principals` 1.0

This Package-neutral presentation capability is scoped by its grant to one Workspace. It delivers Workspace Principals: the Workspace Persons and Workspace Agents whose keys Canonical Membership authorizes. It replaces the names capability of ADR 0022, which no longer exists (ADR 0023).

`current` returns `{workspaceId, cursor, principals}`. Each Principal row contains an opaque `principalId`, `kind` (`person` or `agent`), `stewardPrincipalId` (the Steward's Principal ID for an agent, `null` for a person), `keys` (the Principal's current signing keys as 64-hex identities; exactly one in version 1), nullable `name`, and a fingerprint of the Principal ID. `lookup` accepts exactly one of `identity` (a current member key) or `principalId` and returns `{principal}` or `{principal:null}` when no current Principal matches; a missing name is not a membership error. The current roster comes from Core, not from the Package. Names are non-unique, bounded Unicode text; a Person names itself and a Steward names its Agents. Authorization remains in `teamarness.membership` 1.0, which is unchanged: an Agent appears there as a Member with role `Member`, and a Package that needs kind or steward resolves them here.

Attribution is the record author resolved to its Principal: a Package looks up the signer key and reads `kind` and `stewardPrincipalId` from the row. No request or record carries an acting-Principal claim.

A `capability.stream` with method `subscribe` and decimal-string `afterCursor` emits the latest bounded snapshot when the independent durable profile generation changes. The cursor is a coalescing invalidation token, **not** a replay ledger, membership epoch or immutable-record cursor: subscribe first, read `current`, and refetch after a gap or restart. A cursor ahead of Core's profile generation closes with `cursor_invalid`. Each event consumes stream credit. The closed schema and limits are in `principals.schema.json`.

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

## `teamarness.synchronization.immutable-record-log` 1.1

This generic capability provides one encrypted, append-only record set for the
implicit Package Identity, Workspace, Installation, and Package Process
generation selected by its grant. Requests never repeat those scope selectors.
The closed machine-readable contract is `immutable-record-log.schema.json`.

Minor 1.1 adds one optional `append` field, `principal`. A Package that
negotiates 1.0 sends 1.0 requests and is unaffected.

### `append`

```json
{ "purpose": "message.post", "canonical": "<unpadded base64url>" }
```

`purpose` is visible ASCII of 1–64 bytes. `canonical` is 1–262,144 decoded
bytes. Success is returned only after a local durable commit and has the shape
`{recordId, orderKey, author, signature, sealedRecord, durability:"committed"}`.
The operation is content-idempotent within its granted scope and author, so an
uncertain retry returns the original committed result.

Without `principal`, Core signs with the Installation key and `author` is the
Installation identity. With `principal` (a 64-hex Principal ID, 1.1 only), Core
signs with that Workspace Agent's key and `author` is the Agent's key, when and
only when the calling Package Instance is bound to that Agent on this
Installation. Any other `principal`, including a Person, an Agent bound to
another Package Instance, an unbound Agent or an unknown ID, fails with the
error code `principal_not_bound` and commits nothing. The binding is local
Installation state and is never replicated; one Agent is bound to at most one
Package Instance. The Agent's key never crosses the capability boundary. Peers
accept the record because the Agent's key is in membership history, and a
reader resolves the author to its Principal through `teamarness.principals`.

### `subscribe`

`{ "afterCursor": null }` replays from the beginning; otherwise the cursor is
an Installation-local journal checkpoint. The stream is credit-controlled and
at-least-once. Record events include both canonical bytes and the sealed
Teamarness envelope so a Package can persist ciphertext and its own indexes.
State events report availability and a pending record count; warning events
contain only stable non-sensitive codes. Every event consumes credit. A Package
commits the cursor with its projection and may safely deduplicate a replay by
`recordId`.

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
