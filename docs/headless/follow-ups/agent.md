# Agent Package headless handoff

## Qualified Host surface, not Agent product evidence

This is intake for a separate Agent workstream. The application implementation uses shared Core Runtime and Package Host, not a new Agent runtime. Its Mac local automated qualification uses generic source-built fixtures. Actual Agent artifacts, model execution, and Agent Linux compatibility are **UNRUN** here. Native Ubuntu 26.04 lane N and human M1–M5 remain required for overall application acceptance.

Use the Host source commit containing this brief. Pin `git rev-parse HEAD`, the actual source tree, lock hashes, and final artifact hashes in your own handoff. The application workstream records its exact postcommit qualified Host/artifact pair separately, avoiding a self-referential source-document commit hash. Do not treat a checkout with later edits as that qualified pair. Historical generic Host integration `2039693c1d077413747d19501bbe5277e0c7126f` is an ancestor, not the final qualified application revision.

## Implemented commands and compatibility

Build exactly one native application from the Host repository:

```sh
npm run build:app -- --app headless --target aarch64-apple-darwin
```

On native Ubuntu 26.04 x86_64, select `x86_64-unknown-linux-gnu` instead. Do not cross-label a Mac binary. See [operator commands](../README.md) and the [canonical verification runbook](../verification.md) for complete onboarding, consent, configuration, service, diagnostics, and evidence procedures.

Implemented interface:

- `serve --data-dir <private-dir>` owns the Installation. Clients use its owner-only same-UID Unix socket. No implicit profile, daemon spawn, network administration, or acting-Principal override exists.
- `agent list/add/rename/remove --workspace <id>` uses Core stewardship. Creating a Workspace Agent does not bind or configure a Package.
- `packages inspect/install/reload/remove` operates on independently built native Personal Package Directories. Install requires `--package <expected-identity>` and explicit native-risk consent via private JSON input. Native code retains the OS user's ambient authority; it is not sandboxed.
- `packages restart/pause/resume --workspace <id> --package <identity>` applies per Instance. Pause/resume are Personal-only. Install/reload/remove spans the Installation. Removal retains data; failed reload may remove the old instance.
- `package operations/schema/request` requires explicit Workspace and Package IDs. Invocation also names `--operation`, `--contract-digest` from admitted metadata, and `--input <private-file|->`. Source edits do not change admitted schemas before reload.
- Each request opens and closes the ordinary generation-bound non-rendered Host lease. No UI loader/resource/JavaScript executes. One operator request owns an Instance through cleanup even after observer loss; concurrency receives `busy`. Background work and normal Ports do not hold that operator gate.

Existing compatibility is preserved: Package manifest schema 1, UI contract schema 1, Package Process Protocol 1.0/1.1 negotiation, UI Host version 3, and existing `teamarness.agent-binding` 1.0. Host offers package-ports 2.0/3.0 together; major-1 declarations fail negotiation without a legacy dispatcher. Use only the existing family/major your Package actually declares. No headless capability, SDK/vendor migration, or wire version is added. Native Backend targets are `aarch64-apple-darwin` and `x86_64-unknown-linux-gnu`; complete validated UI artifacts remain required but inert on headless.

Limits include 256 KiB validated Package payloads, bounded admitted contracts/metadata, a 90-second client observation budget, existing Host request deadlines, and no automatic reconnect/replay. Shutdown closes admission, drains owned operations, then Host/Core under 120 seconds. Linux's example service allows 150 seconds before cgroup cleanup; actual Linux service behavior is not established by Mac tests.

## Error and authority limitations

Proven validator/input, unknown-operation, stale-digest, and busy refusals are certain and do not dispatch. **Every UiRequest-returned failure after same-lease existing-validator preflight has conservative outer `outcomeUncertain: true`, `retryable: false`.** The existing UI facade loses internal dispatch uncertainty, even for late Package errors. Original bounded Host/Package details and embedded retryability remain unchanged. A Package terminal error is not proof of no mutation; embedded retryability is not permission to replay. Inspect Package-owned state first. There is no universal operation-result ledger.

Agent Binding belongs to the Package through its existing capability. Do not write Core storage or Package settings directly. Credentials use Package-owned handlers and private file/stdin input, not argv, logs, or Host-specific configuration branches. Explicit success/error output can contain secrets. Subscriptions, graphical pickers, chat UI, and arbitrary UI workflow parity are not operator request features.

## Source findings to recheck in the independent repository

Historical inspection only, Agent `60f05c67550b9339d1014cd4b773da8dfcdb6ff4`:

- Manifest/assembler declared only Apple Silicon. Native Linux artifacts and truthful target/path/size/digest metadata need independent work.
- `backend/src/lib.rs` exposes `agent.settings.get/set`, `agent.key.set/clear`, `agent.binding.candidates/set/clear`, and status handlers. Settings/binding transitions reconcile background harness work. Recheck current names/schemas before use.
- `backend/src/settings.rs` uses Unix private files. That source finding is not a Linux build or custody qualification.

Keep one Package identity/codebase and separate distribution. Preserve Mac desktop behavior, private key/model custody, budgets, attribution, and settled-work no-replay semantics. A real generic Host gap needs a bounded upstream finding, not an Agent-specific Host branch or hand-edited vendor workaround.

## Evidence to collect in the Agent workstream

1. Record current Package source, clean SDK/vendor provenance, exact Host commit, both locks, toolchains, and native artifact hashes. Build one explicit native target at a time and use ordinary admission.
2. In disposable profiles without gateway, discover admitted schemas, configure model/key through existing requests, create a Core Agent, and bind through the Package's capability.
3. Use a scripted local model to prove background supported interactions as the bound Agent without a persistent UI lease. Record real-model behavior separately.
4. Test restart, durable settings/binding, invalid input, stale generations, unbind/remove, uncertain outcome inspection, and no duplicate settled work. Preserve actual Mac desktop behavior.
5. Run native Linux evidence on Ubuntu 26.04, including process/service-facing paths. Never use ordinary credentials/profiles or infer Linux from Mac.

Host fixture references: `apps/teamarness-headless/tests/{package_management,package_requests}.rs` covers unknown identity, consent, settings, retained data, generation/busy/digest/validation/errors, observer loss, and fixture-owned binding. `tests/system/tests/{headless_mesh,headless_personal_flow}.rs` covers real Team data without external Packages. `node scripts/verify-headless.mjs` builds current fixtures and executes both new targets without skips. These are Host qualification tools, **not Agent product compatibility evidence**.
