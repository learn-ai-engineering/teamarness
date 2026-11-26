# Removed Packages

This file lists the bundled Packages that were removed from the repository and explains how to restore them. Each restore works for a single Package or for several Packages together.

## 2026-11-26 — Reference, Tutorial, and Repo-aware

**Why:** Don has enough real Packages now. These three were always hidden after first use, and they still cost build time and CPU in every `npm run build`, `cargo test --workspace`, and CI run.

**Removed in:** the commit that adds this file. Its parent is `4561259`, which is the last commit with all three Packages. To find the removal commit, run:

```sh
git log --diff-filter=D --oneline -- packages/tutorial/teamarness.package.json
```

| Package | Identity | Source directory | Cargo package | npm workspace |
| --- | --- | --- | --- | --- |
| Reference | `dev.teamarness.reference` | `packages/reference-package/` | `teamarness-reference-package` | `@teamarness/reference-package-ui` |
| Tutorial | `dev.teamarness.tutorial` | `packages/tutorial/` | `teamarness-tutorial-package` | `@teamarness/tutorial-package-ui` |
| Repo-aware | `dev.teamarness.repo-aware` | `packages/repo-aware/` | `teamarness-repo-aware-package` | `@teamarness/repo-aware-package-ui` |

### Restore everything

To restore all three Packages, revert the removal commit:

```sh
git revert <removal-commit>
npm install
```

The revert also reverses the test and doc retargeting described below. If later commits changed the same files, resolve the conflicts by keeping both sides.

### Restore one Package

Replace `<dir>` with a source directory from the table, for example `packages/tutorial`.

1. Restore the source tree:

   ```sh
   git checkout 4561259 -- <dir>
   ```

2. Add `<dir>/backend` to `[workspace] members` in the root `Cargo.toml`. Repo-aware also needs these `[workspace.dependencies]`, which were dropped because nothing else used them. Copy the exact lines from `git show 4561259:Cargo.toml`:
   - `gix`
   - `uuid`
   - `chrono`
3. In the root `package.json`, make these changes. `git show 4561259:package.json` has the original lines.
   - Add `<dir>/ui` to `workspaces`.
   - Add `npm --workspace <npm workspace> run typecheck` to the `typecheck` script.
   - Add `npm --workspace <npm workspace> run lint` to the `lint` script.
   - Add `npm --workspace <npm workspace> test` to the `test` script.
   - Tutorial only: add the `build:tutorial` script back.
   - Repo-aware only: add the `build:repo-aware` script back.
   - Reference only: add the `verify:reference-webkit-probe:prepare` script back.
4. Add `/<dir>/ui/dist/` to `.gitignore`.
5. Add the entry back to `packages/bundled-packages.json`, then regenerate the derived configuration:

   ```sh
   node scripts/bundled-packages.mjs --write
   npm run build:application-packages
   ```

   The first command rewrites the `bundle.resources` and `bundle.externalBin` entries in `apps/teamarness-desktop/src-tauri/tauri.conf.json`. The second rewrites `packages/application-packages.v1.json`.
6. Update the lockfiles:

   ```sh
   npm install
   cargo update --workspace
   ```

7. Add the identity back to the ordered list in the `delivery configuration` test in `scripts/package-foundation-qualification.test.mjs`. If you want the old style and contract checks back, also add the directory to `PACKAGE_UI_DIRECTORIES`, `BACKEND_DIRECTORIES`, and the author-tree list in the same file.

### Package-specific coverage that was removed or retargeted

Restore these only together with their Package.

**Reference**

- `crates/teamarness-package-host/tests/reference_package.rs` was deleted. It ran the Reference Backend to test three things: state and processes across bundled, local, and version restarts; that `reference.*` operations stay out of the Host and Tauri lifecycle interfaces; and that a Package-declared failure reaches the Host with its own code and details. DELTA-006 names this test as the live fixture for error fidelity. To restore it, run `git checkout 4561259 -- crates/teamarness-package-host/tests/reference_package.rs`. No other Package covers the error-fidelity case yet.
- `scripts/smoke-packaged-app.mjs` used to select the Reference Package and check its UI kit and signing round trip. It now waits for, selects, and checks the Backlog UI instead. The qualification test looks for the stage name `"bundled Backlog navigation"`.
- `docs/package-author-guide.md` showed the Reference Package's directory layout. It now uses a generic `teamarness-example-package` Backend name.

**Tutorial**

- `docs/build-your-first-package.md` began with the finished Tutorial. It now begins with Backlog. The guide phrases checked in `scripts/package-foundation-qualification.test.mjs` changed to match. The Tutorial `ui-contract.json` shape assertions were removed.
- `README.md` and `docs/local-package-registration.md` now name Backlog instead of Tutorial.
- These tests now use Backlog instead of Tutorial:
  - `crates/teamarness-package-host/tests/external_package_authoring.rs` (`backlog.draft.get`; its environment variable is now `TEAMARNESS_BUNDLED_PACKAGE_DIRECTORY`)
  - `scripts/create-package.integration.test.mjs`
  - `scripts/validate-packages.test.mjs`
  - `packages/teamarness-package-ui-sdk/bin/contract-types.test.mjs`
- `scripts/smoke-packaged-app.mjs` no longer has the Tutorial navigation, selection, and generate/save stages.

**Repo-aware**

- `context/02-system/06-repos/requirements.md` (`TEAM.SYS.REPO`) is retired, not deleted. Restoring Repo-aware makes those requirements active again.

### Left unchanged on purpose

- Several tests use the strings `dev.teamarness.reference` or `dev.teamarness.tutorial` only as sample Package Identities. These tests do not load the Packages, so the strings were kept. Examples: `public_api.rs`, `admission_matrix.rs`, contract fixtures, and the desktop `*.test.ts` files.
- `context/roadmap.md`, the ADRs, and the DELTA files still mention these Packages as history.
