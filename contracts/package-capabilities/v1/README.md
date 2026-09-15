# Package Semantic Capabilities version 1

The Package Host negotiates each required capability before process spawn and sends an opaque grant in `initialize`. A grant is valid for one Package Identity, Workspace, Installation, selected capability version, and process generation. Calls carry the grant and method. They do not carry scope selectors.

## `teamarness.workspace-context` 1.0

`get` takes no arguments. It returns `workspaceId`, `workspaceName`, `installationIdentity`, and `installationDisplayName`.

## `teamarness.membership` 1.0

`current` returns the canonical member list, durable decimal-string epoch, and content-derived membership version. `was_member` accepts an installation identity and signed decimal-string Unix millisecond. Its result carries `informational_not_authorization` because historical membership cannot authorize a new write.

A Package opens `capability.stream` with its membership grant, method `subscribe`, and `afterEpoch`. Transition events contain the Core epoch, membership version, cause operation ID, and snapshot digest. Stream credit bounds delivery. The host drops its Core subscription when credit or writer capacity runs out and reopens after the last delivered epoch.

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

## `teamarness.synchronization.immutable-record-log` 1.0

This generic capability provides one encrypted, append-only record set for the
implicit Package Identity, Workspace, Installation, and Package Process
generation selected by its grant. Requests never repeat those scope selectors.
The closed machine-readable contract is `immutable-record-log.schema.json`.

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
State events report `waiting_for_history_access`, `offline`, `catching_up`,
`current`, or `degraded`; warning events contain only stable non-sensitive
codes. Every event consumes credit. A Package commits the cursor with its
projection and may safely deduplicate a replay by `recordId`.

### `open`

`{ "sealedRecords": [...] }` accepts 1–32 versioned envelopes and no more than
512 KiB of decoded sealed input. Results preserve request order and are either
`ready` (with canonical bytes) or `waiting_for_history_access`, `wrong_scope`,
or `corrupt`. The smaller bound keeps valid requests and responses below the
Package Process Protocol's 1 MiB frame limit.

`recordId`, `orderKey`, and `cursor` are distinct opaque values. `recordId` is
stable authenticated equality; `orderKey` is a bytewise-sortable causal order;
`cursor` is local delivery progress and is never an order offset. Core retains
raw signing and content-key custody. Packages receive no Iroh, P2Panda, SQLite,
private-key, or raw-key types. Version 1 permits one content key, creator-only
bootstrap, current-holder grants, and no protection removal, update, deletion,
or revocation-driven rotation.
