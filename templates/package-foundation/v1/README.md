# __PACKAGE_DISPLAY_NAME_MARKDOWN__ Package

This project is a standalone Teamarness Package foundation generated for `__PACKAGE_IDENTITY__`.

Before asking an AI agent to change the Package, have it read [`AGENTS.md`](AGENTS.md). That file explains the Teamarness runtime boundary, project ownership, contract-first workflow, data scope, artifact rules, SDK policy, security model, and required checks.

## Build and check

Run these commands from this project, not from the Teamarness checkout:

```sh
npm ci
npm run validate
npm run typecheck
npm run lint
npm test
npm run build
```

`npm run validate` applies Package Admission's manifest and UI contract rules to this project, and `npm run build` compiles the Backend and UI, assembles `dist/__PACKAGE_IDENTITY__/` with final sizes, lowercase SHA-256 digests and restricted file modes, then validates and smoke-launches that Package Directory. The generated manifest is request-only and declares no capabilities, assets, subscriptions, networking, Workspace enablement, sharing, or collaboration.

## Authoring provenance

This project was generated from foundation template version `2`. It records Package Process Protocol `1.0` and UI host version `1` in `teamarness-authoring.json`, together with the creating checkout's revision (`sdkRevision`, and whether that checkout was dirty) and a digest of every vendored tree (`vendorDigests`).

The Rust and TypeScript SDK sources under `vendor/teamarness/` are copied from that checkout, with their license metadata and source notices preserved. Their dependencies are explicit and locked in `Cargo.lock` and `package-lock.json`. `npm run check:vendor` recomputes the digests, so an edited vendored file is reported by this project's own tests; from a Teamarness checkout, `npm run create:package -- --vendor-report <project>` compares this project with that checkout without changing it.

There is no supported in-place SDK upgrade yet. Fixes to Teamarness SDK source do not reach this generated copy automatically; regenerate from a compatible checkout when the authoring policy instructs you to do so.

## Source and policy boundaries

The generated Package is intended to be registered as an installation-local Package (Package Source `local`, shown as `Personal`). Source kind is separate from Package Trust and does not grant Workspace enablement, synchronization, sharing, networking, or collaboration. The application builder, not this project, owns bundled Package metadata.
