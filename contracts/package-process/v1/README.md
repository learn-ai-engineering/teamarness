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

## Requests, terminal frames, and cancellation

`request` carries an origin-scoped `id`, an operation, an optional opaque capability `grant`, optional `mountId`, a deadline in Unix milliseconds, and an object payload. Package-originated capability calls use a `p:` ID and a grant. Host lifecycle and UI requests use an `h:` ID. Mount traffic always carries `mountId`.

Exactly one `response` or `error` is terminal for a request. Responses may finish out of request order. An unknown response or second terminal frame is fatal. The receiver keeps bounded tombstones for cancelled and timed-out IDs, so a late first terminal frame can be ignored without accepting a second one.

`cancel` names the original request ID. Cancellation is best effort. A request deadline causes cancellation; failure to stop within 500 milliseconds terminates the generation. A state-changing request can then report `outcome_uncertain` and is never replayed automatically.

`error` contains a stable host or Package-owned code, retryability, an optional bounded details object, and an optional `outcomeUncertain` flag. Operating-system error text and stderr do not cross this frame.

## Streams and credit

`stream_open` creates one origin-scoped stream and grants 32 events initially. `event` carries that stream ID, an increasing decimal-string sequence, and an object payload. The sender must not send without credit. `credit` adds a positive bounded amount only after the receiving adapter has accepted preceding work. `stream_close` closes either direction and can include a stable reason code.

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

`schemas/frame.schema.json` dispatches to the schema for every frame kind. The other schema files expose each frame definition directly. `golden/*.jsonl` are valid bidirectional sequences. `invalid/*.json` are malformed encoding or state-machine cases with one expected stable error code. Both the Rust contract test and direct Node test enumerate every file, so adding a fixture without coverage fails the suites.
