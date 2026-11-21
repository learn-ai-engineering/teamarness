# Linux deploy findings, 2026-11-21

This page records friction from the first real Linux deploy: Host 72cb5f2, Agent 1.0.0 (later 1.0.1 for the update test), and External Gateway 1.1.0. They were deployed on factory2 (Ubuntu 26.04.1 x86_64, systemd 259) and joined to a desktop Workspace. Each item is a candidate for later work, not a commitment.

## What passed

- Native Host build and smoke, a copied install, the systemd user service, and lingering.
- Workspace join from a desktop Invitation.
- Agent replies in Conversations and in Document Comments, including Document edits.
- External Gateway: `tmx` install, loopback listener, connect, all tools selected. An external agent created a Message, Document, Ticket, Exhibit, and Comment.
- Reboot: the service started without a login, and identity, membership, Agent key and binding, and Gateway connection and port all stayed.
- Offline catch-up: changes made on desktop while the service was stopped synced after restart. The Agent answered a mention it missed.
- Package update: copy over the same directory, then `packages reload`. Settings, key, binding, and Package data stayed.
- Multiple Workspaces: no reinstall was needed. Configuration was per Workspace, and data stayed isolated between Workspaces.

## Install experience

| Finding | Suggested follow-up |
|---|---|
| No prebuilt Linux artifacts exist. Each target needs a build machine with toolchains. | Build Linux releases on factory1 or factory2 as self-hosted GitHub Actions runners. Check fork-PR risk for public repositories first. |
| The Agent and Gateway have no release archives. | Add Package release archives, so that an install becomes "download, copy, reload". |
| Operator requests need a manual contract-digest lookup. | `install-linux.md` uses a shell helper for now. Consider an operator option that resolves the current digest. |
| A group-writable `~/.local/lib` blocked the Gateway's `tmx` install (`output_unsafe`). | Documented in `install-linux.md`. The Gateway's onboarding guide must state the rule for directories above the Package directory. |
| A small user-quota `/tmp` failed the smoke test copy with `EDQUOT`. | Documented (`TMPDIR`). The smoke script could report the cause. |
| A desktop Invitation was truncated to 256 characters during a clipboard transfer. The join refused it as `invalid_input`. | The cause is unknown. The desktop copy path writes the full value. Consider a clearer "truncated invitation" error. |

## Agent Package

| Finding | Suggested follow-up |
|---|---|
| The API key, model, and settings are per Workspace. This is intended. | No change. Document it as a deliberate choice. |
| Before binding, `agent.packages.get` fails with `agent.binding_required`, and the outer error is marked uncertain. | Documented in the Agent guide (97a1caa). |
| After an outage, the Agent answers every mention it missed. | Decide whether a long outage should produce a burst of late replies. |
| A second Agent Package on one Installation needs a distinct Package Identity, because a registration with the same identity replaces the first. | Decide between several identities and several bound Agents in one Package. |

## External Gateway

| Finding | Suggested follow-up |
|---|---|
| `message.post` returns no Message ID, so an uncertain post can only be reconciled by matching content. | Provider change in Conversations. |
| An unknown port name returns `forbidden`, not "unknown". | Consider a distinct code, or guidance in the skill. |
| `tmx schema` output is large (about 34 KB for four contracts). | Consider a summary mode. |
| The guide does not say what to pass on the first Exhibit draft. | Guide text in Exhibits. |
| `#general` has the same all-zero ID in every Workspace. A wrong `--workspace` value writes silently to the wrong Workspace. | Tell agents to select Workspaces by ID. |
| Tool selection is per Workspace. | Documented in `install-linux.md`. |
