# Package Process Protocol version 1

This directory is the normative language-neutral contract between the Package Host and a Package Process. Rust types are only one adapter. A process can conform without linking Teamarness code.

## Transport

The host writes frames to the process's standard input. The process writes frames to standard output. Standard error is diagnostic text and must never contain protocol frames.

Each frame is a four-byte unsigned big-endian payload length followed by one UTF-8 RFC 8259 JSON object. The payload limit is 1,048,576 bytes. A zero length, oversized length, truncated payload, invalid UTF-8, malformed JSON, duplicate object key, or unknown `kind` is fatal to that process generation.

JSON integers that may exceed JavaScript's safe integer range are decimal strings matching `0|[1-9][0-9]*`. Protocol byte strings use RFC 4648 base64url without padding. IDs are opaque within their declared namespace: host IDs start with `h:` and Package IDs start with `p:`.

## Version and limits

Version 1 selects one exact major and the highest minor supported by both peers. Version 1.0 fixes these limits:

| Limit | Value |
|---|---:|
| Payload bytes per frame | 1,048,576 |
| In-flight requests in each direction | 32 |
| Queued frames per writer | 64 |
| Queued payload bytes per writer | 8,388,608 |
| Initial event credit per stream | 32 |

A minor version may add optional fields or operations without changing existing meaning. Unknown frame kinds and incompatible majors are fatal. Receivers ignore unknown object fields only where the schema explicitly allows them.

## Handshake

The first host frame is `initialize`. It binds the launch nonce, Package identity and digests, process generation, Workspace and Installation, Package Data Directory, selected protocol version, negotiated opaque capability grants, and active limits. The first Package frame is `initialized`; it echoes the binding fields and UI contract digest. A mismatch fails the generation.

After initialization, the host sends a `request` whose operation is `lifecycle.activate`, or `lifecycle.recover` for a replacement generation. Lifecycle requests are serialized barriers. New mount-scoped UI requests cannot enter while a lifecycle request is unresolved.

The launch nonce and capability grants are non-secret opaque base64url values. They bind traffic to one admitted process generation but do not authenticate against a hostile local process.

## Frame verdicts

Every frame gets one of three verdicts, and the same classifier produces them on both sides.

| Verdict | When | What the receiver does |
|---|---|---|
| Deliver | The frame names state the receiver still holds. | Act on it. |
| Ignore | The frame is well formed and would have been legal when the sender sent it; the receiver has since retired that request or closed that stream. | Drop it. Do not deliver it and do not fail. |
| Fatal | No conforming peer that had seen everything this receiver sent could have sent this frame. | Fail the process generation. |

A receiver retires a request or a stream at the moment it stops being willing to serve it, not when a later grace period expires, and keeps a bounded tombstone for its ID. Tombstones are bounded by count, not by clock: 64 IDs per direction for answered requests, 64 for abandoned requests, and 64 for closed streams, evicted oldest first. A frame naming an ID that no longer has a tombstone is out of contract, because the peer exceeded the window the contract grants.

A sender never withholds a frame it owes. Exactly one `response` or `error` still reaches the wire for every request, even one the peer cancelled, and the peer applies these verdicts to what arrives.

Ignore never widens a budget. Events beyond the credit window, a sequence gap on a live stream, credit outside `1..=32` or above the window a stream was opened with, a 33rd in-flight request, a second terminal frame, and anything naming state that never existed are all fatal.

## Requests, terminal frames, and cancellation

`request` carries an origin-scoped `id`, an operation, an optional opaque capability `grant`, optional `mountId`, a deadline in Unix milliseconds, and an object payload. Package-originated capability calls use a `p:` ID and a grant. Host lifecycle and UI requests use an `h:` ID. Mount traffic always carries `mountId`.

Exactly one `response` or `error` is terminal for a request. Responses may finish out of request order. An unknown response or second terminal frame is fatal. The receiver keeps bounded tombstones for cancelled and timed-out IDs, so a late first terminal frame can be ignored without accepting a second one. A receiver enters that tombstone when it sends `cancel` or abandons a deadline, not when a grace period later expires.

`cancel` names the original request ID. Cancellation is best effort. A request deadline causes cancellation; failure to stop within 500 milliseconds terminates the generation. A state-changing request can then report `outcome_uncertain` and is never replayed automatically.

`error` contains a stable host or Package-owned code, retryability, an optional bounded details object, and an optional `outcomeUncertain` flag. Operating-system error text and stderr do not cross this frame. A Host capability error with code `overloaded` or `deadline_exceeded` is retryable. A state-changing capability call that exceeded its deadline after starting reports `outcomeUncertain`.

A Package's `error` answer to a host request is forwarded, not replaced. The host keeps its own authoritative code for what it did with the failure and carries the Package's code, retryability and bounded details beside it, so a Package UI reads both. Details are bounded rather than truncated: a details object over the limit is dropped whole, and the code and retryability still cross.

## Provider Port operations (1.0 frames)

The Host delivers a declared command as an unmounted `request` with operation `teamarness.port.command.v1`, and an ordinary query with `teamarness.port.query.v1`. A live query uses an unmounted Host-originated `stream_open` with topic `teamarness.port.query.v1` and the normal 32-event credit window. The first explicit event subscription opens an unmounted Host-originated `stream_open` with topic `teamarness.port.event.v1` (one shared upstream per Workspace/event Port); the Provider's ordinary `event`/`stream_close` frames carry live facts to the Host. Later subscribers share this stream and the Host closes it after the last leaves. All carry a closed payload `{contract,major,minor,port,actor,request}`: `actor` is Host-authored and has two kinds, `{kind:"person",identity:<Installation public identity>}` for an Instance with no Agent Binding and `{kind:"agent",principal:<64-hex Principal ID>,identity:<Agent signing key>}` for one bound through `teamarness.agent-binding` (ADR 0030); the event-subscription upstream always carries the Person. A Provider that cannot serve the Agent kind rejects that frame with its own error code. `contract` identifies the Port Contract, not a Provider Package; `major`/`minor` are the negotiated version, `port` is the exact dotted name. A Provider responds with the schema-conforming `response` or, for a live query, ordered schema-conforming `event` frames and `stream_close`. No new frame kind or mount is involved. The caller side uses the generation-scoped `teamarness.package-ports` 2.0 capability documented in `contracts/package-capabilities/v1/README.md`; a routing envelope cannot supply a target Workspace, Package or actor. The Host may relate the existing process-scoped Consumer `p:` request or stream ID to the Provider `h:` ID in a bounded local observation receiver. The Provider SDK may observe its `h:` ID at dispatch. These diagnostic IDs do not add a protocol field or an authority claim, and two Consumers can use the same `p:` counter in different process scopes.

The Port Contract `major` is independent of Package Process Protocol 1.0; a compatible declared major greater than 1 uses the same frame operations. Process `response` and `event` payloads must be objects. A Port request value can use any JSON root because it is nested under `request` in the Provider envelope. Encode a Port value that is not an object, or is itself exactly the reserved two-field shape, as `{"$teamarnessPortEnvelope":1,"$teamarnessPortValue":<schema value>}`. The Rust Provider SDK wraps every Port response and event at its router, including objects, so an envelope-shaped object is unambiguous. A direct process adapter may send an ordinary object unwrapped, but must wrap an object equal to the reserved shape. The Host validates the logical value against the Port schema and preserves an event's transport object when forwarding it. Host-to-Consumer capability frames likewise wrap non-objects and the reserved-shape object. A Consumer unwraps one layer only. The shape is transport, never an actor or grant claim. `EventSender::send_port_value` accepts non-object Port events; `send` still requires object UI events. A Port event may contain a logical 256 KiB value plus the fixed envelope overhead within the separate wire and queue bounds. No process frame rule changes.

The Host admits at most 8 simultaneous Port routes to one Provider generation, below the published 32 Host-to-Package request/SDK-stream slots; aggregate route and byte permits are reserved before dispatch, separately from UI/lifecycle traffic. The Host's aggregate byte bound holds four maximum request/response pairs (8 MiB), and each live query holds its permits until its stream ends. Rejection is `overloaded`, not a session-fatal frame classifier verdict. `ContractState` remains the only wire frame classifier. A writer-accepted command can have an uncertain outcome; the Host never retries it. Event fan-out has independent global/per-Provider stream, subscriber, event-count and queued-byte limits derived from the process frame, writer and credit constants; see `contracts/package-capabilities/v1/README.md`. No event stream uses a UI mount. Its Host-to-Consumer output remains within normal stream credit, and each bounded subscriber queue accepts ordered validated events without waiting for peer credit. Overflow ends only that subscriber with `port_subscriber_overflow` after already accepted events; a shared byte shortage returns stream-local `overloaded`. A schema-invalid event closes its affected stream with `port_event_invalid`, and old streams are revoked on Provider replacement. The SDK's inbound `event_lagged` also retires only its affected stream, not the process session.

## Streams and credit

`stream_open` creates one origin-scoped stream and grants 32 events initially. `event` carries that stream ID, an increasing decimal-string sequence, and an object payload. The sender must not send without credit. `credit` adds a positive bounded amount only after the receiving adapter has accepted preceding work, and never more than that adapter has accepted: the 32 events are a ceiling on the window, not a starting balance, so a grant that would carry a stream past it is `credit_violation`. `stream_close` closes either direction and can include a stable reason code.

The reason code on a Package's `stream_close` says who owns the ending. No code, `completed` or `closed` is an ordinary completion. `cancelled` and `overloaded` are host codes. Every other code is the Package's own termination code and is forwarded to the subscriber as a Package-owned error, with `retryable` false and no details, because `stream_close` carries neither. A completion and a Package-owned termination both end that one stream and nothing else; only a Teamarness-owned terminal stream error causes Package UI Mount recovery.

A receiver that has observed `stream_close` for a stream, in either direction, keeps a bounded tombstone for its ID and ignores a later `event`, `credit` or `stream_close` naming it. Credit released for events already accepted, and events already written before the close was observed, are ordinary in-flight traffic and are not protocol violations. A frame naming a stream ID that was never opened remains fatal, and a `stream_open` reusing a tombstoned ID is `duplicate_id`.

A sender keeps `stream_close` behind the events of the same stream, so a close never overtakes the events it ends.

Sequence values increase by one within a stream. Streams have no ordering relationship to requests or other streams. Mount-scoped streams carry `mountId` and stop when that mount closes or its process generation changes.

## Health and shutdown

After 30 seconds without valid traffic, the host sends `ping`; the Package answers the same nonce with `pong` within five seconds. Health traffic uses reserved writer capacity.

For application shutdown the host sends `shutdown`, waits up to three seconds for `shutdown_ack`, then terminates and reaps the process if needed. A conforming process also exits when standard input reaches EOF. Protocol shutdown is not the durable `lifecycle.dispose` operation.

## Lifecycle deadlines

| Operation | Deadline |
|---|---:|
| Handshake | 5 seconds |
| Activate or recover | 30 seconds |
| Suspend or dispose | 10 seconds |
| UI request | 30 seconds by default, capped at 60 seconds |
| Capability call | 10 seconds |
| Idle health probe | 5 seconds after 30 idle seconds |
| Cancellation grace | 500 milliseconds |
| Protocol shutdown | 3 seconds |

## Contract files

`schemas/frame.schema.json` dispatches to the schema for every frame kind. The other schema files expose each frame definition directly. `golden/*.jsonl` are valid bidirectional sequences, including the lifecycle races the verdict table tolerates. `invalid/*.json` are malformed encoding or state-machine cases with one expected stable error code. Both the Rust contract test and direct Node test enumerate every file, so adding a fixture without coverage fails the suites.
