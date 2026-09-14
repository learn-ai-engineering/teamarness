# teamarness-package-protocol

This crate is the Rust adapter for the language-neutral Package Process Protocol. The normative prose, JSON Schemas, golden sequences, and malformed fixtures live in [`contracts/package-process/v1`](../../contracts/package-process/v1/README.md). Rust types do not define interoperability.

The adapter provides strict frame encoding and decoding, protocol state checks, and major and minor version negotiation. Each frame uses a four-byte unsigned big-endian length followed by UTF-8 JSON. The maximum JSON payload is 1,048,576 bytes. The decoder rejects zero or oversized lengths, invalid UTF-8, malformed JSON, duplicate keys, unknown frame kinds, and invalid protocol state.

Host and Package request IDs use separate namespaces. Wide integers cross as decimal strings and bytes use unpadded base64url. Requests may complete out of order. Event streams use ordered sequences and explicit credit.

The crate has only `base64`, `serde`, and `serde_json` as direct dependencies. It contains no host policy, Core Runtime type, process launcher, Tauri adapter, Package SDK, or product operation.

Run its contract suite from the repository root:

```sh
cargo test -p teamarness-package-protocol --locked
node --test contracts/package-process/v1/fixtures/*.test.mjs
```
