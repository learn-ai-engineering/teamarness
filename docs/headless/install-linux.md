# Install headless Teamarness on Linux

Use this guide to set up a headless Teamarness Installation on a Linux machine. The Installation runs as a user service, joins your Workspaces, and synchronizes with the desktop app and other Installations. The [operator guide](README.md) explains each command and its error policy in full.

> [!NOTE]
> This guide is verified on Ubuntu 26.04 x86_64 with systemd 259. Other Linux distributions that meet the requirements below are expected to work, but they are not tested yet.

Setup uses an answers file:

1. Print a template.
2. Fill in the template.
3. Run setup with the answers file.

Setup reads the current state before each step, so you can safely run it again. It also adds Workspaces and applies Host updates.

To add Personal Packages, such as an Agent, follow each Package's own headless guide after you finish this guide.

## Requirements

- Linux on x86_64 with GNU libc (glibc). musl-based distributions are not supported.
- systemd with user services and lingering.
- Python 3.11 or later, and rsync.
- One unprivileged account that runs the Installation and any local agent tools. Do not run setup or the Installation as root.
- The Teamarness desktop app on another machine, to create Workspace Invitations.

## 1. Get the distribution

You need two things side by side:

- The headless distribution directory for `x86_64-unknown-linux-gnu`. It contains `teamarness-headless` and `packages/`.
- The `teamarness-headless-setup` script.

> [!NOTE]
> No release archives exist yet. Until they do, build both on a Linux x86_64 machine. See [Build one native application](README.md#build-one-native-application). The build writes `target/teamarness-applications/headless/x86_64-unknown-linux-gnu/` and `target/teamarness-applications/headless/teamarness-headless-setup`. Copy both to the machine, for example with `rsync -a`. Update this guide when release archives are available.

## 2. Create the answers file

```sh
./teamarness-headless-setup --print-template host.toml
```

Open `host.toml` and set the two required values:

- `distribution_dir`: the absolute path of the distribution directory.
- `installation_name`: the name this Installation shows in Workspaces.

The other values have working defaults. The template explains each one.

## 3. Add a Workspace Invitation (optional)

To join a Workspace during setup, select **Copy Workspace Invitation** in the desktop app. Do not paste the Invitation anywhere else. Send it straight to the Linux machine as a private file. From a Mac:

```sh
pbpaste | ssh my-linux-box 'umask 077; cat > ~/teamarness-invite.txt'
```

In `host.toml`, set `invitation_file = "~/teamarness-invite.txt"` under `[join]`. Setup checks that the Invitation is complete before it changes anything, and it deletes the file after a successful join.

## 4. Run setup

```sh
./teamarness-headless-setup --answers host.toml
```

Setup prints one line for each item: `changed`, `unchanged`, or `stopped`. It ends with the joined Workspaces and a summary:

```text
changed: install /home/you/.local/lib/teamarness-headless (installed)
changed: unit /home/you/.config/systemd/user/teamarness-headless.service
changed: service enabled and started
changed: linger enabled (starts at boot, survives logout)
unchanged: readiness ready
changed: installation name my-linux-box
changed: joined Workspace team (aad43be122caa818738f763c373ba820); invitation file deleted
changed: helpers /home/you/.config/teamarness-headless/helpers.sh
workspace: team aad43be122caa818738f763c373ba820
summary: 7 changed, 1 unchanged
```

Keep `host.toml`. It holds no secrets.

Load the operator helpers in each shell that you use for operator commands and Package guides:

```sh
source ~/.config/teamarness-headless/helpers.sh
```

`tm` runs the operator CLI against this Installation. `tm_req WORKSPACE PACKAGE OPERATION` sends one Package request, with the JSON request on stdin.

## Join another Workspace

Save a new Invitation to the `invitation_file` path, then run setup again. Setup reports `unchanged` for everything else.

## Update the Host

Get the new distribution, set `distribution_dir` to it, and run setup again. Setup stops the service, replaces the installed files, and starts the service again. The Installation identity, memberships, and Package data in `data_dir` stay.

## If setup stops

| Output | Meaning and action |
|---|---|
| `stopped: answers …` (exit 2) | An answers value is invalid. Fix it and run setup again. |
| `stopped: permissions …` | A directory above the install location is group- or other-writable and is not yours. Ask an administrator. Setup fixes directories that you own. |
| `stopped: … outcome unknown — inspect with: …` (exit 1) | A change might have happened. Run the inspect command that setup prints, then run setup again. Setup never retries by itself. |
| `stopped: readiness not ready …` | The service did not become ready. Read the logs with `journalctl --user -u teamarness-headless -b`. |

## What setup does

Setup runs these steps in order. Each step changes something only when the current state differs:

1. Checks Python, rsync, systemd, the account, and the distribution.
2. Creates missing directories with mode 0700. Removes group and other write access from directories you own above the install, data, and configuration locations. Trusted native code runs from there, and some Packages refuse group-writable ancestors.
3. Copies the distribution to `install_dir`. If the installed copy differs, it applies the copy as an update.
4. Writes the [user unit](teamarness-headless.service.example), then enables and starts the service.
5. Enables lingering, so the service starts at boot and keeps running after logout.
6. Waits until the Installation reports `ready`.
7. Sets the Installation name.
8. Joins the Workspace from `invitation_file`, then deletes the file.
9. Writes the operator helpers.

The [operator guide](README.md) documents the equivalent manual commands.

## Check the Installation

| Check | Command |
|---|---|
| Service state | `systemctl --user status teamarness-headless` |
| Logs | `journalctl --user -u teamarness-headless -b` |
| Workspaces | `tm workspace list` |
| Package instances | `tm status` |
