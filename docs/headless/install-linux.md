# Install headless Teamarness on a Linux machine

Use this guide to set up a headless Teamarness Installation on an Ubuntu 26.04 x86_64 machine, with the Agent and External Gateway Personal Packages. You build on one Linux machine and copy the results to the target machine. The [operator guide](README.md) explains each command and its error policy in full.

Setup has two tiers:

- **Machine tier, once per machine:** copy the Host and Packages, start the service, and install each Package.
- **Workspace tier, once per Workspace:** join the Workspace, then configure the Agent and the Gateway for that Workspace.

## Before you start

You need:

- A **build machine**: native Ubuntu 26.04 x86_64 with rustup, Node.js through mise or another version manager, npm, git, and a C/C++ toolchain. The build machine can also be the target machine.
- A **target machine**: Ubuntu 26.04 x86_64 with systemd. It needs no toolchain.
- One unprivileged account on the target that runs the Host, the Agent, the Gateway, and any local agent tools. In this guide, that account is called `you`.
- The Teamarness desktop app on another machine, so that you can create Workspace Invitations.
- For each Agent, a model provider endpoint, a model ID, and an API key.

> [!IMPORTANT]
> Every directory above an installed Package directory must not be group- or other-writable. This includes `~/.local/lib`. If a directory is group-writable, the External Gateway refuses to install `tmx` and reports `external_gateway.output_unsafe`.

## Machine tier

### 1. Build on the build machine

Build the Host from a clean Teamarness checkout:

```sh
cd ~/src/teamarness
npm ci
npm run build:app -- --app headless --target x86_64-unknown-linux-gnu
mkdir -p ~/smoke-tmp
TMPDIR=$HOME/smoke-tmp node scripts/smoke-headless.mjs target/teamarness-applications/headless/x86_64-unknown-linux-gnu
```

The smoke test copies the distribution into the temporary directory. Set `TMPDIR` if `/tmp` is a small `tmpfs` or has a user quota. Otherwise, the copy fails with error `-122` (`EDQUOT`).

Build each Package from its own checkout with `npm ci` and `npm run build`. Each build produces a complete Package directory:

| Package | Build output |
|---|---|
| Agent | `dist/dev.teamarness.agent/` |
| External Gateway | `dist/dev.teamarness.external-gateway/` |

If several builds share the build machine, run each one under the same lock, for example `flock ~/src/.heavy-build.lock npm run build`.

### 2. Copy to the target machine

On the target, create the private directories:

```sh
umask 077
mkdir -p ~/.local/lib/teamarness-packages
chmod go-w ~/.local ~/.local/lib
```

From the build machine, copy the complete directories. Do not use symlinks, and do not edit files after the copy:

```sh
rsync -a --delete ~/src/teamarness/target/teamarness-applications/headless/x86_64-unknown-linux-gnu/ target:.local/lib/teamarness-headless/
rsync -a --delete ~/src/agent-package/dist/dev.teamarness.agent/ target:.local/lib/teamarness-packages/dev.teamarness.agent/
rsync -a --delete ~/src/external-gateway-package/dist/dev.teamarness.external-gateway/ target:.local/lib/teamarness-packages/dev.teamarness.external-gateway/
rsync docs/headless/teamarness-headless.service.example target:.config/systemd/user/teamarness-headless.service
```

If `~/.config/systemd/user` does not exist on the target, create it first with `mkdir -p`.

### 3. Start the service

On the target:

```sh
systemd-analyze --user verify ~/.config/systemd/user/teamarness-headless.service
systemctl --user daemon-reload
systemctl --user enable --now teamarness-headless.service
loginctl enable-linger "$USER"
```

Lingering starts the service at boot and keeps it running after you log out.

### 4. Define the shell helpers

Paste these helpers into each shell that you use on the target. They hold the two paths, and `tm_req` looks up the current contract digest for you:

```sh
APP="$HOME/.local/lib/teamarness-headless/teamarness-headless"
DATA="$HOME/.local/share/teamarness-headless"
tm() { "$APP" --data-dir "$DATA" "$@"; }
# tm_req WORKSPACE PACKAGE OPERATION  (reads the JSON request on stdin)
tm_req() {
  local digest
  digest=$(tm package operations --workspace "$1" --package "$2" |
    python3 -I -c 'import json,sys; print(json.load(sys.stdin)["contractDigest"])') || return
  tm package request --workspace "$1" --package "$2" --operation "$3" --contract-digest "$digest" --input -
}
```

### 5. Name the Installation and install the Packages

```sh
tm identity set-name --name my-linux-box
tm status
```

Check that `readiness` is `ready`. Then inspect and install each Package. The install consents to native-code risks for filesystem, network, and process access:

```sh
for p in agent external-gateway; do
  dir="$HOME/.local/lib/teamarness-packages/dev.teamarness.$p"
  tm packages inspect --directory "$dir"
  printf '%s\n' '{"acceptedRisks":["filesystem","network","process"]}' |
    tm packages install --directory "$dir" --package "dev.teamarness.$p" --input -
done
```

Each install returns `"kind":"personal_installed"`. A Personal Package installs once per machine and runs in every Workspace that the Installation joins.

## Workspace tier

Repeat these steps for each Workspace.

### 6. Join the Workspace

In the desktop app, select **Copy Workspace Invitation**. Do not paste the Invitation anywhere else. On the Mac, send it straight to the target:

```sh
pbpaste | ssh target 'umask 077; cat > ~/teamarness-invite.txt; wc -c < ~/teamarness-invite.txt'
```

A complete Invitation is about 800 characters. If the count is 256 or lower, the Invitation was truncated. Copy it again.

On the target:

```sh
tm workspace join --person-name my-linux-box --invitation-file ~/teamarness-invite.txt
rm ~/teamarness-invite.txt
tm workspace list
```

Set `W` to the Workspace ID from the output, for example `W=aad43be122caa818738f763c373ba820`. Always select Workspaces by ID.

### 7. Configure the Agent

Each Workspace has its own Agent configuration: endpoint, model, system prompt, API key, and binding.

1. Create the Workspace Agent. People mention this name to get a reply:

   ```sh
   tm agent add --workspace "$W" --name Factory
   ```

   Set `AGENT` to the returned `id`.

2. Set the endpoint, model, and system prompt:

   ```sh
   printf '%s\n' '{"settings":{"endpointBaseUrl":"https://openrouter.ai/api/v1","model":"z-ai/glm-5.3-flash","systemPrompt":"You are a member of the team.","contextMessages":20}}' |
     tm_req "$W" dev.teamarness.agent agent.settings.set
   ```

3. Set the API key. The key stays out of argv, shell history, and logs:

   ```sh
   IFS= read -rs KEY
   printf '%s' "$KEY" | python3 -I -c 'import json,sys; print(json.dumps({"apiKey": sys.stdin.read()}))' |
     tm_req "$W" dev.teamarness.agent agent.key.set
   unset KEY
   ```

   The output is `{"hasKey":true}`.

4. Bind the Agent, then enable the Packages that the Agent can use. Bind first. Otherwise, the Package calls fail with `agent.binding_required`:

   ```sh
   printf '{"principalId":"%s"}\n' "$AGENT" | tm_req "$W" dev.teamarness.agent agent.binding.set
   for c in conversations documents; do
     printf '{"contract":"dev.teamarness.%s","enabled":true,"major":1}\n' "$c" |
       tm_req "$W" dev.teamarness.agent agent.packages.set
   done
   echo '{}' | tm_req "$W" dev.teamarness.agent agent.status.get
   ```

   The status is `"state":"ready"`. Mention the Agent name in a Conversation to test it.

### 8. Configure the External Gateway

The `tmx` client installs once per machine. The listener, connection, and tool selection belong to each Workspace. Each Workspace gets its own loopback port and its own token.

1. Inspect the setup. In the output, `cli.issue` must be `null`:

   ```sh
   echo '{}' | tm_req "$W" dev.teamarness.external-gateway external-gateway.setup.inspect
   ```

2. Install `tmx` if `cli.state` is `missing`, then connect. Each plan is previewed, applied, and then checked:

   ```sh
   gw_plan() {
     local plan
     plan=$(printf '{"intent":"%s"}\n' "$1" | tm_req "$W" dev.teamarness.external-gateway external-gateway.setup.preview |
       python3 -I -c 'import json,sys; print(json.load(sys.stdin)["planId"])') || return
     printf '{"planId":"%s"}\n' "$plan" | tm_req "$W" dev.teamarness.external-gateway external-gateway.setup.apply
     sleep 2
     echo '{}' | tm_req "$W" dev.teamarness.external-gateway external-gateway.setup.status
   }
   gw_plan install-cli   # first Workspace only
   echo '{"enabled":true}' | tm_req "$W" dev.teamarness.external-gateway external-gateway.listener.set
   gw_plan connect
   ```

   Each `setup.status` reports `"phase":"complete"`. If the phase is still `accepted`, `prepared`, `outputs`, or `committed`, run `setup.status` again.

3. Select Providers and tools. Nothing is selected by default. To select every eligible tool of a contract:

   ```sh
   gw_select() {
     printf '{"provider":"dev.teamarness.%s","enabled":true,"contracts":[{"contract":"dev.teamarness.%s","major":1}]}\n' "$1" "$1" |
       tm_req "$W" dev.teamarness.external-gateway external-gateway.package.set
     printf '{"contract":"dev.teamarness.%s"}\n' "$1" |
       tm_req "$W" dev.teamarness.external-gateway external-gateway.catalog.page |
       python3 -I -c 'import json,sys
   c=sys.argv[1]
   for t in json.load(sys.stdin)["tools"]:
       if t["eligible"]:
           print(json.dumps({"contract":"dev.teamarness."+c,"major":1,"provider":"dev.teamarness."+c,"port":t["port"],"fingerprint":t["fingerprint"],"enabled":True}))' "$1" |
       while read -r body; do printf '%s\n' "$body" | tm_req "$W" dev.teamarness.external-gateway external-gateway.tool.set; done
   }
   for c in conversations documents backlog; do gw_select "$c"; done
   ```

4. Test with `tmx` as the same account:

   ```sh
   ~/.local/bin/tmx workspaces
   ~/.local/bin/tmx --workspace "$W" call dev.teamarness.conversations channel.list --major 1 --json '{}'
   ```

Point external agents at `~/.local/bin/tmx --workspace <id>`, and give them `tmx --skill`. Calls through the Gateway act as this Installation's Workspace Person.

## Update a Package or the Host

To update a Package, build it on the build machine and copy it over the same directory. Then reload it. Settings, keys, bindings, and Package data stay:

```sh
rsync -a --delete ~/src/agent-package/dist/dev.teamarness.agent/ target:.local/lib/teamarness-packages/dev.teamarness.agent/
tm packages reload --package dev.teamarness.agent
```

To update the Host, stop the service, replace the Host directory with `rsync -a --delete`, then start the service. Do not delete files in `~/.local/share/teamarness-headless`.

## Check the Installation

| Check | Command |
|---|---|
| Service state | `systemctl --user status teamarness-headless` |
| Logs | `journalctl --user -u teamarness-headless -b` |
| Package instances | `tm status` |
| Agent state | `echo '{}' \| tm_req "$W" dev.teamarness.agent agent.status.get` |
| Gateway state | `echo '{}' \| tm_req "$W" dev.teamarness.external-gateway external-gateway.status.get` |

If a command reports `outcomeUncertain: true`, inspect the state before you do anything else. Do not repeat the command automatically. For details, see [Handle errors without replay](README.md#handle-errors-without-replay).
