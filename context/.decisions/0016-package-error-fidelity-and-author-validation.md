# 0016 — Package error fidelity and author validation

**Status:** accepted (2026-09-19)
**Refines:** ADR 0010 (vendored trees and provenance)

## Context

A Package owns its own failures, and a Package UI is the only place a person sees them. The Package Host carried a Package Backend's error as one generic host code, so a Package UI could not tell a permanent refusal from a retryable one, and a Package-owned stream failure faulted the whole mount. A Package author could learn whether a Package Directory would be admitted only by starting the desktop application, because the admission rules lived in a crate that depends on the Core Runtime and cannot be vendored. The Rust SDK left every Backend to hand-write dispatch, JSON shaping, blocking work and cancellation, and every Package UI to cast its request answers.

A Package is written outside this repository, by people and agents who do not have the Host to read. What the seam guarantees therefore has to be visible in the contract, the SDK, and the author's own checks.

## Decision

1. **Package-owned errors cross unchanged.** A Package Backend's stable code, `retryable`, and bounded `details` travel beside the authoritative Teamarness code as a nested `packageError`, through the Host, the desktop bridge and the Package UI Frame, and reach the subscriber as a typed `PackageRequestError`. Details are bounded and dropped rather than truncated. The Package Process Protocol stays 1.0; the host error envelope is version 2 and the private frame bridge protocol is 2.
2. **A Package-owned stream termination is delivered, not a mount fault.** A stream the Backend completes, or closes with its own code, reaches its subscriber through `onTerminated` and does not by itself recover the Package UI Mount. A Teamarness-owned terminal stream error still does.
3. **One set of admission rules.** `teamarness-package-contract` holds the manifest and UI contract rules. The Package Host applies them, and so does `teamarness-package-validate`, the author-runnable validator that a generated project vendors and runs as `npm run validate`, with `--smoke` launching the Backend through handshake, activation and shutdown. The Host's diagnostics stay payload-free: rule and pointer names are for the validator's output.
4. **The SDK owns the machinery, the Package owns the product.** A Backend registers typed handlers on a `Router` with per-activation state; the SDK owns dispatch, typed initialization, cancellation and deadlines, blocking work, an unstarvable control plane, durable storage helpers, typed Semantic Capability clients, a record log session that credits only after ingest, and a protocol-free test harness. Codes the SDK owns are `package.*`.
5. **A Package UI programs against its contract.** `teamarness-contract-types` generates request, response and event types from `ui-contract.json`; the generated module is committed and `typecheck` fails when it is stale. `typedPackageClient` adds no runtime work: the Host already validates every request, response and event against the admitted contract.
6. **A generated project records where its SDK came from.** `teamarness-authoring.json` carries the source revision, whether that checkout was dirty, and a digest per vendored tree; `npm run check:vendor` fails when a vendored file is edited, and `--vendor-report` compares a project with the current checkout without writing. There is still no in-place SDK updater.
7. **The Host owns capability wire contracts.** Each Semantic Capability's shape, bounds, error mapping and version negotiation belong to the Package Host; the Core Runtime owns the authority and state behind them. The Host's limits and the published capability schemas are pinned to each other by test.

## Rejected alternatives

- **Replacing the host code with the Package's code.** The host code says what Teamarness did with the failure, which is what the Shell's recovery depends on. Nesting keeps both.
- **A Package Process Protocol minor version for error details.** Nothing in the frame set changes: the details already fit `error`. A version bump would have cost every Package a renegotiation for no new capability.
- **Faulting the mount on any stream close.** It made a Package's own "this stream is over" indistinguishable from "Teamarness lost the stream", and recovered a healthy Backend.
- **Vendoring the Package Host for validation.** The Host depends on the Core Runtime, Iroh and SQLite. Extracting the rules into a dependency-light crate is what makes an author-side verdict equal to the Host's.
- **A validator that re-implements the rules.** Two implementations of one rule set drift, and the author's copy is the one that is wrong.
- **Runtime type guards in Package UIs.** The Host validates against the admitted contract already; a second check in every Package would cost code and speed without adding a guarantee.
- **An SDK updater for generated projects.** ADR 0010's exit condition is published crates, not an in-place rewriter of a project someone else owns. Provenance plus a drift check gives the author the facts and leaves the decision with them.

## Consequences

- The Rust SDK's `Initialization`, the generated-project template, the host error envelope and the frame bridge protocol are clean breaks. There is no install base and no published SDK, so no shim, dual-read path or migration exists.
- The foundation template is version 2. A project generated from version 1 keeps its vendored copy until it is regenerated.
- `TEAM.SYS.SHELL-R06` now distinguishes a Teamarness-owned terminal stream error from a Package-owned one. `TEAM.SYS.PKG-R06` requires the error fidelity and the author-runnable validator.
- Every bundled Package uses the router, the harness and contract-typed UI requests, so the conventions are demonstrated by the code an author reads.
- A capability limit and its schema can no longer move alone.

## Evidence

- `crates/teamarness-package-contract/` — the shared rules, the `teamarness-package-validate` binary and its smoke launch; `tests/{fixtures,ui_contract_rules,manifest_rules,cli}.rs`.
- `crates/teamarness-package-host/src/{protocol.rs,lib.rs,capability.rs}` — `PackageOwnedError`, envelope version 2, `StreamEnd`, capability families and schema-pinned limits. `tests/{supervisor_failures,ui_event_stream,reference_package,public_api}.rs` hold the behaviour.
- `crates/teamarness-package-sdk/src/{router.rs,capabilities/record_log.rs,testing.rs,storage.rs,blocking.rs,cancellation.rs}` — the Backend machinery, with `tests/{router,capabilities,harness,runtime,storage,blocking,cancellation}.rs`.
- `apps/teamarness-desktop/src/package-port-host.ts` and `src/package-frame/` — the bridge that carries a Package error and a stream termination into the frame without faulting the mount.
- `packages/teamarness-package-ui-sdk/src/index.ts` and `bin/contract-types.mjs` — the typed error, `typedPackageClient`, `adoptPackageStyles`, and the contract-type generator.
- `scripts/create-package.mjs`, `scripts/vendor-digest.mjs` and `scripts/create-package.integration.test.mjs` — provenance, drift detection, and the end-to-end generated-project probe.
- **Weaknesses.** The validator's smoke launch proves handshake, activation and disposal, not that a Backend answers its own declared requests correctly. The generated-project probe runs on `aarch64-apple-darwin` only. Error-detail bounds are enforced at the Host and the frame bridge, and a Package that exceeds them loses its details silently rather than being told.

### Provenance (non-versioned)

`.myflow/workstreams/architecture-review/plan/20260918T000000+1200_phase-6-package-contract-authoring.md` and its design, and `.myflow/workstreams/architecture-review/decisions/20260918_developer-answers.md`.
