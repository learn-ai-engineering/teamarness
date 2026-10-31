# Conversations Package

Conversations owns Channels, immutable Markdown Messages, and their durable local projection. The Backend uses the generic immutable-record log for signing, protection, and synchronization. The Package UI is unchanged by runtime guidance.

## Public Port Contract 1.3

[`ports.json`](ports.json) declares `dev.teamarness.conversations` 1.3. It adds the ordinary read-only `runtime.guide` query to the five existing operations without changing their kinds or schemas:

- `runtime.guide`: closed empty request `{}`; closed response `{markdown: string}` with a 32,768-character schema bound. The actual encoded response, including its process envelope, is tested below 32 KiB.
- `channel.list`: at most 64 visible Channels, with `#general` first. Hidden Channels are excluded. There is no continuation.
- `message.page`: bounded reads with `epoch`, `revision`, pending rows, and `nextBefore`. The schema permits a limit of 1 to 64. The Backend caps each read at 32 raw rows and can shorten it further for the encoded-byte budget.
- `message.changes`: at most 32 revision-ordered changes, with `continuation`, captured `highWater`, and explicit `resyncRequired` on invalid continuity.
- `message.post`: 1 byte to 32 KiB of UTF-8 Markdown, signed as the Host-derived Installation Person or Workspace Agent. JSON Schema character counts do not replace the byte limit.
- `messages.changed`: empty, coalescible, live-only Workspace invalidation, with no initial event or replay.

The runtime guide is generated from enforced Backend constants. It explains subscribe-before-read, acknowledged prefixes, pending and unavailable content, changed-epoch reconstruction, Host-derived authorship, Provider errors, and read repeatability. Port reads do not mark Channels read. A post can take effect even when its result is uncertain. Never retry an uncertain post automatically.

The [Port declaration reference](../../contracts/package-ports/v1/README.md) and [Package author guide](../../docs/package-author-guide.md#consume-a-package-port) describe generic consumption. A declared consumer can retrieve guide metadata without a binding. Bound-Agent access now defaults to `port_not_enabled`, including declared Ports, because the contract has a conforming guide. The Host implements durable enablement, generic Agent discovery and lifecycle revocation, with scripted repository coverage including Conversations. Independent Verify and external harness provenance, UI observations and model quality remain separate qualification.

## Build and validation

From the repository root:

```sh
npm run build:conversations
cargo test -p teamarness-conversations-package --locked
cargo test -p teamarness-package-host --all-features --locked --test package_ports_metadata
```

The normal build generates the staged Package Manifest's sizes and digests. Source manifest placeholders are not edited by hand. Backend tests invoke the actual public Port router, validate the guide against its admitted schema, pin recovery advice and limits, and compare the existing five operation shapes with their pre-guide structural fingerprint. Existing historical characterization tests remain in place.
