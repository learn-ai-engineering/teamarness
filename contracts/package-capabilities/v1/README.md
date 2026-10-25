# Package Semantic Capability contract collection v1

The Package Host negotiates each required capability before process spawn and sends an opaque grant in `initialize`. A grant is valid for one Package Identity, Workspace, Installation, selected capability version, and process generation. Calls carry the grant and method. They do not carry scope selectors.

## Families and versions

A capability identity names a **family**: one contract shape, such as the immutable record log. A family evolves by minor version within a major, and a minor may only add — new optional request fields, new event kinds a Package may ignore, wider bounds. Nothing is removed or narrowed within a major.

The Package declares a family and the versions of it it accepts; the Host offers a major and a range of minors and grants the highest minor both accept. A family the Host does not provide is an unavailable capability, not a version error.

A different shape is a **new family**, declared and negotiated beside the existing ones — document-oriented synchronization would be one — never a new version of an existing family. Adding a family changes no lifecycle operation, no Package Process Protocol version, and no Package UI contract, and a Package that does not declare it is unaffected.

See ADR 0018, Package seam versioning, for the policy, including the deprecation window intended for the first breaking change.

## `teamarness.package-ports` 2.0

The closed [2.0 wire schema](package-ports-2.0.schema.json) defines the final request and result shapes, with examples under each definition. Family versions are independent of this `v1` collection, Package Process 1.0, and each Provider's Port Contract version. The Host offers only capability 2.0. Major-1 manifests fail negotiation with `capability_version_unsupported` and must be rebuilt; there is no legacy dispatcher (ADR 0035).

**Phase 3 checkpoint, not a completed 2.0 release:** declared and Agent metadata, durable self-scoped enable/disable and shared dynamic dispatch are qualified. Bound-Agent guide-bearing access defaults to `port_not_enabled` through both views. Subscription revocation ownership/lease issuance, atomic enqueue/revocation and stream-open races remain mandatory Phase 4 work, including Agent-authorized declared streams. External harness and final generic-client qualification remain pending.

Every grant fixes the caller, Workspace, Installation, and process generation. Requests never accept a caller Package, Provider, Workspace, or actor selector. `access` selects `declared` composition or `agent` access; it is not an authority claim. Agent access requires a current binding and never falls back to Person authority.

### Summary and complete metadata

`{method:"available", access:"declared", cursor?}` lists actual declared contract dependencies, without requiring a binding. Each row contains `contract`, declared `major`, nullable selected `minor`, nullable `description`, `available`, `enabled`, nullable `enabledMajor`, and nullable `metadataSha256`. Optional missing Providers remain rows with no metadata and `available:false`. Schemas and Port arrays are not summary fields. Admitted on-demand Providers are available without starting a process. Suspended, failed, disposed, or missing Instances are unavailable.

A page has at most 32 rows and **32 KiB of encoded JSON**, including its optional `nextCursor`. Pass that opaque cursor unchanged. It binds the caller, Workspace, access view, admission revision, and last sorted contract identity. Malformed, cross-scope, or stale cursors return `port_catalog_changed`. Reopening the Host invalidates cursors; no per-client snapshot is retained. Availability is current when sampled, not a snapshot promise.

`{method:"describe", access:"declared", contract, metadataSha256, offset}` returns `{metadataSha256,totalBytes,offset,data,nextOffset?}`. `offset` is an integer byte offset from zero through `totalBytes - 1`. `data` is standard padded base64 containing at most **64 KiB decoded bytes**, starting exactly at that offset. The whole response stays below the **256 KiB value budget**. `nextOffset`, when present, is the next byte after the chunk. Offsets need not be aligned. Invalid offsets return `capability_request_invalid`.

Decoded bytes form deterministic compact UTF-8 JSON: `{schemaVersion:1,$defs,contracts:[oneProjectedContract]}`. Declared metadata contains exactly declared Ports, sorted by name, and every transitively reachable local definition. Ordinary queries retain request/response schemas; live queries retain request/event schemas; events retain only an event schema. No schema is truncated. The complete document is at most **1 MiB**, the admission document bound. The digest is lowercase SHA-256 over the exact projected bytes, checked for every request. A mismatched digest returns `port_catalog_changed`; discard partial assembly and list again. Inaccessible contracts return `ungranted_port`; unavailable metadata returns `port_unavailable`. Discovery never activates a Provider and never exposes graph-rejected candidates.

The SDK's `declared_contracts` follows pages; `describe` bounds assembly, checks base64, offsets, stable totals, and digest, then compiles the complete document and refuses dangling references. See the real declared consumer in `tests/system/src/bin/conversations_port_consumer.rs`.

### Agent view and durable settings

`{method:"available",access:"agent",cursor?}` lists admitted eligible guide-bearing contracts and retained enabled selections, including unavailable selections. `describe` with Agent access projects all Ports. A conforming `runtime.guide` is an ordinary query with a closed empty-object request and a closed response requiring a finite-length `markdown` string; local references are resolved. Invalid guide shapes produce author warnings and stay ordinary declared composition.

`{method:"enable",contract,major}` returns `{contract,major,enabled:true}`. `{method:"disable",contract}` returns `{contract,enabled:false}`. Both require the caller's current binding. Defaults are empty, with no inferred grant or historical migration. Selections pin a contract major under Installation, Workspace, and harness Package Identity, not a Provider or Agent identity. They survive restart, rebind, unbind, and reinstall under the same identities. Enable requires an admitted conforming Provider of that major but does not start it. Disable may remove an unavailable saved selection. Settings are self-scoped, idempotent, and published only after durable success.

A bound Agent's new access to a guide-bearing contract requires enablement in **both** views. Declared no-guide composition remains supported. Phase 4 must qualify disable and binding-loss revocation without disturbing unrelated subscribers. Reconnection is explicit; already-authorized commands preserve their original actor and uncertainty. Registration and metadata changes take effect on the next Host open, not through a future live registration manager.

When a conforming current Provider exists, summary `major`, `minor`, description and digest identify that offered version. `enabled` means a saved selection exists; `enabledMajor` is its pinned major. For an offered major 2 with saved major 1, `enabled:true`, `enabledMajor:1` and `available:false` do not imply an upgrade. Complete current metadata remains discoverable under its own digest while the Provider is metadata-available. Only explicit enable changes the selection. A missing or nonconforming Provider yields a retained-only unavailable row without current metadata. Discovery and describe never write settings or start Providers.

### Invocation, subscriptions, and errors

Calls use exactly `{method:"invoke",access,contract,major,port,kind:"command"|"query",request}`. A declared Port must match its actual declaration and selected major. Live queries open `capability.stream` with exactly `{grant,access,contract,major,port,kind:"query",request}`. Event subscriptions use `{grant,access,contract,major,port,kind:"event"}`. No request accepts actor context. Request/response and event validation, permits, fan-out, and command uncertainty use the same broker, not a version adapter.

For non-object Port responses or events, or an object equal to the reserved transport shape, the Host uses `{"$teamarnessPortEnvelope":1,"$teamarnessPortValue":<schema value>}`. Other objects remain unwrapped on the capability route. `PortClient` returns the original value. Provider and capability majors are independent.

Stable errors include `capability_request_invalid` (malformed or extra fields), `ungranted_port` (inaccessible contract/name/kind), `port_version_mismatch` (wrong declared major), `port_unavailable`, `port_catalog_changed`, `port_not_enabled`, `agent_not_bound`, and the final revocation code `agent_binding_changed`. `port_request_invalid`, `port_response_invalid`, `port_event_invalid`, `provider_error`, `provider_exited`, `overloaded`, and `outcome_unknown` retain broker semantics. The schema lists the complete vocabulary. Settings and subscription revocation are implemented and qualified through Host lifecycle tests. Disable, unbind or Agent removal, changed binding, and Provider loss terminate affected streams with `port_not_enabled`, `agent_not_bound`, `agent_binding_changed`, and `provider_exited`, respectively. Accepted bounded events may drain before the terminal, but none enqueue after revocation. Reconnect explicitly after recovery; no stream or command is replayed. Generic-client and external UI qualification remain Phase 5 work. Storage failures use `capability_unavailable`; they do not claim a durable success. Reread discovery after a failed acknowledgement.

An event subscription is distinct from a live query: explicitly open `capability.stream` with exactly `{grant,access,contract,major,port,kind:"event"}` (no request or cursor). The Host opens one Provider upstream for the first subscriber to that Workspace/event Port, validates each event once, and sends ordered live facts to each independently credit-controlled Consumer stream. A declaration never opens a subscription or wakes an inactive Consumer. Late or reconnected subscribers receive no history; use a Provider query or cursor for recovery. Conversations' `messages.changed` is an empty-object, coalescible invalidation; its `message.page` query recovers current state without marking the Channel read. When the last subscriber leaves, the Host closes the upstream. Each subscriber has four queued events (at most four maximum-frame payloads); the Host permits at most 16 subscribers and eight event upstreams globally, at most four subscribers and four upstreams per Provider Instance, and at most 4 MiB / 2 MiB of queued event bytes globally / per Provider Instance. These budgets derive from Package Process 1.0 frame, writer-byte, in-flight and credit constants and remain separate from UI/lifecycle traffic. A slow subscriber ends with `port_subscriber_overflow` **after** its accepted queued events; shared byte exhaustion ends only subscriptions that cannot reserve space with `overloaded`. Invalid Provider event schema ends the affected event streams with `port_event_invalid`, never the process session. Provider exit/replacement revokes old streams (`provider_exited`); reconnection is explicit and does not replay events. The SDK reports inbound `event_lagged` on its one affected stream and keeps other streams and calls usable.

A command rejected before the Provider writer accepts its frame is certain. After acceptance, timeout, exit, cancellation race or an invalid response can conceal a mutation: the Host reports `outcomeUncertain:true` and `retryable:false`. A timeout, cancellation race or Provider exit after dispatch reports `outcome_unknown`; an invalid response reports `port_response_invalid`. A Provider-owned post-commit error can report `provider_error` with the same uncertainty flags. The Host never automatically replays a command. Queries may be retryable but are not automatically replayed. A live query is one-to-one, credit-controlled and not an event subscription. The Provider receives typed Host-authored actor context, read from the caller's Agent Binding at each command and live-query dispatch: `{kind:"agent",principal:<64-hex Principal ID>,identity:<Agent signing key>}` when the calling Package Instance is bound through `teamarness.agent-binding`, otherwise `{kind:"person",identity:<Installation public identity>}`. An event subscription's shared upstream carries the Person actor. Callers cannot submit their own actor context; a request that carries `actor` or a Workspace selector is `capability_request_invalid`. A Provider that acts for an Agent passes `principal` to a record-log 1.1 `append`, and Core signs under the standing rule below (ADR 0030). A Provider that serves only Persons refuses the Agent kind with its own error code.

## `teamarness.workspace-context` 1.0

`get` takes no arguments. It returns `workspaceId`, `workspaceName`, `installationIdentity`, and `installationDisplayName`.

## `teamarness.membership` 1.0

`current` returns the canonical member list, durable decimal-string epoch, and content-derived membership version. `was_member` accepts an installation identity and signed decimal-string Unix millisecond. Its result carries `informational_not_authorization` because historical membership cannot authorize a new write.

A Package opens `capability.stream` with its membership grant, method `subscribe`, and `afterEpoch`. Transition events contain the Core epoch, membership version, cause operation ID, and snapshot digest. Stream credit bounds delivery. The host drops its Core subscription when credit or writer capacity runs out and reopens after the last delivered epoch. Core transitions also know the admitted or revoked Installation and the author; the 1.0 event does not carry them. A later minor version will.

## `teamarness.principals` 1.0

This Package-neutral presentation capability is scoped by its grant to one Workspace. It delivers Workspace Principals: the Workspace Persons and Workspace Agents whose keys Canonical Membership authorizes. It replaces the names capability of ADR 0022, which no longer exists (ADR 0023).

`current` returns `{workspaceId, cursor, principals}`. Each Principal row contains an opaque `principalId`, `kind` (`person` or `agent`), `stewardPrincipalId` (the Steward's Principal ID for an agent, `null` for a person), `keys` (the Principal's current signing keys as 64-hex identities; exactly one in version 1), nullable `name`, and a fingerprint (the first 12 hex characters of its current signing key). `lookup` accepts exactly one of `identity` (a current member key) or `principalId` and returns `{principal}` or `{principal:null}` when no current Principal matches; a missing name is not a membership error. The current roster comes from Core, not from the Package. Names are non-unique, bounded Unicode text; a Person names itself and a Steward names its Agents. Authorization remains in `teamarness.membership` 1.0, which is unchanged: an Agent appears there as a Member with role `Member`, and a Package that needs kind or steward resolves them here.

Attribution is the record author resolved to its Principal: a Package looks up the signer key and reads `kind` and `stewardPrincipalId` from the row. No request or record carries an acting-Principal claim.

A `capability.stream` with method `subscribe` and decimal-string `afterCursor` emits the latest bounded snapshot when the independent durable profile generation changes. The cursor is a coalescing invalidation token, **not** a replay ledger, membership epoch or immutable-record cursor: subscribe first, read `current`, and refetch after a gap or restart. A cursor ahead of Core's profile generation closes with `cursor_invalid`. Each event consumes stream credit. The closed schema and limits are in `principals.schema.json`.

## `teamarness.agent-binding` 1.0

A Package Instance's own Agent Binding: Installation-local configuration, held by Core and never replicated, that names the Workspace Agent this Package acts as on Ports and in record-log 1.1 appends (ADR 0030). The grant fixes the calling Package and Workspace, so no request names a Package and a Package can never read or change another Package's binding. Only the Steward's Installation can bind: Core requires the Agent's Steward key group to contain this Installation's key.

`current` takes no arguments and returns `{agent}`: `null` when unbound or when the bound Agent is no longer a current Workspace Agent, otherwise `{principalId, identity, name}` with the Agent's 64-hex Principal ID, its current signing key and its nullable name. `candidates` takes no arguments and returns `{agents:[{principalId, identity, name, bound}]}`, at most 1,024 rows: the Workspace Agents this Installation stewards, each marked with whether this Package Instance is bound to it. Other Persons' Agents are never listed. `bind` takes exactly `{principalId}`, replaces this Package Instance's previous binding, and returns the same `{agent}` shape as `current`. `unbind` takes no arguments, is idempotent, and returns `{agent:null}`.

`bind` fails with `agent_not_stewarded` when this Installation does not steward the Principal (including a Person or another Steward's Agent) and with `agent_not_found` when no current Workspace Principal has that ID. A malformed ID or any extra field is `capability_request_invalid`. A binding is cleared when this Installation removes the Agent; a Package observes removal or rename through `teamarness.principals`. The closed schema is `agent-binding.schema.json`.

## `teamarness.content-views` 1.0

A Package serves files from its own Package Data Directory to a nested frame without the Host or the Shell knowing what the files are. The Package states *what* to serve; the Host owns path rules, source resolution, verification, media types, token lifetime and bounds. The grant fixes the Package, Workspace and process generation, so no request names a Package or a directory, and a view ends with the generation. Declaring this family is also the Package's opt-in to nesting content: the Host reports `nestedContent: true` on the declaring Package's opened UI mounts, and every other Package's mounts report `false`.

`open` takes exactly `{entry, files}`. `files` lists 1–1,000 entries of exactly `{path, sha256, size, source}`:

- `path` is the content path inside the view: at most 128 bytes, non-empty `/`-separated segments, none of them `.` or `..`, and no `\`, NUL, `%`, `?`, `#` or control characters. Paths are unique.
- The extension of the last segment chooses the media type from the shared allowlist below. Extensions match **lowercase only**: `image.PNG` is refused.
- `sha256` is the lowercase hexadecimal SHA-256 of the file's bytes, and `size` is its byte length, at most 16,777,216 (16 MiB). The declared sizes total at most 67,108,864 bytes (64 MiB).
- `source` 1.0 is only `{kind:"file", path}`: a relative path of at most 256 bytes inside the caller's Package Data Directory. The Host refuses a symlink anywhere on the path, a missing file, a file that is not regular, and a path outside the directory.
- `entry` is one of the listed paths.

It returns `{view, url}`. `view` is an opaque token of 128 random bits as 32 lowercase hexadecimal characters. `url` is `teamarness-content://localhost/<view>/<entry>`, with every entry byte outside the URL unreserved characters and `/` percent-encoded. Relative references inside the content resolve against it.

`close` takes exactly `{view}` and returns `{closed}`: `false` when the view had already ended or belongs to another generation. A generation holds at most 32 open views; opening a 33rd closes the least recently opened one. Every view of an Instance closes when its process generation ends.

The Host serves a view itself; no Package request happens per file. The first read of each file in a view checks its size and SHA-256 and caches the verdict for that view; every read checks its size and that it is still a regular file. A file that changed after `open` is no longer served, so a Package that changes a file opens a new view. The media type always comes from the allowlist, never from the Package. An unknown or ended view, an unlisted path and a file that fails verification are all the same not-found answer.

| Extension | Media type | Extension | Media type |
|---|---|---|---|
| `html` | `text/html; charset=utf-8` | `ico` | `image/x-icon` |
| `css` | `text/css; charset=utf-8` | `woff` | `font/woff` |
| `js`, `mjs` | `text/javascript; charset=utf-8` | `woff2` | `font/woff2` |
| `json` | `application/json` | `ttf` | `font/ttf` |
| `svg` | `image/svg+xml` | `otf` | `font/otf` |
| `png` | `image/png` | `wasm` | `application/wasm` |
| `jpg` | `image/jpeg` | `txt` | `text/plain; charset=utf-8` |
| `gif` | `image/gif` | `md` | `text/markdown; charset=utf-8` |
| `webp` | `image/webp` | `csv` | `text/csv; charset=utf-8` |
| `avif` | `image/avif` | `mp4` | `video/mp4` |
| `webm` | `video/webm` | `mp3` | `audio/mpeg` |

The allowlist, the bounds and the path rules are published once in Rust as `teamarness_package_contract::media_types`, so a Package that stores content can refuse a file with the verdict the Host would give.

`open` fails with `unsupported_media_type` for an extension outside the allowlist, `content_too_large` for more than 1,000 files, a declared total over 64 MiB or a file over 16 MiB, and `invalid_content_path` for a malformed or duplicate path, an entry that is not listed, or a source the Host refuses. Any other malformed field or extra field is `capability_request_invalid`. The closed schema is `content-views.schema.json`.

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
only when the Agent is a current Workspace Agent, this Installation stewards
it, and at least one Package Instance on this Installation holds an Agent
Binding to it (`teamarness.agent-binding`). The calling Package need not be the
bound one: a bound Agent is a local signer for every Package Instance on this
Installation, which is how a Provider appends for the Agent actor the Host
derived from its caller (ADR 0030). Any other `principal`, including a Person,
an Agent with no binding on this Installation, an Agent another Installation
stewards or an unknown ID, fails with the error code `principal_not_bound` and
commits nothing. Bindings are local Installation state and are never
replicated; one Package Instance is bound to at most one Agent, and any number
of Packages may bind the same Agent. The Agent's key never crosses the
capability boundary. Peers accept the record because the Agent's key is in
membership history, and a reader resolves the author to its Principal through
`teamarness.principals`.

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
