# Test application updates before a release

Use native Apple Silicon macOS with Node 24.20.0 and Rust 1.99.1. Run commands from the repository root. Use disposable data, not your everyday Installation.

## 1. Check the integrated source

```sh
npm run check
cargo build --workspace --locked
```

Stop on failure and retain the result. Do not treat a later focused pass as a full-check pass. The unresolved Host deadline failure is tracked in [DELTA-022](../context/.delta/DELTA-022-host-deadline-qualification-failure.md).

## 2. Preview the consent and recovery screens locally

No push, public release, or production signing key is needed for this preview.

Build the disposable probe:

```sh
CI=true npm run tauri build -- --features ui-isolation-probe --config src-tauri/tauri.probe.conf.json --bundles app,dmg
```

Launch it with a new data directory:

```sh
probe_data="$(mktemp -d "${TMPDIR:-/tmp}/teamarness-update-preview.XXXXXX")"
target/release/bundle/macos/Teamarness.app/Contents/MacOS/teamarness-desktop --data-dir "$probe_data"
```

Check the visible behavior:

1. Find **Restart now** and **Later**. The candidate notes identify a disposable isolation probe.
2. Choose **Later**. Use the Workspace and Package controls. Confirm that **Update ready** remains available.
3. Open **Update ready**. Use the keyboard to reach the consent controls.
4. Choose **Restart now**. Package frames must disappear as the Shell shows **Updating Teamarness**. The probe then deliberately enters recovery instead of installing or restarting. The Updating screen may be brief.
5. Check that recovery offers guidance and **Quit**, not normal Workspace controls or another installation attempt. Choose **Quit**.

The probe starts with a small, genuinely signature-verified fixture. It does not download a new application. Its native adapter refuses installation, so **recovery here is expected**, not an updater failure. See [the probe's boundaries](../README.md#application-update-consent-and-frame-isolation-probe).

## 3. Run the packaged isolation smoke

Close the preview first. From an Accessibility-authorized terminal, run:

```sh
npm run verify:packaged-app -- --update-probe
```

The script creates its own disposable data. It checks packaged startup, Backend cleanup, and hostile Package-frame attempts to call update commands. Those attempts must not enter native update handlers. Main-Shell consent must reach the deliberately blocked installer once, followed by recovery Quit. A permission error or timeout is not a pass.

After the probe, rebuild the ordinary app so the output no longer contains test instrumentation:

```sh
CI=true npm run tauri build -- --bundles app,dmg
npm run verify:packaged-app
```

Ordinary local builds have production updates **Disabled**. Do not distribute the probe. `CI=true` skips Finder DMG cosmetics only.

## 4. Qualify signed artifacts without publishing

Follow [Prepare and qualify a release](releasing.md#prepare-and-qualify-a-release) for the disposable-key build and actual archive, signature, DMG, and Package checks. Inspect and preserve any existing distribution outputs before building. The fixture publication dry-run makes no GitHub changes.

These checks do not prove replacement of an installed app.

## 5. Test a real installed A→B update separately

The current production client uses a fixed public GitHub feed. There is no supported local-feed switch. A private end-to-end rehearsal needs separately designed test configuration; the probe is not that configuration.

For the existing production path, obtain separate publication approval and follow [Manual bootstrap, terminal recovery and A→B qualification](releasing.md#manual-bootstrap-terminal-recovery-and-ab-qualification). That procedure uses two increasing releases under the same trust root. It does not require the first publication merely to run steps 1–4 above.

Use a copied, writable installation and disposable persisted content. Check **Later** leaves the bundle unchanged. After **Restart now**, require version B, no surviving old Backends, the same Installation Identity, the same Workspaces and persisted content, and the original data-directory selection. Unsaved UI-only drafts are not guaranteed to survive. Never delete a data directory to repair an update.

Record each manual result as pass, fail, or not run. Keep identity values, keys, and private data out of the evidence. Local success does not establish hosted permissions, anonymous downloads, Gatekeeper behavior, or installed continuity.
