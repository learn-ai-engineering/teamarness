# Install headless Teamarness on Linux

Use this guide to set up a headless Teamarness Installation on a Linux machine. The Installation runs as a user service, joins your Workspaces, and synchronizes with the desktop app and other Installations. The [operator guide](README.md) explains each command and its error policy in full.

> [!NOTE]
> This guide is verified on Ubuntu 26.04 x86_64 with systemd 259. Other Linux distributions that meet the requirements below are expected to work, but they are not tested yet.

Setup has two tiers:

- **Machine tier, once per machine:** install the Host, start the service, and name the Installation.
- **Workspace tier, once per Workspace:** join the Workspace.

To add Personal Packages, such as an Agent, follow each Package's own headless guide after you finish this guide.

## Requirements

- Linux on x86_64 with GNU libc (glibc). musl-based distributions are not supported.
- systemd with user services and lingering, to run the Installation as a service. Without systemd, you can still run `serve` in the foreground.
- One unprivileged account that runs the Installation and any local agent tools. Do not run the Installation as root.
- The headless distribution for `x86_64-unknown-linux-gnu`. This is a directory that contains `teamarness-headless` and `packages/`.
- The Teamarness desktop app on another machine, to create Workspace Invitations.

> [!NOTE]
> No release archives exist yet. Until they do, build the distribution yourself on a Linux x86_64 machine. See [Build one native application](README.md#build-one-native-application). Update this guide when release archives are available.

## Machine tier

### 1. Install the Host

Create private directories:

```sh
umask 077
mkdir -p ~/.local/lib ~/.config/systemd/user
chmod go-w ~ ~/.local ~/.local/lib
```

Trusted native code runs from these directories. Some Packages refuse to run when a directory above their installed files is group- or other-writable. Distributions with a default `umask` of `002` often create such directories, so `chmod go-w` removes that access.

Copy the complete distribution into place. Do not use symlinks, and do not edit files after the copy. In this example, `DIST` is the distribution directory:

```sh
rsync -a --delete "$DIST"/ ~/.local/lib/teamarness-headless/
~/.local/lib/teamarness-headless/teamarness-headless --version
```

### 2. Start the service

Copy the [example user unit](teamarness-headless.service.example) to `~/.config/systemd/user/teamarness-headless.service`. Then enable it and lingering:

```sh
systemd-analyze --user verify ~/.config/systemd/user/teamarness-headless.service
systemctl --user daemon-reload
systemctl --user enable --now teamarness-headless.service
loginctl enable-linger "$USER"
```

Lingering starts the service at boot and keeps it running after you log out. Your administrator's policy can restrict lingering.

### 3. Define the shell helpers

Paste these helpers into each shell that you use for operator commands. `tm` runs the operator CLI against this Installation. `tm_req` sends one Package request and looks up the current contract digest for you. Package guides use both helpers.

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

### 4. Name the Installation

```sh
tm identity set-name --name my-linux-box
tm status
```

Check that `readiness` is `ready`.

## Workspace tier

Repeat this step for each Workspace.

### 5. Join the Workspace

In the desktop app, select **Copy Workspace Invitation**. Do not paste the Invitation anywhere else. Send it straight to the Linux machine as a private file. From a Mac:

```sh
pbpaste | ssh my-linux-box 'umask 077; cat > ~/teamarness-invite.txt; wc -c < ~/teamarness-invite.txt'
```

A complete Invitation is about 800 characters. If the count is 256 or lower, the Invitation was truncated. Copy it again.

On the Linux machine:

```sh
tm workspace join --person-name my-linux-box --invitation-file ~/teamarness-invite.txt
rm ~/teamarness-invite.txt
tm workspace list
```

The Installation joins as a Workspace Person. Use the Workspace ID from `workspace list` in later commands and Package guides. Always select Workspaces by ID, not by name.

## Update the Host

1. Stop the service: `systemctl --user stop teamarness-headless`.
2. Replace the distribution: `rsync -a --delete "$DIST"/ ~/.local/lib/teamarness-headless/`.
3. Start the service: `systemctl --user start teamarness-headless`.

Do not change or delete files in `~/.local/share/teamarness-headless`. That directory holds the Installation identity, memberships, and Package data.

## Check the Installation

| Check | Command |
|---|---|
| Service state | `systemctl --user status teamarness-headless` |
| Logs | `journalctl --user -u teamarness-headless -b` |
| Workspaces | `tm workspace list` |
| Package instances | `tm status` |

If a command reports `outcomeUncertain: true`, inspect the state before you do anything else. Do not repeat the command automatically. For details, see [Handle errors without replay](README.md#handle-errors-without-replay).
