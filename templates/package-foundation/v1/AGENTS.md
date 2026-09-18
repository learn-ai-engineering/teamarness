# Working on __PACKAGE_DISPLAY_NAME_MARKDOWN__

This repository contains the standalone Teamarness Package `__PACKAGE_IDENTITY__`. Read this file before changing its contract, Backend, UI, persisted data, dependencies, or build output.

## The system this Package joins

Teamarness is a local-first, peer-to-peer team workspace. A team can use one desktop application while Packages provide separate product capabilities.

The Teamarness Core Runtime owns authority and behavior shared by Packages:

- Installation identity;
- Workspace creation and selection;
- canonical Workspace membership;
- Package lifecycle; and
- implementations of declared Semantic Capabilities.

The Teamarness Shell owns desktop navigation and Package UI mounting. The Package Host admits Package bytes, starts Package Backends, routes declared requests, supplies Workspace context and data paths, and supervises Backend processes.

This Package owns its product domain. That includes its request schemas, domain rules, validation, records, migrations, projections, policy, Backend behavior, and UI. Keep product-specific behavior here rather than expecting a change to the Teamarness Core Runtime, Shell, or Package Host.

## Runtime path

A normal request follows this path:

```text
Package UI
  -> mount-scoped PackageClient
  -> Teamarness Package Host
  -> supervised Package Backend
  -> Package-owned behavior or data
  -> response through the same declared contract
```

`ui-contract.json` defines the requests that may cross this path. Treat it as the shared boundary between the UI and Backend. Change the contract before implementing a new request on either side.

The current foundation proves this path with `__REQUEST_NAMESPACE__.read`. It has one Backend, one bundled ESM UI entrypoint, one UI contract, no UI assets, no subscriptions, and no Semantic Capabilities. It targets `aarch64-apple-darwin`.

## Project map

| Path | Responsibility |
|---|---|
| `teamarness.package.json` | Declares Package identity, compatibility, Backend artifacts, UI artifacts, the UI contract, assets, and required capabilities. |
| `ui-contract.json` | Contains closed JSON schemas for every UI request, response, subscription request, and event. |
| `backend/src/lib.rs` | Owns domain behavior, lifecycle handling, validation, and Package data. |
| `backend/src/main.rs` | Starts the Backend through the vendored SDK. Keep it thin. |
| `backend/tests/` | Exercises Backend behavior through stable public seams. |
| `ui/src/index.ts` | Mounts the Package UI and communicates through the supplied `PackageClient`. |
| `ui/src/index.test.ts` | Exercises UI behavior with a Package client test double. |
| `scripts/assemble-package.mjs` | Assembles final Backend, UI, contract, and manifest bytes into `dist/`. |
| `vendor/teamarness/` | Contains the generated copies of the Package Process Protocol and SDK source used by this project. |
| `teamarness-authoring.json` | Records template, protocol, UI host, SDK provenance, and request namespace information. |
| `Cargo.lock` and `package-lock.json` | Lock this project's Rust and JavaScript dependency graphs. Commit both. |
| `dist/` | Generated Package Directory output. Never edit it by hand. |

The root Cargo and npm workspaces belong to this Package project. This project must build without a Teamarness checkout path.

## Design rules

### Start with the contract

For each product operation:

1. Give it a lowercase dotted name in `ui-contract.json`.
2. Define separate, closed request and response schemas.
3. Add Backend behavior and tests.
4. Add UI behavior and tests.
5. Build and inspect the final Package Directory.

Use IDs for stable domain choices and labels for presentation. Reject unknown fields where the contract allows. Version persisted schemas and plan migrations before changing stored records.

Do not add a request that bypasses the product model merely because it is convenient for the UI. Prefer a small operation that states user intent and lets the Backend enforce invariants.

### Keep domain behavior in the Backend

The Backend should own:

- domain rules and calculations;
- validation at trust boundaries;
- random seed or identifier creation when those values affect product meaning;
- persisted records and schema migrations;
- save-time verification and regeneration; and
- background work that must outlive a visible UI mount.

Prefer deterministic pure functions for rules that can be expressed that way. Test them with independent examples, invalid inputs, and boundary values.

When generated output includes a seed, revision, mode, or other value that is meant to affect the result, test the result itself. Use fixed inputs at the pure behavior seam and prove both directions: equal inputs produce equal output, and selected unequal inputs produce unequal output. A test is wrong if only metadata changes while the meaningful output stays fixed.


The Backend receives its Package Data Directory during lifecycle activation. Use that supplied path. Do not hard-code an application-data path, inspect Teamarness databases, or store Package records in the project directory.

A Package Data Directory belongs to one Package Identity, Workspace, and Installation. Data is not global across Workspaces. Teamarness does not inspect, migrate, back up, delete, synchronize, or share Package-owned files for you.

Keep lifecycle operations bounded. The vendored SDK handles the Package Process Protocol and control-frame machinery. Use its public adapter rather than hand-writing protocol frames.

### Keep the UI mount-scoped

Export `mount(context)` from the ESM entrypoint and return an idempotent unmount function.

Use `context.root` for rendered content and `context.client` for Package requests. Clean up listeners, subscriptions, timers, and DOM nodes during unmount. Present loading and request failures explicitly.

The UI should own interaction and presentation, not durable product authority. Do not calculate canonical records in the UI. The UI runs in a sandboxed frame, so Tauri is unreachable, browser storage and the network are unavailable, and the Shell-owned DOM is out of reach; `context.client` is the only way out.

Use semantic controls and accessible names. Mark purely decorative content outside the accessibility tree. Test behavior through user-visible state and PackageClient requests rather than internal function calls.

### Treat P2P and capabilities as explicit dependencies

Teamarness is a P2P application, but that does not make Package records synchronized. Do not open direct peer connections or assume access to Core Runtime transport, identities, keys, membership state, databases, or secrets.

A Package uses Teamarness-owned authority only through versioned Semantic Capabilities declared in `teamarness.package.json` and granted by the host. Add a capability only when the product requires it. Define the behavior and failure cases before changing the manifest.

Package source, trust, Workspace enablement, synchronization, sharing, networking, and collaboration are separate decisions. A local registration provides none of them automatically.

## Artifact rules

`npm run build` creates `dist/__PACKAGE_IDENTITY__/`. That Package Directory is the unit Teamarness validates and runs. The assembler writes final sizes and lowercase SHA-256 digests after copying the final files.

Do not edit `dist/` after assembly. Any byte change makes the manifest declaration stale.

The foundation assembler copies one Backend executable, `ui/dist/index.js`, and `ui-contract.json`. The manifest declares no assets. Keep the UI inside the single ESM bundle unless the product needs a different artifact shape.

If you add UI assets, another Backend, or another target platform, update all of the following together:

- `teamarness.package.json`;
- `scripts/assemble-package.mjs`;
- build dependencies and commands; and
- tests that verify final paths, permissions, sizes, and digests.

Do not add this project to the Teamarness Cargo or npm workspaces. Do not edit Teamarness application Package metadata for an installation-local Package.

## Vendored SDK policy

The source under `vendor/teamarness/` was copied from the Teamarness checkout that created this project. It makes the project independent of that checkout.

Treat vendored SDK files as external infrastructure. Do not modify them to implement Package behavior or to work around a contract mistake. Change Package code first. If an SDK defect blocks correct implementation, report the exact defect and the smallest upstream change needed.

There is no supported in-place SDK upgrade yet. Teamarness SDK fixes do not arrive in this repository automatically. Preserve source notices, license metadata, relative dependency paths, and `teamarness-authoring.json` provenance.

## Security model

A Package Backend runs with the current user's ambient filesystem, network, and process authority. Process supervision is a recovery boundary, not a sandbox. Avoid unnecessary dependencies and operating-system access. Validate paths and untrusted input.

A Package UI runs in a sandboxed frame with an opaque origin. Tauri is unreachable, the Shell's DOM and storage are unreachable, and the frame's own policy denies network requests, nested frames, form navigation, and inline code. Treat the mount root and PackageClient as the supported interface. Frame isolation separates realms, not threads: a UI that blocks its thread freezes the Shell.

Review both Backend and UI dependencies before shipping or registering the Package.

## Working method for agents

Before editing:

1. Read `teamarness.package.json`, `ui-contract.json`, and `teamarness-authoring.json`.
2. Read the relevant Backend and UI tests.
3. Read the local SDK README for any interface you will use.
4. State the product behavior, ownership boundary, files to change, and tests to add.
5. Confirm whether the artifact shape or declared capabilities must change.

While editing:

- work only in this Package project;
- keep the contract, Backend, UI, and tests aligned;
- write a failing behavior test before production behavior when the seam is testable;
- use real temporary directories for persistence tests;
- keep lockfile changes deliberate; and
- do not weaken schemas, tests, permissions, or digest checks to make a build pass.

Before reporting completion, run:

```sh
npm run typecheck
npm run lint
npm test
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
npm run build
```

Then inspect `dist/__PACKAGE_IDENTITY__/` and report:

- which behavior changed;
- which contract operations changed;
- which persisted schema or migration changed;
- whether capabilities or artifact shape changed;
- the tests and checks that passed; and
- any remaining runtime or manual verification.

Do not claim synchronization, team sharing, sandboxing, another target platform, or compatibility beyond what the manifest and tests prove.
