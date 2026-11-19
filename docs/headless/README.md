# Run a headless Teamarness Installation

Use the headless application as a continuously available peer without a graphical session. It uses the same Core Runtime and Package Host as desktop. Agent and External Gateway source, artifacts, and registrations are not required. They remain independent Personal Packages; their native product compatibility is not established by this application build.

## Build one native application

Use the repository's pinned Rust 1.99.1 and Node.js 24.20.0, npm, and a native C/C++ toolchain. SQLite builds from bundled source. On Mac, use Apple Silicon with Xcode Command Line Tools. The Linux qualification target is Ubuntu 26.04 Server x86_64 GNU/Linux. No display server, Tauri, browser runtime, or virtual display is needed for headless execution. Building complete Package Directories still needs npm for their inert UI assets.

From the checkout, install locked dependencies with `npm ci`. Then run **one** matching command:

```sh
npm run build:app -- --app desktop --target aarch64-apple-darwin
npm run build:app -- --app headless --target aarch64-apple-darwin
npm run build:app -- --app headless --target x86_64-unknown-linux-gnu
```

Both selectors are mandatory. Unsupported or non-native selections fail before building or removing output. Cross-compilation, Linux desktop, other CPU/libc targets, and build-all are not supported.

Desktop delegates to `npm run tauri build -- --bundles app,dmg` and retains `target/release/bundle/`. It does not build headless. The existing `npm run build` remains a desktop-oriented compatibility command, not a headless build. [Desktop release tooling](../releasing.md) remains independently invoked and unchanged.

Headless builds only its binary and the Team Packages in `packages/bundled-packages.json`. It never invokes the desktop frontend or Tauri. The result is:

```text
target/teamarness-applications/headless/<target>/
  teamarness-headless
  packages/
    application-packages.v1.json
    <Package Identity>/...
```

Keep this complete directory together. Each Package contains one native Backend and its complete validated UI and contracts. The UI is not executed. Assembly verifies final file digests, sizes, native executable headers, and private permissions. Mac ad-hoc signing happens before hashing. Linux never invokes codesign. These are local distributions, not a public signing, publishing, or update service. Mac ad-hoc signatures are not Developer ID signing or notarization.

Run the final artifact smoke from the checkout, replacing `<target>` with the native selection:

```sh
node scripts/smoke-headless.mjs target/teamarness-applications/headless/<target>
```

The smoke copies the final distribution outside the checkout, removes GUI environment variables, tests offline help/version, starts disposable storage, checks admitted Team Packages, and verifies SIGTERM, SIGINT, and persistent identity. It does not join a Workspace or prove domain synchronization. Linux native build, smoke, systemd syntax, and actual Ubuntu service/logout/reboot observations remain pending native qualification; Mac checks cannot qualify Linux.

## Install and start in the foreground

Copy the complete native distribution to a private, stable directory. Preserve executable modes and do not re-sign or edit declared Package files after assembly. In the following commands, set `APP` to the installed executable's absolute path and `DATA` to a new private absolute path:

```sh
APP="$HOME/.local/lib/teamarness-headless/teamarness-headless"
DATA="$HOME/.local/share/teamarness-headless"
umask 077
"$APP" --help
"$APP" --version
"$APP" serve --data-dir "$DATA"
```

Run clients in another terminal with the same `APP` and `DATA` values. Only `serve` initializes an Installation. Clients never create a missing directory or start a daemon. One application owns each data directory. Never share a running profile with desktop or clone an identity to another machine.

The default resource root is the canonical executable's parent, independent of cwd and executable symlinks. For an intentional separate resource installation, only `serve` accepts `--application-root /absolute/resource/root`. Clients cannot override resources.

Mac headless runs in the foreground. Stop with Ctrl-C or SIGTERM. No launchd integration is included. Startup reports application and Package diagnostics, not universal sync readiness. All joined Workspaces activate required eager instances without UI selection; on-demand activation stays on-demand.

## Join and inspect a Workspace

Obtain a new invitation from a compatible desktop Workspace Manager. Each Installation joins as a separate Workspace Person. Store the opaque invitation in an owned regular file with no group or other permission bits. Do not put bearer values in argv, shell history, logs, or verification notes.

Set `INVITATION_FILE` to that private file's absolute path. A symlink is refused. Stdin (`--invitation-file -`) is the alternative.

```sh
"$APP" --data-dir "$DATA" identity set-name --name vm-one
"$APP" --data-dir "$DATA" identity show
"$APP" --data-dir "$DATA" workspace join --person-name vm-one --invitation-file "$INVITATION_FILE"
"$APP" --data-dir "$DATA" workspace list
"$APP" --data-dir "$DATA" status
```

Read the returned IDs. Set `WORKSPACE` to the exact Workspace ID before further commands. There is no implicit selection by name, cwd, or sole membership.

```sh
"$APP" --data-dir "$DATA" workspace members --workspace "$WORKSPACE"
"$APP" --data-dir "$DATA" workspace rename-self --workspace "$WORKSPACE" --name vm-one
"$APP" --data-dir "$DATA" agent list --workspace "$WORKSPACE"
"$APP" --data-dir "$DATA" agent add --workspace "$WORKSPACE" --name helper
```

Core enforces existing stewardship for Agent rename/removal. Creating an Agent does not configure a harness or bind a Package. The CLI does not create Workspaces or add Manager administration.

A join can commit membership and then fail Host reconciliation. Inspect the returned details and `workspace list`; do not blindly repeat the invitation. An unavailable peer or degraded Package status is not proof of lost membership.

## Admit and configure a Personal Package

Build a compatible native Package independently. Set `PACKAGE_DIR` to its absolute Package Directory and `PACKAGE` to the expected Package Identity. Inspection does not execute its Backend:

```sh
"$APP" --data-dir "$DATA" packages inspect --directory "$PACKAGE_DIR"
"$APP" --data-dir "$DATA" packages list
```

Review the preview and native-code risks. A trusted native Package has your OS user's filesystem, network, and process authority. Process separation is not a sandbox. To consent explicitly:

```sh
printf '%s\n' '{"acceptedRisks":["filesystem","network","process"]}' | "$APP" --data-dir "$DATA" packages install --directory "$PACKAGE_DIR" --package "$PACKAGE" --input -
"$APP" --data-dir "$DATA" package operations --workspace "$WORKSPACE" --package "$PACKAGE"
```

Set `OPERATION` to a declared request name. Discover its admitted schema and contract digest:

```sh
"$APP" --data-dir "$DATA" package schema --workspace "$WORKSPACE" --package "$PACKAGE" --operation "$OPERATION"
```

Set `DIGEST` to the returned contract digest and `INPUT_FILE` to an owned private regular JSON file that matches the request schema. Use `umask 077` before creating input files. Do not repair an untrusted path with a recursive chmod. Files must not be symlinks or have any group/other bits. Reads are bounded and accept exactly one JSON value. Stdin (`--input -`) is also supported.

```sh
"$APP" --data-dir "$DATA" package request --workspace "$WORKSPACE" --package "$PACKAGE" --operation "$OPERATION" --contract-digest "$DIGEST" --input "$INPUT_FILE"
"$APP" --data-dir "$DATA" packages restart --workspace "$WORKSPACE" --package "$PACKAGE"
"$APP" --data-dir "$DATA" packages pause --workspace "$WORKSPACE" --package "$PACKAGE"
"$APP" --data-dir "$DATA" packages resume --workspace "$WORKSPACE" --package "$PACKAGE"
"$APP" --data-dir "$DATA" packages reload --package "$PACKAGE"
"$APP" --data-dir "$DATA" packages remove --package "$PACKAGE"
```

Pause/resume are Personal-only; Team instances and overrides are refused. Install/reload/remove are Installation-wide. Removal retains Package data. Team changes remain restart-bound. After reload, rediscover schemas and the digest. One operator request owns each Instance through cleanup, even after its client disconnects. Concurrent configuration receives `busy`, not replacement or queued replay. No UI resource or JavaScript is executed. Subscriptions and graphical pickers are not operator request workflows. Bind an Agent only through the Package's own declared handlers and existing capability.

## Handle errors without replay

Results are JSON on stdout. Routine stderr and service logs omit request/response bodies. Explicit success responses and bounded error details can contain sensitive Package output; protect redirected output too.

Exits are 0 for success, 1 for operational failures or uncertain outcomes, 2 for invalid invocation/input/compatibility, and 130 for an interrupted client. Losing a reply or interrupting observation after dispatch does not prove that a mutation failed. The client neither reconnects nor retries mutations.

Proven input, unknown-operation, digest, and busy preflight refusals are certain and do not dispatch. After existing-validator preflight, **every UiRequest-returned failure is conservatively `outcomeUncertain: true` and `retryable: false` in the outer operator error**. The existing Host UI facade loses internal dispatch uncertainty, including on late Package errors. The original bounded Host/Package envelope remains in `details`, with its own code, details, and retryability unchanged. Embedded retryability is not permission to replay, and a Package error is not proof of no mutation. Inspect Package-owned state before taking further action. No new shared protocol or universal idempotency ledger is supplied.

## Run the Linux user service manually

Run under the same unprivileged account as local agents. Do not use a system-wide root service. The [example user unit](teamarness-headless.service.example) expects the complete distribution at `%h/.local/lib/teamarness-headless/` and data at `%h/.local/share/teamarness-headless`. Replace both paths in the unit if your installation differs. `%h` is the user home; systemd does not expand `$HOME` in `ExecStart`. For paths with spaces, use systemd argument quoting. The executable finds resources beside itself.

After verifying the foreground artifact, manually copy the unit to `~/.config/systemd/user/teamarness-headless.service`. Create the unit directory with private permissions if needed. Review the unit before enabling it:

```sh
systemd-analyze --user verify "$HOME/.config/systemd/user/teamarness-headless.service"
systemctl --user daemon-reload
systemctl --user enable --now teamarness-headless.service
systemctl --user status teamarness-headless.service
journalctl --user -u teamarness-headless.service -b
```

To request boot startup and survival after logout, manually enable lingering for your account, subject to the machine's administrator policy:

```sh
loginctl enable-linger "$USER"
loginctl show-user "$USER" -p Linger
```

The unit uses `UMask=0077` and `Restart=on-failure`. It sends SIGTERM to the main process first (`KillMode=mixed`). The application has 120 seconds from the first terminal signal to drain admitted operations and shut down Host then Core. `TimeoutStopSec=150s` leaves margin before systemd forcibly kills remaining cgroup processes. Forced cleanup is not a successful graceful shutdown; a deadline exit is nonzero/uncertain. Do not shorten the unit timeout below the application's budget.

```sh
systemctl --user stop teamarness-headless.service
systemctl --user start teamarness-headless.service
systemctl --user restart teamarness-headless.service
```

Before replacing a distribution, stop its service and preserve private data. Do not delete ownership locks or process leases to bypass refusal. Inspect bounded diagnostics and resolve ownership or ambiguous process evidence safely. Use `journalctl` to distinguish graceful stops from failure restarts and forced termination. Disable with `systemctl --user disable --now teamarness-headless.service` when intended. No build or smoke command installs a unit, enables lingering, or changes login policy.

Verify logout persistence, reboot startup, retained identity/membership, stop/restart, and catch-up on the actual Ubuntu 26.04 machine. Syntax checks alone do not establish these outcomes. Native Linux and human observations remain required before overall acceptance.

## Protect storage and qualify synchronization

The private data directory contains unencrypted Installation secrets and Content Keys in `core.sqlite3` (ADR 0017). Same-user programs and trusted native Packages can read them. Use account isolation, encrypted disks and backups, and no cloud-synced profiles. Invitations, input/output files, and Package-owned credentials need the same care. Local administration is owner-only Unix-socket authority, not a network endpoint or a substitute for gateway authorization.

A running peer is not a point-in-time backup. Package registrations, credentials, Agent Bindings, repository registrations, Folder Mappings, and Exhibit Drafts remain Installation-local. No universal synchronized/backup-complete indicator exists. Domain convergence, offline catch-up, real desktop compatibility, and service reboot behavior require the final qualification matrix. A fresh-profile artifact smoke alone proves none of those outcomes.
