# Stable releases and application update recovery

For a local preview and a short testing sequence, start with [Test application updates before a release](test-application-updates.md).

## Status and boundaries

The local release tooling and native consent flow are implemented. **Passing updater Verify and broad distribution qualification remain incomplete** until the Don/maintainer gates below are observed. On 2026-11-01, Don reported successful source checks, the consent/recovery preview, and the packaged update-probe run. He authorized integration and a controlled first stable `v0.1.0` release, manual installation, then a higher-version update test after release setup and hosted checks. He deferred rebuilding/smoking the ordinary local bundle; that observation is not a pass. This approval does not authorize bypassing failed checks or publishing probe/disposable-key artifacts. Fixtures, successful app/DMG builds and `CI=true` (Finder cosmetics only) are not hosted, Gatekeeper or installed-update evidence.

Source, Actions, releases and metadata use **one public repository**, `learn-ai-engineering/teamarness`. Discovery is anonymous at `https://github.com/learn-ai-engineering/teamarness/releases/latest/download/latest.json`; application URLs are pinned to `releases/download/vX.Y.Z/`. No client login, PAT or embedded credentials are required. Ordinary local use remains available offline. The first updater-enabled release must be installed manually; old binaries cannot acquire an updater remotely.

Only native Apple Silicon macOS is supported. Updater cryptographic signatures authenticate both bytes and version. macOS **ad-hoc signing** is not Developer ID signing or notarization; those, other targets, native menus/tray, per-Package updates and one-hour adoption remain deferred. No TUF freeze protection, threshold signing, automatic key revocation, transactional rollback or generic unsaved Package UI draft flushing is promised.

## Maintainer setup — separate deployment approval required

1. Review source **and repository history** for public disclosure, including old secrets/private material, before making the existing repository public. This tooling is not a disclosure audit and never changes repository settings.
2. Protect `main`: require the lightweight `checks` job, review/integration policy and prevent force pushes/deletion. Stable tag source must be reachable from protected `main`. Establish tag rulesets targeting `v*` that prevent tag updates/deletion, restrict creation to authorized maintainers, and have no routine bypass. Do not move a failed tag: correct source with a newer version.
3. Create protected environment **`release`**, restrict deployment refs to stable release tags, require maintainer approval, and review environment bypass/admin policy. Don explicitly selects a solo-maintainer exception for initial releases: `don-smith` reviews and approves his own release runs. Keep administrator deployment bypass disabled. A separate approver and prevention of self-approval remain the preferred multi-maintainer setup. Both signed-build and publish require approval. Use repository-scoped `GITHUB_TOKEN`; restrict Actions defaults to read-only. Do not grant untrusted PRs signing secrets or write tokens. No `pull_request_target` source execution is used.
4. After approval, generate the production updater key using the pinned CLI's `npm run tauri signer generate -- --help` and its encrypted-key/password options in a private offline location, **not** in source or a shared build output. Maintain an encrypted offline backup and prove recovery privately. Never paste keys/passwords into logs, release assets, workstream evidence, or chat. This implementation did not generate or authorize a production key.
5. Store the encrypted key as environment secret `TAURI_SIGNING_PRIVATE_KEY` and password as `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Set environment variable `TEAMARNESS_UPDATER_PUBLIC_KEY` to the reviewed public-key text (not a path). All shipped versions must retain this trust root; compare it independently before approving a build. Only the signing step receives private inputs. The public-key overlay is temporary and only the public key compiles into the app; there is no runtime trust-root override.
6. Review the exact action commits in the workflows for current advisories/availability before first production enablement. The repository uses JSON-subset YAML so contract tests parse the **actual workflow structure**, not a parallel policy. Checkout/setup-node use Node 24 action runtimes; artifact actions use Node 20 independently of the application Node pin. GitHub's `macos-15` image floats: every job records image version, OS, Xcode/SDK, Node/npm and Rust, and asserts native ARM. Do not claim a fixed image digest.

### Job permissions and transfers

| Job | Permission | Environment / secrets |
|---|---|---|
| PR/main Checks | `contents: read` | lightweight Linux checks; no environment or signing secrets |
| stable checks | `contents: read` | read-only preflight; complete check/build/default bundle + disposable artifact qualification |
| signed-build (needs checks) | `contents: read` | protected `release`; private key/password **only** in Build and sign step |
| publish (needs checks + signed-build) | `contents: write` | protected `release`; **no signing secrets**; public key and same-repository token only |

Checkout always uses event SHA with `persist-credentials: false`. No build caches are used. The signed build uploads exactly the four public files plus `qualification.json`; no signing files, runtime data, logs, executables, arbitrary `target/` directory or cache is transferred. Download uses only the named artifact from the **same workflow run**, not a supplied run ID or fork. Publisher rebuilds the pinned native verifier and Package validator from that same trusted checkout, never trusting an uploaded executable.

Publish uses one cross-version concurrency group, `teamarness-stable-publication`, with `cancel-in-progress: false`. GitHub can replace a pending run; concurrency is not a guaranteed queue. Rerun a dropped run if still needed; delayed/backfill versions fail the strict increasing-version check. Manual maintainers must not race or bypass this policy by changing tags/drafts/assets outside the workflow.

## Validation cadence

- During development, run focused checks for the change. `npm run check:fast` installs locked npm dependencies, builds the UI SDK, checks Rust formatting and source guards, and runs workspace lint/typechecks, release-tool unit tests and all UI workspace tests. It does not compile or test Rust, build native bundles, or qualify release artifacts.
- PRs and main pushes run `check:fast` on `ubuntu-24.04`, with pinned Node/Rust tools and read-only permissions. The required job remains named `checks`; branch protection stays enabled. A green main check is not full native release qualification. Rust/native failures can therefore reach main but must block a release.
- Pushes to `ci/**` run the same lightweight job. This lets a maintainer validate an exact commit before a protected direct-main fast-forward, without bypassing required checks. Delete the temporary validation branch afterward. This is not a publication route.
- A stable tag runs the complete native Apple Silicon checks, workspace/default bundle builds and disposable signed-artifact qualification once. Signing depends on that successful release-run gate. Production signing, same-run artifact transfers, authenticated readback and separate publication approval remain unchanged. No PR artifact or local test result substitutes for this gate.

## Prepare and qualify a release

Use exact **Rust 1.99.1**, **Node 24.20.0**, committed lockfiles and native `aarch64-apple-darwin`. Prefer latest compatible tooling deliberately, with actual checks; discuss concrete breakage rather than silently retreat or blanket-refresh dependencies.

Before changing versions or starting another build, inspect `target/release/distribution`. Qualification intentionally rejects a prior version's DMG as an unexpected asset. Preserve the previous version's outputs in a new private archive directory **outside** the distribution directory. For a completed `v0.1.0` qualification, archive only these explicitly named files:

```sh
mkdir -m 700 /private/path/teamarness-v0.1.0-distribution &&
  mv -i target/release/distribution/Teamarness_0.1.0_aarch64.dmg \
    target/release/distribution/Teamarness.app.tar.gz \
    target/release/distribution/Teamarness.app.tar.gz.sig \
    target/release/distribution/latest.json \
    target/release/distribution/qualification.json \
    /private/path/teamarness-v0.1.0-distribution/
```

Use the actual prior version and a new archive path; if an earlier attempt was incomplete, explicitly name only the known files present. Stop and investigate any other entries rather than deleting them or bypassing the qualifier's allowlist. Confirm the named files are preserved in the archive and distribution is ready for the new version before proceeding. Do not use wildcards, recursive cleanup, or place signing material in either asset directory.

The commands below are the complete local qualification path when needed; they are not a mandatory duplicate of the hosted release gate before every PR. For ordinary development, use focused checks or `npm run check:fast` instead. Hosted release checks still run in full.

```sh
node scripts/release/version.mjs set 0.1.1
node scripts/release/version.mjs check --tag v0.1.1
npm run check
cargo build --workspace --locked
CI=true npm run tauri build -- --bundles app,dmg
# Local disposable key, private temporary generation/output, removed even on failure:
CI=true node scripts/release/build.mjs --disposable --public-key-out /private/path/disposable-public-key.txt
```

The disposable wrapper builds the normal production configuration and qualifies the actual DMG/archive/signature and every Package. It retains only the public key at the supplied path. Never distribute this temporary trust root as production. The default local bundle is update-Disabled; the release configuration explicitly enables production policy and ad-hoc signing. Missing/malformed public-key configuration fails non-debug production builds. Do not re-sign or edit the final app: Backend signing occurs **before** Package digests are generated. No stripping/signature repair is allowed.

Commit the deliberate desktop-only version change through the normal approved source integration process. Independent Package versions must not change. On clean source, revalidate/requalify artifacts so `qualification.json` has the exact committed `sourceSha` and `workingTreeDirty: false`:

```sh
npm run verify:release-artifacts -- --directory target/release/distribution --tag v0.1.1 --public-key /private/path/disposable-public-key.txt
```

No local command above publishes. The qualifier accepts only `target/release/distribution`; `--collect` collects the exact build outputs and generates metadata, and optional `--notes FILE` permits at most 4 KiB of plain text. Without `--collect`, existing bytes are qualified again. It checks every Package/Backend, signatures, hashes/modes/ARM, manifest/version/identifier, safely extracted archive and readonly mounted DMG whole-tree correspondence. The pinned official updater verifies both macOS target entries and the signed announced version. Nonzero checks are fatal.

Public assets are exactly:

- `Teamarness_X.Y.Z_aarch64.dmg`
- `Teamarness.app.tar.gz`
- `Teamarness.app.tar.gz.sig` (complete signature text)
- `latest.json` (both `darwin-aarch64` and `darwin-aarch64-app`, same archive/signature)

`qualification.json` is a **CI-only** same-run source/digest record, never a public release asset. Signing files, logs and runtime data are forbidden in the distribution directory.

### Read-only validation / local fixture dry-run

```sh
node scripts/release/cli.mjs --help
npm run release:publish -- --tag v0.1.1 --public-key /private/path/public-key.txt --dry-run --fixture /private/path/github-fixture.json
```

The fixture is bounded JSON with fields `event` (`name: "push"`, `repository: "learn-ai-engineering/teamarness"`, `ref: "refs/tags/v0.1.1"`, exact commit `sha`, `deleted: false`), `repository` (`full_name`, `private: false`, `visibility: "public"`, `fork: false`), `main` (`protected: true`), `tagSha` (same commit), `ancestor: true`, and `releases` (GitHub-shaped release history; `[]` for first release). This adapter is structurally read-only and allowed **only** with `--dry-run`. It still requires clean source, exact record/digests and real native qualification of actual local files. It does not prove real GitHub protections, API access or hosting. Dirty-source records fail even in dry-run.

Without `--fixture`, dry-run reads the real CI event file/environment and `GITHUB_TOKEN`; it makes no GitHub mutations. `node scripts/release/cli.mjs preflight --tag vX.Y.Z` uses the same read-only policy before secrets/build. Only canonical stable tag pushes in the exact public non-fork repository, resolved to the clean checkout SHA with version agreement and protected-main ancestry, are accepted. `workflow_dispatch`, PR and fork events are not publication routes.

After separate integration/tag/publication approval, use the reviewed version/source on protected main, check it, create `git tag vX.Y.Z <approved-source-SHA>` and push only that tag. Those remote operations are **not authorized by this runbook alone**. Approve environment jobs only after inspecting source, checks, key identity and prerequisite manual evidence. The workflow invokes `npm run release:publish -- --tag "$TAG" --public-key "$RUNNER_TEMP/teamarness-public-key.txt"`; do not supply an arbitrary destination or use a PAT.

## Draft failures, reruns and immutable publication

A draft records exact `target_commitish` and public `Source-SHA` provenance. A same-SHA rerun may delete/replace **draft** assets, then uploads only the four files and downloads every byte with CI authentication. Digest, metadata, complete bundle/Package qualification and official signature verification must pass on the downloaded archive. The CI token is never forwarded to release storage redirect hosts. Missing/extra files, bad digests/signatures, wrong SHA or public destination fail closed.

Immediately before publication, the job rechecks resolved tag/SHA, protected-main ancestry, required asset identities/sizes, and **all paginated published non-draft/non-prerelease canonical stable versions**. The candidate must exceed their greatest version. Empty history is allowed. Publishing `draft: false`, `prerelease: false`, `make_latest: "true"` is the **final mutation**; nothing uploads or rewrites metadata afterward.

- Upload/readback/verifier failure leaves an unadvertised draft and previous latest unchanged. Inspect evidence; correct the defect rather than retry until green. Same-source transient draft failure may be deliberately rerun.
- Wrong-source drafts fail; investigate, do not rewrite provenance to force acceptance.
- Published assets/metadata are immutable by workflow policy. Same tag with different bytes/SHA cannot overwrite. Use a strictly higher version for corrections, never backfill or downgrade latest.
- If the final response was lost, a rerun reads and fully qualifies matching already-published bytes and returns `already-published` **without mutations**. Newer releases do not make an exact response-loss recognition overwrite anything.
- On key loss, recover the encrypted backup privately. Without it, old trust roots cannot accept a newly signed key. A planned trust-root transition needs an old-key-authenticated release when possible and separate review. On compromise, halt publication, protect/rotate CI credentials and investigate; manual reinstall from a separately authenticated new trust root may be necessary. No automatic revocation or compromise recovery is promised.

## Manual bootstrap, terminal recovery and A→B qualification

**Record each gate separately.** Don reports the local preview and packaged update-probe completed on 2026-11-01. The subsequent ordinary local bundle rebuild/smoke is deferred by his decision to proceed to controlled hosted release and installation testing. Remaining release-style, hosted, Gatekeeper and installed A→B observations are pending, not inferred from the probe. Local green is not AC2 public publication or AC7 installed continuity. Do not delete an existing data directory to repair an update; use FileVault and encrypted backups. Before testing with an existing Installation, make and privately verify an encrypted backup. Evidence must not include identity values, invitations, database dumps, user paths or keys.

1. **H1, Don:** Run `npm run verify:packaged-app` from an Accessibility-authorized terminal on final ordinary and release-style bundles. Record every bundled Backend launch and graceful process-group exit. Build/run the disposable Ready-state isolation probe exactly as README describes (`--features ui-isolation-probe --config src-tauri/tauri.probe.conf.json`, then `npm run verify:packaged-app -- --update-probe`); require all four Package-origin update attempts rejected with zero handler entry and positive main control. Observe keyboard access, Later/retained affordance, Updating/recovery layout. Probe/default Disabled fixtures alone are not real WKWebView evidence. Rebuild ordinary artifacts afterward.
2. **H2, maintainer/Don after setup and separate deployment approval:** Exercise a nonsecret PR/fork path and inspect actual permission/secret isolation. Deliberately approve trusted stable-tag build/publish. Confirm anonymous feed, DMG/archive/signature, exact source/target/version, stable/latest status and version-pinned URLs. A failed draft must leave previous latest unchanged. Do not corrupt an advertised release to test failure. Controlled qualification publication is necessary for the following real public-feed test, not proof already obtained.
3. **H3, Don:** With two strictly increasing approved public versions A and B under the same production trust root, download A's DMG, open it, copy `Teamarness.app` to a writable installed location and launch **the copy**, not the mounted DMG or a translocated app. Record actual Gatekeeper/quarantine behavior; ad-hoc signatures do not eliminate it. Do not disable system protections globally. Use disposable private data and encrypted backup, never a valuable Installation.
4. Persist a Workspace and representative content in Reference/Conversations and Documents where supported. Compare identity/Workspaces/content locally without dumping values. Run the scenario once with default data selection and again with an explicit absolute `--data-dir`. Record old app digest and all Backend PIDs/process groups.
5. Publish B only with approval. In A, observe launch acquisition plus one actual due periodic/resume check (six-hour policy; unobserved intervals are not passes). Ready follows verification, not a progress callback. Choose Later; keep using Workspace/Package controls, retain a ready affordance, and verify unchanged bundle digest/no restart. Quit and relaunch once: pending bytes are session-only and must verify again.
6. Choose Restart now, knowing unsaved UI-only edits may be discarded. Observe frames removed and Updating rendered **before** terminal work. Require old app/Backend groups gone, B relaunched with original args/data selection, identical Installation Identity/Workspaces/persisted Package content and fresh healthy Backends. Reopen B and verify persisted content again. Known unrelated Documents deltas remain separate, not relabeled as updater passes.
7. Launch offline: normal use must work and background failures stay nonmodal. On a disposable read-only/unwritable location, require Disabled or honest terminal recovery, never success. Deterministic failure tests are not permission observations.

Before consent, offline/host/transfer/signature failures leave the running app usable; Check again is coalesced/rate-limited. After consent, cleanup uncertainty/timeout or installation failure cannot return to Running. Use recovery Quit only when owned cleanup/install settles; do not retry installation in place. If a preserved old-bundle location is shown, inspect it locally without logging paths. Download a trusted complete DMG and manually reinstall into a writable location; keep the existing data directory. If replacement succeeded but relaunch failed, manually open the installed app with the original `--data-dir` selection. There is no promised automatic rollback. Escalate any data/process/isolation failure before general distribution or passing Verify.
