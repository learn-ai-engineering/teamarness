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
