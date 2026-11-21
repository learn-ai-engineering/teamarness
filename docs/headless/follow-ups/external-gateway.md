# External Gateway headless handoff

## Qualified Host surface, not gateway product evidence

This brief seeds an independent gateway workstream. The application can synchronize indefinitely without Agent or gateway source, artifacts, or registrations. Generic Personal fixtures qualify Host admission/configuration/lifecycle, **not actual gateway, managed `tmx`, policy/token/stream/write behavior or Linux compatibility**. Those product checks are UNRUN here. Native Ubuntu N and human M1–M5 remain mandatory before overall application acceptance.

Pin the source commit containing this brief with `git rev-parse HEAD`, both locks, and actual final artifact hashes in your workstream. The application handoff ledger records the exact qualified postcommit Host/artifact pair outside source docs, avoiding a self-referential commit hash. Recheck a changed checkout rather than inheriting an old qualification. Integrated generic Host baseline `2039693c1d077413747d19501bbe5277e0c7126f` is historical ancestry, not the final application candidate.

## Implemented commands and supported contracts

Build one native Host application, separately from the gateway:

```sh
npm run build:app -- --app headless --target aarch64-apple-darwin
```

For native Ubuntu 26.04 x86_64, select `x86_64-unknown-linux-gnu`. Cross-compilation and Linux desktop are not supported selections. Use the [operator guide](../README.md) and [canonical verification runbook](../verification.md), not a second gateway-specific Host procedure.

- Foreground `serve --data-dir <private-dir>` owns Core/Host. Clients use the private same-UID Unix socket. This is full local operator authority, **not** gateway HTTP, bearer-token authorization, or an external-agent data API.
- `packages inspect --directory` never executes the candidate. `packages install --directory --package <expected-identity> --input <private-file|->` requires explicit native-code consent. Packages remain independently distributed complete Package Directories with inert but validated UI artifacts.
- `package operations` and `package schema --operation` return immutable admitted metadata/digest. `package request --operation --contract-digest --input` invokes existing validated handlers with explicit `--workspace` and `--package`. No implicit Workspace, Host domain branch, direct database edit, unchecked path privilege, graphical picker, or subscription interface is added.
- Requests hold per-Instance ownership through response/cleanup after observer loss, use ordinary generation-bound non-rendered leases, and never execute UI JavaScript. Concurrent operator requests receive `busy`; normal Ports and background synchronization do not share that gate.
- `packages reload/remove` is Installation-wide. `restart/pause/resume` names Workspace/Package; pause/resume is Personal-only. Ambiguous lifecycle failures are outer unknown/nonretryable, with unchanged original details and certain proven refusals. See the [operator error policy](../README.md#handle-errors-without-replay). Removal retains data. Failed reload may remove the old instance. Rediscover metadata after reload; never assume a stale generation is usable.

Preserved versions: Package manifest schema 1, UI contract schema 1, Package Process Protocol 1.0/1.1 negotiation (including existing confirmed streams), UI Host version 3, and existing package-ports 2.0/3.0. Major-1 declarations fail negotiation without a legacy dispatcher. Person-authorized gateway consumers use their existing package-ports 3.0 contract; major-2 consumers retain their declared semantics. No new shared protocol/capability, headless Package mode, SDK/vendor migration, or gateway-specific Host branch is introduced. Native Backend targets are `aarch64-apple-darwin` and `x86_64-unknown-linux-gnu`.

The 256 KiB Package payload bound, existing validators/Host deadlines, 90-second client observation, and bounded socket/operation admission apply. A client losing observation does not cancel or replay admitted work. Long Package-owned setup can return an operation ID and expose a declared status request. The 120-second application stop budget and 150-second Linux service stop allowance do not prove graceful completion of arbitrary setup.

## Preserve error fidelity and credential authority

For local operator CLI tooling, continuously drain stdout and stderr. The interruption/exit130 contract requires responsive output consumption. A full, open non-draining destination, including a shared `2>&1` pipe, can prevent prompt client termination or warning delivery. Delivery into a full pipe is not guaranteed. A caller can supervise and terminate the client under its own policy. Lost observation or termination after mutation dispatch leaves an unknown outcome. Inspect Package-owned state before another intentional action. Never automatically replay the mutation. See the [operator output and error policy](../README.md#handle-errors-without-replay). This boundary does not require the gateway runtime to use shell pipes or the operator CLI. Foreground server shutdown guarantees remain unchanged.

Before dispatch, proven validator/input, unknown-operation, stale-digest, and busy refusals are certain. After same-lease existing-validator preflight, **every UiRequest-returned failure is conservatively uncertain and nonretryable in the outer operator envelope**. The shared UI facade loses internal dispatch uncertainty, including late Package errors. Original bounded Host/Package codes, details, and embedded retryability remain unchanged. A Package error is not proof of no effect; do not replay automatically or treat embedded retryability as permission to do so. Query Package-owned setup/state first. No universal idempotency ledger exists.

Run service, gateway, and local agents under the chosen same unprivileged OS account. The operator socket is not a security boundary against same-user agents/native Packages. Model/gateway secrets enter private files/stdin, not argv. Routine diagnostics omit bodies, but explicit success/error output can contain secrets. Core secrets remain unencrypted private storage; use OS account isolation and encrypted disks/backups.

## Historical product findings to recheck

Historical gateway source `cb071a5b3506c4621a014c5f2e5bfa17a1b2a76b`, generic Host integration `96ff960f55fd5cfdeaf9aa0fa722b13008f1033a`. These are source findings, not a supported current pair:

1. Manifest/assembler declares only Apple Silicon. Produce actual native Linux bytes and truthful path/size/digest metadata, not renamed Mac binaries.
2. `backend/Cargo.toml` includes `rfd`, `objc2`, and `objc2-app-kit` unconditionally; `picker_helper.rs` invokes AppKit. Gate real platform code/dependencies, preserve Mac desktop pickers, and qualify headless without GUI linkage/session.
3. `backend/src/cli/home.rs` resolves effective-user home with `/usr/bin/dscacheutil`; Backend setup shares it. Linux needs account-based resolution, not arbitrary environment `HOME` authority.
4. Existing listener, exact Package/tool policy, tokens, and `setup.preview/apply/status` handlers own setup. Reuse them. `tmx` remains the data client; it cannot grant itself policy or bootstrap authority.
5. `setup.preview` permits no skill selection for ordinary connection setup. Existing `install-skill` uses a picker. Automatic headless skill-folder publication/native Linux picker support needs its own Package-owned path/preview/consent decision, not fabricated selection IDs or an unchecked Host path route.
6. Managed CLI installation, OS-home lookup, private credential storage, partial setup/repair, policy recovery, and actual `tmx` execution need native product tests beyond compiling the listener.

Keep one gateway identity/codebase, independent artifacts/registration, complete UI, loopback exposure, exact-tool consent, Person attribution, confirmed streams, live-only delivery/backpressure, and no automatic replay. Respect canonical SDK/vendor provenance. Send generic Host gaps upstream; do not hand-patch vendors.

## Evidence required in the gateway workstream

1. Record current Package/SDK/vendor source, exact Host pair, locks/toolchains, native artifact hashes, and ordinary admission for each separately selected target.
2. Use disposable injected homes/profiles, no Agent Package and no ordinary credentials/managed CLI. Configure UI-free connection through preview/apply/status with explicit listener/tool policy; test partial failure and repair.
3. Run the installed native `tmx` for real permitted Documents/Backlog/Conversations reads and writes. Assert independent content, attribution, Provider versions/pagination, authorization refusals, stale generation, streams, errors, and uncertainty.
4. Reject missing/wrong/revoked tokens and unselected tools. Verify stream confirmation, live-only behavior, backpressure and no reconnect/replay.
5. Restart Host/Package and verify retained private configuration/managed connection. Remove gateway and assert continuing Team synchronization; reinstall without rebuilding Host.
6. Qualify actual Ubuntu 26.04 native runtime paths and Mac headless without AppKit. Preserve actual Mac desktop onboarding/pickers. Treat GUI-only skill publication as separately scoped work.

Host evidence references: `apps/teamarness-headless/tests/{package_management,package_requests,control_socket}.rs`, generic `headless_personal_fixture`, `tests/system/tests/{headless_mesh,headless_personal_flow}.rs`, and `node scripts/verify-headless.mjs`. The new runner builds current native fixtures and runs every named target without silent skips. No real gateway repository, product artifact, external configuration, credential, or registration is required by these proofs.
