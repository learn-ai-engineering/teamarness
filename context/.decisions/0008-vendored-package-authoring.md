# ADR 0008: Vendored Package authoring foundation

- Status: accepted
- Date: 2026-09-15

## Decision

Teamarness provides a foundation-only external Package generator:

```sh
npm run create:package -- --output <directory> --identity <reverse-dns-id> --display-name <name>
```

Template version 1 copies the canonical production Rust Package Process Protocol, Rust Backend SDK, and TypeScript UI SDK source into `vendor/teamarness/`. It supplies standalone manifests, exact third-party dependency declarations, committed Cargo and npm lockfiles, a request-only Package foundation, an independent Package Directory assembler, and a generated `AGENTS.md`. The agent guide explains the Teamarness runtime boundary, Package ownership, contract-first development, data scope, artifact rules, SDK policy, security model, and required checks. Generated projects are independent of Teamarness checkout paths after creation. Existing source notices and MIT OR Apache-2.0 license metadata remain with the generated SDK copies.

The command and Package Host contracts are the stable seams. The generator does not create finished Emojart behavior, register a Package, edit application Package metadata, add capabilities, or provide Workspace enablement, synchronization, sharing, networking, or collaboration. There is no SDK updater (no updater is provided) and no supported in-place upgrade for generated vendored copies.

## Rejected alternatives

- Cargo and npm publication: deferred until release ownership and versioning are ready.
- Git, tarball, or registry dependencies: rejected as the first authoring route because they add artifact access and upgrade questions without a supported release policy.
- Manual copying: rejected because coordinated manifests, contracts, and assembly are easy to miss.
- A full Emojart generator: rejected because the tutorial must leave the implementation exercise to the learner.

## Exit condition

Do not claim production authoring support for Packages until Teamarness publishes versioned Rust and TypeScript SDK artifacts and documents a supported generated-project upgrade policy. That future migration may change generator dependencies without changing the command or Package Host contract.
