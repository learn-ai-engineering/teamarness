# Package author guide

A Teamarness Package is a directory of files, not a Rust library or a dynamic ABI. Build the complete Package Directory in your own repository, test it as an installation-local Package, then give the same declared bytes to the Teamarness application build. For a supported starter project, run `npm run create:package -- --output <directory> --identity <reverse-dns-id> --display-name <name>` from a Teamarness checkout. The generator creates a foundation outside the Teamarness workspaces and vendors the current Rust and TypeScript SDK source with exact lockfiles. Generated SDK copies have no supported in-place upgrade yet.

Phase 3 supports `aarch64-apple-darwin`. It ships a Rust Backend SDK and a TypeScript UI SDK. The Package Process Protocol is language-neutral, but no other production Backend SDK is supported yet.

## Package Directory

The directory root contains `teamarness.package.json`. The manifest points to one target Backend, one ESM UI entrypoint, optional UI assets, and one UI contract. The reference Package has this shape:

```text
teamarness.package.json
backend/
  teamarness-reference-package-aarch64-apple-darwin
ui/
  index.js
  contract.json
  probe.svg
```

You own executable packaging. Ship a self-contained executable for every declared target. Teamarness does not install Node, Python, shared libraries, language runtimes, or Package dependencies. It starts the exact admitted Backend path without a command shell and uses the Package Data Directory as its working directory.

Do not use symlinks. Every manifest path must be normalized relative UTF-8 with no empty, `.`, `..`, absolute, or backslash segment. Compute each declared byte size and lowercase SHA-256 digest after the final build. Any later byte change invalidates admission until Teamarness restarts and admits the directory again.

## Package Manifest

The normative schema is [`contracts/packages/v1/schemas/manifest.schema.json`](../contracts/packages/v1/schemas/manifest.schema.json). Admission also applies stricter semantic checks described in [`contracts/packages/v1/README.md`](../contracts/packages/v1/README.md).

A version 1 manifest declares:

- `identity`, a lowercase reverse-DNS Package Identity;
- a strict Semantic Version in `version`;
- `presentation.displayName`;
- one Package Process Protocol major and minor range;
- Backend files with exact target, path, size, and digest;
- the ESM entrypoint, assets, and UI contract;
- every required versioned Semantic Capability;
- optional Package-owned metadata under reverse-DNS `extensions` keys.

Source, trust, override policy, installation paths, and data paths do not belong in the manifest. Keep registration metadata outside the Package Directory.

The current application selects exactly one `aarch64-apple-darwin` Backend. The Backend executable implements [`contracts/package-process/v1`](../contracts/package-process/v1/README.md). It must keep reading control frames while work runs, honor lifecycle requests, and exit when stdin closes. Rust authors can use [`teamarness-package-sdk`](../crates/teamarness-package-sdk/README.md).

## UI contract and module

The UI contract is a closed JSON document:

```json
{
  "schemaVersion": 1,
  "$defs": {},
  "requests": {
    "example.read": {
      "request": { "type": "object", "additionalProperties": false },
      "response": { "type": "object" }
    }
  },
  "subscriptions": {
    "example.changes": {
      "request": { "type": "object", "additionalProperties": false },
      "event": { "type": "object" }
    }
  }
}
```

Every request has separate request and response schemas. Every subscription has separate request and event schemas. Operation names use lowercase dotted segments. Only local `#/$defs/...` references are accepted. Remote references, dynamic references, cycles, unknown descriptor fields, schemas deeper than 64 levels, and contracts over 16,384 schema nodes fail admission.

The contract file may use at most 1 MiB. Each generic request, response, subscription request, or event may use at most 256 KiB of encoded JSON and 64 levels. A contract may declare at most 128 requests, 128 subscriptions, and 256 operations total.

Build one UTF-8 ESM entrypoint of at most 8 MiB. Declare at most 64 static assets, each at most 8 MiB. The entrypoint, assets, and contract may total at most 32 MiB. Do not emit undeclared chunks or remote imports.

Export `mount(context)`. The context provides a fresh Shadow Root, Package and Workspace presentation, theme token names, the UI host version, and a mount-scoped typed client. Return an idempotent unmount function. Use `@teamarness/package-ui-sdk` for requests, cancellation, ordered subscriptions, and post-listener acknowledgement.

The Shell loads admitted resources through `teamarness-package://`. The loader accepts only `GET` and `HEAD` for exact admitted paths. It rejects traversal, encoded separators, query selectors, stale tokens, undeclared files, changed cache bytes, and MIME changes. Responses disable sniffing and storage. Package code never receives the mount token or a source, cache, or data path.

## Capabilities and local data

Declare only the capabilities you call. Phase 3 provides Workspace context, canonical membership, historical membership, membership transitions, and domain-separated signing. Grants bind one Package Identity, Workspace, Installation, and process generation. A Package cannot select another scope. Private keys, Core database handles, Core types, raw secret bytes, and Iroh handles never cross this interface.

The initialization frame grants an absolute Package Data Directory. Your Backend owns that directory's files, schema, and migration policy. The directory remains bound to Installation, Workspace, and Package Identity across Package source changes, versions, process generations, disposal, and application restarts. Teamarness does not inspect, migrate, back up, or delete Package-owned data.

## Trust limits

A Package Backend is trusted native code. It has the current user's ambient filesystem, network, and process authority. Process supervision handles ordinary exits, deadlines, graceful shutdown, and conservative orphan cleanup. It is not a sandbox and does not contain hostile code or deliberately evasive descendants.

A same-webview Package UI can access the Shell DOM and any Tauri command it knows. A Shadow Root limits style collisions only. It does not reduce DOM, JavaScript, network, or Tauri-command authority. Review both Backend and UI code before trusting a Package.

## Bundled and local Package Source kinds

`packages/tutorial/` is checked-in Tutorial source. The application builder turns it into the bundled `dev.teamarness.tutorial` Package Directory under `packages/dev.teamarness.tutorial/`. A generated learner project such as `dev.teamarness.emojart` remains installation-local and is admitted through the owner-only registration file. Package Source values `bundled` and `local` remain canonical technical source data; the Shell presents them as `Team` and `Personal`. `Team` and `Personal` are UI aliases, not trust verdicts, and do not define registration scope or imply Workspace enablement, synchronization, sharing, networking, or collaboration.

## Local development loop

1. Build the final Backend executable, ESM entrypoint, declared assets, and UI contract in the Package repository.
2. Assemble the Package Directory and write final sizes and SHA-256 digests into `teamarness.package.json`.
3. Copy the directory outside the Teamarness repository if you want to test the real separate-source path.
4. Follow [Local Package registration](local-package-registration.md). Use the manifest identity as `expectedIdentity`, accept the native-code risks, and restart Teamarness.
5. Check the Shell's source, lifecycle, and generation values. Exercise requests, subscriptions, suspend, recover, dispose, restart, and data continuity.
6. Rebuild in place when needed, then restart. There is no source watch or hot reload for Backend bytes.

## Promotion to application distribution

Promotion changes distribution metadata, not Package-owned code or contracts. Give the release Package Directory to an application maintainer. The maintainer adds a reviewed declaration to the application Package builder and updates the matching Tauri resource and external-binary configuration. The builder remains the only writer of `packages/application-packages.v1.json`; Package authors do not edit that generated index or copy Tauri resources by hand. Run `npm run build:application-packages` to stage and verify the declared Package Directories, then remove the installation override and restart to confirm the bundled source uses the same Package Data Directory.

Signing and notarization, Gatekeeper remediation, other operating systems, remote Package fetching, Package updates, hot reload, enablement policy, and synchronization engines remain outside the current delivery claims.
