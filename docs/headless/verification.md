# Qualify a native headless Installation

Use this canonical runbook for automated gates and Don's M1–M5 observations. Use disposable data, not an ordinary profile. Keep invitations, keys, request bodies, and private database contents out of evidence. Record identifiers and independently observed content instead.

## Record the candidate

Record the source commit, dirty status, OS release, architecture, Rust/Cargo, Node/npm, command exit codes, executed test names, and SHA-256 hashes of final artifacts. Do not call a modified checkout the committed candidate.

```sh
git rev-parse HEAD
git status --short
uname -a
rustc -vV
cargo --version
node --version
npm --version
```

On Ubuntu, also record `/etc/os-release` and `systemd --version`. Ubuntu 24.04 CI is supplementary, not Ubuntu 26.04 evidence. A Mac result does not qualify Linux.

## Run source-current loopback automation

From the repository root with locked dependencies installed:

```sh
npm ci
node scripts/verify-headless.mjs
```

The runner builds every system binary from current Cargo sources, including the production CLI, Personal fixtures, and Documents, Backlog, and Conversations Backends. It then runs **both** `headless_mesh` and `headless_personal_flow`, requiring every named case to execute with zero failures, ignored cases, or filters. It has no skip/prebuilt/target-override switches. It records source and fixture hashes and refuses changed source or binaries during the run. External Agent/gateway configuration variables are refused; neither external repository nor artifact is read.

The tests use two production headless compositions and CLI/socket endpoints, plus an ordinary Core/Host desktop-backend composition. They cover two joined Workspaces, both directions, late admission, offline edits/restart catch-up, real domain content, canonical Document body revisions through a generic fixture's Person Ports, and unchanged Team generations across Personal install/remove. Background work holds no UI lease. Short request leases close after each observation or mutation. UI fixture bytes are inert; these tests do **not** render or qualify the desktop UI or production Iroh mesh.

## Run final Mac gate M serially

Freeze source before this gate. Run each command only after the previous command succeeds:

```sh
npm run check
cargo build --workspace --locked
CI=true npm run build:app -- --app desktop --target aarch64-apple-darwin
npm run build:app -- --app headless --target aarch64-apple-darwin
node scripts/smoke-headless.mjs target/teamarness-applications/headless/aarch64-apple-darwin
```

`CI=true` skips Finder cosmetics, not bundle checks. These broad checks intentionally differ from a selected application build. The artifact smoke relocates complete resources and exercises help/version, GUI-free foreground startup, signals, and identity persistence. It does not prove Workspace synchronization.

Record all executable/resource hashes and the app/DMG hashes after the last build. If only documentation wording changes afterward, compare source/build inputs and artifact hashes instead of rebuilding bundles without cause.

## Run native Ubuntu 26.04 lane N

Don supplies native access/source delivery separately. Do not install system tools, push source, enable a service, or use a Linux desktop build implicitly. On actual Ubuntu 26.04 x86_64, use the pinned toolchains and a native C/C++ toolchain. Run serially:

```sh
npm ci
cargo fmt --all --check
cargo clippy --workspace --exclude teamarness-desktop --all-targets --all-features --locked -- -D warnings
cargo test --workspace --exclude teamarness-desktop --locked
npm run build:app -- --app headless --target x86_64-unknown-linux-gnu
node scripts/smoke-headless.mjs target/teamarness-applications/headless/x86_64-unknown-linux-gnu
node scripts/verify-headless.mjs
npm run lint
npm run typecheck
npm test
```

Root `npm test` retains the build-selection, guards, and generated-authoring integration tests. The generated foundation template/assembler is intentionally Darwin-only. Its predicted Linux Host-admission conflict has **not** been observed on Linux. If this or another macOS-only scaffold fails, preserve the exact failure, diagnose it, and port only qualification scaffolding without skipping or relabeling the test. A required product/template change needs a bounded upstream decision. Do not edit vendor trees or quietly narrow N. Existing explicitly external optional-Package tests remain external; the new headless targets are never optional.

Validate the service syntax without installation. Set `APP_ROOT` to the absolute final Linux distribution directory. Set `SERVICE_CHECK_DIR` to a new disposable private directory. This adapts only the example's executable/data paths:

```sh
umask 077
mkdir "$SERVICE_CHECK_DIR"
sed -e "s|%h/.local/lib/teamarness-headless|$APP_ROOT|g" -e "s|%h/.local/share/teamarness-headless|$SERVICE_CHECK_DIR/data|g" docs/headless/teamarness-headless.service.example > "$SERVICE_CHECK_DIR/teamarness-headless.service"
systemd-analyze --user verify "$SERVICE_CHECK_DIR/teamarness-headless.service"
```

Record the exit status on Linux. This is syntax evidence, not logout or reboot evidence. Follow the operator guide for actual installation only when Don authorizes it.

## Observe M1: native artifact outside the checkout

1. On Mac and each Ubuntu VM, copy the complete final native distribution to a private location outside the checkout. Record its hashes before and after the observation.
2. Follow [foreground setup](README.md#install-and-start-in-the-foreground) from another cwd with no graphical session or GUI environment variables. Confirm help/version, ready/degraded diagnostics, and no optional registrations.
3. Stop and reopen the same disposable Installation. Compare its public identity. Attempt a second owner and confirm refusal without disturbing the first.

Use independent data directories on each machine. Never copy an identity between VMs. Mac runs foreground; launchd is not promised.

## Observe M2: production mesh with compatible desktop

1. From the final compatible Mac desktop, create two disposable Workspaces and issue **separate** invitations to each headless VM. Join with distinct Person names through the [operator onboarding commands](README.md#join-and-inspect-a-workspace).
2. Leave Agent and gateway absent. Keep both headless processes running with no selected desktop Workspace and no background request lease.
3. Through declared operator requests and desktop interfaces, create and edit Documents (including body content), Backlog Tickets (including body/priority), and Conversations messages in each Workspace. Read the exact independent content on all three peers, in both directions. Discover schemas before calling an operation. Do not edit Package databases.
4. Stop one headless peer. Edit on the surviving peers. Restart the stopped peer and observe exact content catch-up and retained identity/membership. Add a fresh late peer and verify only history allowed by current synchronization semantics.
5. Record source/artifact revisions, object IDs, expected/observed text, and timing. Do not retain invitations. Automated loopback success is not a substitute for these production Iroh observations.

## Observe M3: service logout and reboot

1. On each Ubuntu VM, follow [manual user-service setup](README.md#run-the-linux-user-service-manually). Record the reviewed unit and explicit enablement/lingering actions.
2. Log out and observe continued synchronization from another peer. Reconnect and inspect the journal.
3. Reboot the VM. Confirm unattended startup, the same identity/membership, and domain catch-up.
4. Stop/start/restart the service and distinguish graceful shutdown from deadline failure or forced cgroup cleanup. Never delete locks/leases or kill unrelated processes to bypass refusal.

## Observe M4: generic Personal configuration

Build the **test fixture**, not an Agent/gateway product. Set `PACKAGE_DIR` to a new absolute directory that does not exist. The helper refuses overwrites and builds the native Backend from current source:

```sh
node scripts/prepare-headless-fixture.mjs "$PACKAGE_DIR"
```

With a running disposable Installation joined to both Workspaces, set `APP`, `DATA`, and `WORKSPACE` as in the operator guide. Use the returned Workspace ID and this independent fixture identity:

```sh
PACKAGE=org.unfamiliar.headless-proof
"$APP" --data-dir "$DATA" packages inspect --directory "$PACKAGE_DIR"
printf '%s\n' '{"acceptedRisks":["filesystem","network","process"]}' | "$APP" --data-dir "$DATA" packages install --directory "$PACKAGE_DIR" --package "$PACKAGE" --input -
"$APP" --data-dir "$DATA" package operations --workspace "$WORKSPACE" --package "$PACKAGE"
"$APP" --data-dir "$DATA" package schema --workspace "$WORKSPACE" --package "$PACKAGE" --operation proof.settings.set
```

Set `DIGEST` to the returned `contractDigest`. These examples contain nonsecret fixture data. Protect real Package inputs and explicit outputs:

```sh
printf '%s\n' '{"value":"manual qualification"}' | "$APP" --data-dir "$DATA" package request --workspace "$WORKSPACE" --package "$PACKAGE" --operation proof.settings.set --contract-digest "$DIGEST" --input -
printf '%s\n' '{}' | "$APP" --data-dir "$DATA" package request --workspace "$WORKSPACE" --package "$PACKAGE" --operation proof.settings.get --contract-digest "$DIGEST" --input -
"$APP" --data-dir "$DATA" packages pause --workspace "$WORKSPACE" --package "$PACKAGE"
"$APP" --data-dir "$DATA" packages resume --workspace "$WORKSPACE" --package "$PACKAGE"
"$APP" --data-dir "$DATA" packages reload --package "$PACKAGE"
```

Rediscover the digest after reload. Discover `proof.mutate`, send `{"delayMs":5000}`, and issue another request concurrently to observe `busy`. Invoke `proof.error` to inspect preserved Package details and conservative outer uncertainty. Never retry a mutation automatically. Use fixture-owned `proof.status` to inspect effects. `proof.secret` returns a bounded fixture-only one-time result; do not generalize its output to real credential safety.

Record Team process generations with `status` before and after lifecycle changes. Restart the application and verify settings. Remove/reinstall the fixture and read the retained value. Repeat with explicit IDs in the second Workspace. Then remove it and verify new Team content still synchronizes with unchanged live Team generations:

```sh
"$APP" --data-dir "$DATA" status
"$APP" --data-dir "$DATA" packages remove --package "$PACKAGE"
```

Pause/resume are Personal-only. Failed reload can remove the old instance; do not assume old metadata remains. Verify the [Agent](follow-ups/agent.md) and [gateway](follow-ups/external-gateway.md) briefs are usable as independent intake. Actual products are **UNRUN**, not qualified by this fixture.

## Observe M5: actual Mac desktop compatibility

Use the final Mac desktop bundle with a disposable profile. Verify onboarding, Package hosting, ordinary quit, application restart with identity continuity, and duplicate-owner refusal. Deterministic terminal tests cover ordinary/updater restart ownership ordering; a published-update test or release is not authorized here. Record actual GUI observations separately from the third Core/Host backend composition.

## Report readiness honestly

Return exact command statuses, executed names, source/artifact hashes, OS facts, and M1–M5 observations. Record unobserved N/manual gates as **pending; overall Verify blocked**, not passed or waived. Local Phase6 automation can commit before those observations. Independent Verify/review follows that commit. A running replica is not a point-in-time backup; Installation-local credentials, registrations, Agent Bindings, Folder Mappings, and Exhibit Drafts do not replicate.
