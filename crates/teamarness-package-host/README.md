# teamarness-package-host

`teamarness-package-host` admits Package Sources, allocates durable Package Data Directories, supervises one Package Process per Package Instance, routes scoped Semantic Capabilities, and owns Package UI mount leases. Tauri and Package-specific behavior stay outside the crate.

The public facade has four entry points:

- `PackageHost::open` validates application and installation registrations, admits each source independently, reconciles process leases, and eagerly activates admitted Packages for known Workspaces.
- `PackageHost::snapshot` returns generic catalog, diagnostic, instance, lifecycle, source, trust, and revision values.
- `PackageHost::execute` accepts the closed `PackageHostCommand` lifecycle, mount, request, stream, resource, reconciliation, and shutdown vocabulary.
- `PackageHost::subscribe` returns coalesced host-state notifications or one claimed bounded Package UI event route.

Private modules own manifest parsing, admission, host metadata, data-directory allocation, process protocol, supervision, Core adapters, and UI contract validation. Public values do not expose source paths, cache paths, Package data paths, process handles, Core Runtime values, secret bytes, or transport handles.

A Package Instance is one Package Identity and Workspace in the opened Installation. UI visibility never controls Backend lifecycle. Closing or failing a mount revokes its requests, streams, and resource token without stopping a healthy Backend. Process generation changes revoke old mount work without replay.

The host validates UI requests, responses, subscription requests, and events against validators compiled during admission. It resolves only exact admitted files through mount-scoped opaque tokens. Package event routes hold at most 32 events and 8 MiB, and credit returns only after the desktop acknowledges delivered events. The generic `teamarness.synchronization.immutable-record-log` capability routes Core's committed `append`, cursor-safe `subscribe`, and bounded `open` methods without exposing Core, transport, or key-custody types. Base64 inputs are decoded and bounded before Core calls; streams reopen from the last delivered cursor.

Package Processes remain trusted native code with the current user's ambient filesystem, network, and process authority. Supervision is a recovery boundary, not a sandbox. It handles ordinary exits, deadlines, bounded restart, graceful shutdown, process-group reaping, and conservative startup orphan reconciliation.

Run the host and full failure matrices from the repository root:

```sh
cargo test -p teamarness-package-host --locked
cargo test --workspace --locked
```
