# 02-system/06-repos — Repository Requirements

Prefix: `TEAM.SYS.REPO`

Repository integration is delivered by the Repo-aware Package (`dev.teamarness.repo-aware`).

## TEAM.SYS.REPO-R01 — Package-Owned Repository Integration

Repository registration, Git inspection, and repository views SHALL be owned by a Package. The Core Runtime, Package Host, and Shell SHALL contain no repository-specific behavior. The Repo-aware Package SHALL require no Semantic Capability.

## TEAM.SYS.REPO-R02 — Registration Validation

A user SHALL be able to register a local Git checkout by absolute path. The Package SHALL resolve the path canonically, SHALL require an existing directory inside a non-bare Git working tree whose HEAD has at least one commit, and SHALL register the working-tree root when given a subdirectory.

## TEAM.SYS.REPO-R03 — Registration Identity

Each registration SHALL receive a stable, Package-generated identifier and SHALL keep its canonical path. One Package Instance SHALL support multiple registrations. A second registration of the same canonical path SHALL be rejected.

## TEAM.SYS.REPO-R04 — Local Scope and Persistence

Registrations SHALL be stored in the Package Data Directory, local to one Installation and Workspace. They SHALL survive Backend recovery and application restart, SHALL be replaced atomically on change, and SHALL NOT be synchronized to peers.

## TEAM.SYS.REPO-R05 — Removal

A user SHALL be able to remove a registration after confirmation. Removal SHALL delete only the Package's registration and SHALL NOT modify the checkout.

## TEAM.SYS.REPO-R06 — Repository Snapshot

For a selected registration, the Package SHALL report the path, the origin remote URL when one is configured, a GitHub web URL when the origin is a recognized GitHub HTTPS or SSH remote, HEAD as a branch name or a detached commit, the HEAD commit identifier, whether the working tree is clean, and when the snapshot was taken.

## TEAM.SYS.REPO-R07 — Read-Only, Local Inspection

Repository inspection SHALL be local and read-only. It SHALL NOT fetch, contact remotes or hosting services, require credentials, or change repository files, refs, or configuration. GitHub detection SHALL use local remote configuration only.

## TEAM.SYS.REPO-R08 — On-Demand Refresh

The Package SHALL compute a snapshot when a registration is selected or the view opens, and when the user explicitly refreshes. It SHALL NOT poll or watch the filesystem in the background.

## TEAM.SYS.REPO-R09 — Failure Reporting and Isolation

The Package SHALL report distinct, stable errors for a missing path, a non-directory, a non-Git or bare repository, an unborn repository, a duplicate registration, an unknown registration, a previously registered checkout that is no longer available, and an interrupted snapshot. A failing registration SHALL NOT affect other registrations. A snapshot in progress SHALL stop when its request is cancelled.

## TEAM.SYS.REPO-R10 — No Repository Replication

Teamarness SHALL NOT replicate repository files, snapshots, registrations, or repository-derived views to other Installations. Git SHALL remain the only transport for repository content.
