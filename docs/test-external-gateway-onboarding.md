# Verify External Gateway onboarding

**This is the single step-by-step manual verification procedure.** Follow this file. You can tell the assistant what you observe as you go; it will capture your results in the workstream's `verify/manual-onboarding.md`. You do not need to follow or maintain a second checklist. The results sheet contains no alternative walkthrough.

**Status: Don accepts the setup UI and section-local notifications on G `7df368f`. He reports Rotate credentials → Disconnect → Reconnect, then an external-agent 32/32-port smoke pass. Fresh full automation passes (322 Rust, 92 UI, 3 assembly tests, 17/17 real-Host cases), and fresh cumulative three-lane source review passes with no retained findings. Results and evidence limits remain in the workstream's `verify/manual-onboarding.md` and `verify/20261119T010430Z_ui-final-validation.md`. Packaged native/isolation smoke and unreported detailed manual subchecks remain incomplete; this is not an overall Verify pass.** You can begin the functional walkthrough in your current macOS account with a fresh app profile. No new account or app uninstall is required. The existing smoke blocker does not prevent these manual UI and CLI checks.

## First pass

Follow this sequence. You do not need to complete every advanced scenario before reporting your first results.

1. [Check shared user-local files](#check-shared-user-local-files-first).
2. [Launch or resume the test profile](#launch-the-new-build-with-a-fresh-profile).
3. Complete [M1 native setup](#m1--native-setup-without-a-source-build), then inspect [M5 Configuration and the gear](#m5--actual-revised-a--configuration--focus).
4. Try one representative agent read, write, and observation from [M3](#m3--actual-external-agent-session-not-a-scripted-substitute).
5. Test rotation, revocation, and disconnect for the disposable Workspace from [M4](#m4--updates-movement-ports-and-idle-authority).
6. [Clean up the disposable test](#clean-up-the-disposable-test).

In the results sheet, mark each check PASS, FAIL, BLOCKED, or PENDING and add a short observation. If anything is confusing, missing, or fails, record where you stopped. Leave untested subchecks pending. Return to M2 and the advanced M4/acquisition cases later.

## Check shared user-local files first

A fresh `--data-dir` isolates Installation identity, Workspaces, Package registrations, server-side policy and tokens, and test content. It does **not** isolate these external-client outputs:

| Location | Shared across app profiles in your account |
|---|---|
| `~/.local/bin/tmx` | Managed CLI executable |
| `~/.local/share/teamarness/external-gateway/` | Client credentials, connection records, receipts, and setup recovery state |
| Selected `<folder>/teamarness/` | Skill and its ownership receipt |

Package activation can inspect this shared state and refresh a recognized command owned by the same Installation. A different Installation requires explicit consent to take update ownership. Do not assume that opening another profile makes all home-directory access private.

Run this read-only metadata check before activating the test Package. It neither prints credentials nor creates files.

```sh
for path in "$HOME/.local/bin/tmx" "$HOME/.local/share/teamarness/external-gateway"; do
	if [ -e "$path" ] || [ -L "$path" ]; then
		stat -f '%Sp %N' "$path"
	else
		printf 'Absent: %s\n' "$path"
	fi
done
```

- If either location exists, pause to identify its use and ownership from secret-free metadata. Do not dump connection files, move existing state, or approve ownership transfer, downgrade, or repair just to get a test through. Existing unrelated or modified commands are designed to be refused, not overwritten. If you use the existing CLI, agree on a coexistence plan before applying setup.
- If both locations are absent, record that baseline. Applying setup will deliberately create user-local files for this test. Use only a new disposable Workspace and a disposable skill folder.

## Launch the new build with a fresh profile

**Create one test profile, then reuse it for the whole verification.** “Fresh” means the first launch, not every session. A new `--data-dir` creates another Installation and makes you repeat Workspace and Package setup, while the CLI and client store remain shared.

If you have already started, keep using the exact profile path recorded in the results sheet. Do not switch to the first-launch path below. To reopen an existing test, quit its running app yourself, then run:

```sh
APP='/Users/don/orca/workspaces/teamarness/external_gateway/target/release/bundle/macos/Teamarness.app'
PROFILE='/replace/with/your/recorded/test-profile-path'
"$APP/Contents/MacOS/teamarness-desktop" --data-dir "$PROFILE"
```

Do not delete the skill, CLI, client store, or profile between sessions. To add a skill to another folder, use **Install skill…**, not another Connect. The revised setup separates CLI installation, Workspace credentials, and optional skill installation. Local deletion cannot revoke app-side access.

For a first launch only:

1. Keep your installed app and its ordinary data. Save any work and temporarily quit that app yourself while testing. This avoids the known same-name accessibility-driver ambiguity. Do not uninstall it or change window-manager or security settings.
2. Use the built production app and Package below, not the fixture examples. For an acquired-artifact check, substitute the acquired paths and record the transfer route. The results sheet links the latest Verify artifact hashes. Do not use historical pre-correction Phase-8 hashes.

   ```sh
   APP='/Users/don/orca/workspaces/teamarness/external_gateway/target/release/bundle/macos/Teamarness.app'
   PKG='/Users/don/hypr/ai-course-root/teamarness-local-packages/external-gateway/dist/dev.teamarness.external-gateway'
   PROFILE="$HOME/Library/Application Support/Teamarness Verification/external-gateway"
   mkdir -p "$PROFILE"
   printf 'Reuse this test-profile path for every session: %s\n' "$PROFILE"
   "$APP/Contents/MacOS/teamarness-desktop" --data-dir "$PROFILE"
   ```

   This dedicated test path persists across sessions. If it already exists, the command resumes its data rather than creating a fresh Installation. Keep using it until verification and explicit cleanup are complete. Launch the executable with this explicit `--data-dir`, rather than opening an app icon that could reuse the ordinary running instance. Do not change `$HOME` to simulate another account.
3. In the test app, create a **new disposable Workspace** and record its ID as `WS`. Do not join your real Workspace or import your ordinary profile. Install the production Package through the Personal Package UI and its filesystem, network, and process trust consent.
4. In Agent setup, install the CLI if absent. If a recognized older/different installation needs an update, click **Update CLI** once. It applies immediately; there is no second confirmation. A rebuilt development Package can need **Update CLI** even when its version number is unchanged; this updates the executable, not Workspace credentials. Unknown or modified command files must still be refused, not overwritten. If a recognized interrupted CLI publication asks for Reload, restore storage and Reload the Package; do not delete the executable or its receipt as a workaround. There is no CLI uninstall action. No Agent Package or Binding is needed.
5. Enable the listener and only representative exact tools, then **Connect** this disposable Workspace. CLI installation alone must not create credentials, enable the listener, or select tools. Choose a disposable agent project and a supported skill destination, not your existing global skill folder. **Install skill…** opens the native chooser; confirming the folder installs the skill directly, while Cancel writes nothing. Skill installation must not create Workspace credentials or silently install/update the CLI. Create disposable native Backlog Epic and Documents Collection prerequisites when their workflow needs them.
6. Fill in the session details in the workstream's `verify/manual-onboarding.md`. Add results there as you follow the checks below. Record actual app, Package, OS, and harness versions and any artifact hashes checked. Never record bearers, connection-file contents, invitations, private keys, or secret screenshots. A current-account pass is not a clean-home or acquired-artifact pass.

## M1 — Native setup without a source build

- Open Gateway main view and Agent setup. Confirm CLI, Workspace connection, and optional skill are distinct. After CLI installation, select **Install skill…** and choose an empty folder supported by the intended harness (not an invented universal discovery folder). Confirm the native chooser opens. Cancel once: no skill write or credential creation. Open it again and leave it browsing **over 30 seconds** while another view/Workspace and gateway status/read remain responsive. Confirming the chooser installs the skill, so do not confirm until ready for that write. Record focus, title-bar and return-focus observations; do not diagnose AeroSpace from appearance.
- With a pending chooser, close its view and repeat with the **test Package's** Pause and Reload controls. Observe helper exit and no late selection. Parent-only process death is an advanced check: use only the confirmed test-profile process, or leave it pending. Never kill the ordinary app or an unrelated process.
- Inspect the CLI installed state/path and the persistent Workspace connection state independently of dismissible operation messages. **Connect**, credential rotation, and disconnect are direct named actions; routine Preview, approval-checkbox, and Apply screens are gone. Confirm the skill chooser once and observe actual completion, not acceptance alone. CLI, connection and skill progress/errors/results must appear within their own section above its action buttons; successful CLI updates stay in the CLI section even when the update button disappears. Dismiss a result with its X without hiding current installed/connected state. Separate **command installed / skill installed / access configured** outcomes must remain truthful. Record the absolute command path as `CLI="$HOME/.local/bin/tmx"`. Use it without editing shell startup files or relying on PATH. Run these commands without building source or invoking a language runtime:

  ```sh
  CLI="$HOME/.local/bin/tmx"
  "$CLI" --help
  "$CLI" --version
  "$CLI" --licenses
  "$CLI" workspaces
  WS='the-recorded-workspace-id'
  "$CLI" --workspace "$WS" status
  ```

  Confirm correct Package 1.1.0/CLI major 1 and no build/runtime prerequisite in this journey. Existing developer tools on your machine do not establish a clean-machine test. Offline listing is last-observed local metadata, not a liveness claim.

## M2 — Consent, interrupted observation, files and recovery

- Cancel the native chooser: no effects from that canceled action. Install/Update CLI is itself the deliberate action and has no second confirmation. A valid unfinished setup may need its original CLI/skill outputs. **Repair** can then offer **Confirm repair** / **Cancel repair** for that exact accepted work; it must not direct you to an independent CLI action blocked by the same pending setup. Start Connect once, immediately close/remount the UI, and require accepted work to continue with truthful current state and no duplicate managed token. Repeat an interrupted/lost observation and owned process restart, then inspect status before any new action. Do not blindly repeat Connect to discover whether it worked. Never claim interrupted publication rolled back.
- Edit only the disposable installed skill, select that destination again, and verify refusal and preservation. Choose a new empty folder to continue. A destination changed between selection and publication must fail safely, not receive writes through a stale identity. The exact narrow race may remain automated-only; do not invent a manual preview screen to test it.
- **Advanced, not required for the first pass:** test an unrelated `tmx` collision only when preflight confirms the fixed command path is unused. Never move, replace, or modify a working CLI for this experiment. Use a separate account if safe collision state cannot be arranged.
- **Advanced:** exercise duplicate-profile refusal only with the disposable test Workspace. A second app data directory still shares the user-local store. Do not join a real Workspace or transfer a working connection for this test. Disconnect the original test connection before deliberate transfer. If its profile is unavailable, leave manual record removal pending until the exact record is identified safely. Local file removal cannot revoke the old token. Never delete the whole store or policy as recovery.
- Inspect directory/file permissions without printing contents (`stat -f '%Sp %N' <exact-path>`): private client directories 0700, credential records 0600, copied command executable. Do not recursively chmod existing home parents. Help/UI/skill/status/ordinary errors must not disclose bearers or client-private paths. No encryption-at-rest or hostile-same-user protection is claimed. configVersion 2 is not readable by old Backends: old binary downgrade is not a rollback.

## M3 — Actual external-agent session (not a scripted substitute)

1. Record actual harness name/version, supported selected folder, how it discovers `teamarness/SKILL.md`, and whether explicit loading is needed. Start a **fresh external-agent session** with that skill. Automatic discovery is not a pass if you manually loaded it; record the actual route. Ask it to invoke the absolute CLI, list local Workspaces, explicitly select `WS`, and read live help.
2. Ask it to traverse **every contracts cursor including empty pages with nextCursor**, then fetch current `schema <contract> --major <n>` and `guide <contract> --major <n>`. No copied schema/Provider allowlist, inferred major, default Workspace or raw HTTP. Confirm missing/ambiguous Workspace is refused, not guessed.
3. With explicit exact grants and native prerequisites, have the agent read a real Backlog Ticket and create/update a disposable Ticket; preserve its opaque version for a stale-update conflict. Create and revise a Documents Document using the returned opaque version unchanged. Create/revise an Exhibit Draft with expected revision and observe a real stale conflict. Confirm writes are attributed to the Person, never an Agent fallback. Record safe IDs/version tokens and Provider result/error details, not credentials.
4. Have the agent observe a real Backlog event, make a native/CLI change, then re-read current state. Observe an eligible live streaming query from a guided Provider. Test finite `--for 30s --max-events 1`, and separately `--until-cancelled`, then cancel. Ready means confirmed live registration, not history. Heartbeats/control frames are not actionable domain values. For a shell recording use a pipe, **not** direct subscription file redirection:

   ```sh
   set -o pipefail
   "$CLI" --workspace "$WS" subscribe dev.teamarness.backlog backlog.changed \
     --major 1 --json null --for 30s --max-events 1 | tee observations.ndjson
   ```

5. Deliberately interrupt observation; **explicitly reconnect and reconcile with a fresh read**. No replay or automatic reconnect. If a transmitted write loses its response, inspect `outcomeUncertain:true`/`retryable:false`; reconcile using the live Provider guide before another command. Never replay to “see if it worked.”
6. Record whether the actual harness keeps the launched subprocess alive, delivers events while idle, wakes a model turn, and cleans up on session exit. No adapter, universal wakeup, hours-long watch, durable history, automatic session cleanup or transaction/cancellation promise is made. SIGINT normally exits 130; SIGKILL/broken pipes can prevent final diagnostics. Source fixture tests cannot establish harness behavior.

## M4 — Updates, movement, ports and idle authority

- **Advanced:** move only the test Package Directory using explicit registration/reload in the test profile; the copied CLI must still work. For an **actually admitted newer compatible recognized** Package, inspect automatic owned refresh. Record both versions/digests as maintainer evidence, not an ordinary UI comparison task. Equal bytes are a no-op. Same-version/different-byte development artifacts require the explicit **Update CLI** action, never automatic replacement. That click applies its exact recognized change directly; only resuming valid pending setup through Repair retains a targeted confirmation. Modified or unrecognized command bytes must refuse. Do not tamper with a command used by your ordinary app. Test interrupted executable/receipt publication only in an unused client installation or a separate account; otherwise leave it pending. If no newer admitted artifact is available, this subgate is pending, not simulated with edited production binaries.
- Keep a managed idle event observation and unrelated manual-token/other-Workspace access. Rotate through app-led setup: old generation closes promptly, no overlap; fresh CLI uses the published replacement. Manually revoke managed access while idle; activation/reload/CLI/repair must not silently mint it again. New access requires explicit consent. Disable only the exact event, then Pause/Resume, Reload, Stop listener, and Disconnect while idle. Observe each closure without requiring another domain event; unrelated access remains authorized where applicable. Reload restarts all Instances of that Personal Package, so a temporary transport gap is not durable unrelated revocation.
- Occupy the saved port with an owned disposable listener, observe desired Enabled versus stopped/no running URL. Dismiss changes presentation only. Free it, explicitly retry the **same saved port**; no random fallback. If Backend port mutation is exercised, app-led port-only repair preserves intact token generation. Catalog refresh while observing must not restart listener/stream.
- Disconnect revokes before local record cleanup; cleanup-pending is not complete and retry must not mint access. Revoke before Package removal. Retained CLI/records do not keep a listener alive; deleting a local file is not revocation. Never delete policy to repair. Use the cleanup procedure below. This current-account route permits deliberate client-file writes for the disposable test, not changes to the ordinary app's profile or broad cleanup.

## M5 — Actual revised A / Configuration / focus

Compare the production UI with revised A and the subsequently authorized direct-setup refinement, in both main and the **real Shell Configuration tab**. Click the gear: it must open this Package's Configuration in this Workspace, with Shell header/tabs/focus, shared flat control body and token controls below; no imitation tabs or internal Settings switch. Inspect enabled-before-expand, explicit exact tools, destructive disable confirmation, adjacent information/refresh, no port editor, bind retry and useful partial/error/storage/missing-Provider/schema-review presentation. A host without navigation must show a disabled gear/update explanation, not navigate via a URL or parent DOM.

Check that the Workspace name remains without an opaque ID. Agent setup must show persistent CLI/connection facts, short skill guidance, useful path details, and only relevant credential maintenance. No CLI uninstall, routine Preview/checkbox/Apply sequence, raw operation/owner IDs or hashes. A stopped listener must not label intact Workspace credentials disconnected. Each admitted manual catalog refresh must visibly acknowledge progress/result; background polling must not resurrect a dismissed success. The accepted larger gear and information toggle must remain.

Close/unmount while secrets/picker/catalog work are present: those clear; accepted setup and an unrelated listener continue. Check stale mount/Workspace changes do not target a later Workspace. Record actual packaged WKWebView focus and gear behavior. Prototype approval and source UI tests are not Don's production UI acceptance.

## Clean up the disposable test

1. While the test app still runs, **Disconnect** its managed connection. Confirm durable revocation and local cleanup. If cleanup is pending, resolve or record it before deleting the profile.
2. Stop the test listener, then quit the test app. Do not remove the profile first: losing the profile is not token revocation.
3. Remove only the disposable skill folder created for this journey, after checking that you have not reused or edited it for other work.
4. Keep the managed CLI if you want to use it. A retained executable does not keep a listener alive. Before removing a test-created command or receipt, confirm no other profile or connection now uses it. Do not delete the whole shared client store.
5. Remove only the exact test-profile directory printed at launch when its revocation and cleanup are complete. Keep any evidence you need outside that directory. Reopen your installed app normally.

## Optional clean-home and acquisition checks

A separate macOS account or another suitable clean environment is useful for first-use cleanliness and destructive collision, ownership-transfer, downgrade, or crash-publication tests. It is **not required to begin functional verification**. Keep any deferred subchecks PENDING rather than claiming the current-account journey covers them.

For acquired/source-free delivery qualification, use copied or acquired production artifacts outside the source checkout and record hashes, transfer/quarantine state, signing observations, and execution without a source build or extra runtime. A separate account does not by itself prove these properties. Developer tools already installed in your current account also do not prove clean-machine independence.

Observe OS checks without weakening them:

```sh
codesign -dv --verbose=4 "$APP" 2>&1
codesign --verify --deep --strict --verbose=2 "$APP" 2>&1
xattr -lr "$APP"
xattr -lr "$PKG"
spctl --assess --type execute --verbose=4 "$APP" 2>&1
```

If Gatekeeper, signing, or quarantine blocks execution, record the safe error and stop that subcheck. Do not strip quarantine, disable protections, change signing, or patch artifacts to obtain a pass. Normal UI observations do not replace the native frame-isolation smoke. Quitting the ordinary app may remove its known same-name ambiguity, but does not itself establish that the smoke passes.

## Automated reproduction (maintainer, separate from M1–M5)

From H, on the exact committed pair with clean G:

```sh
node scripts/verify-external-gateway.mjs --project /Users/don/hypr/ai-course-root/teamarness-local-packages/external-gateway
```

The runner freshly checks/builds locked G, checks canonical four-tree/SDK ancestry, qualifies actual production offline modes, builds unassembled Backend/CLI fixtures from that G revision, stages and admits their real hashed bytes in a **separate** fixture Package Directory, and provisions a canonical 0700 private root before any fixture runs. Backend homes are per Installation under that root; CLI processes receive the corresponding injected home. All discovered external cases must execute with zero ignored/filtered results. Production/fixture hashes are distinct and checked before/after. H dirty state includes untracked files truthfully, without reading them. No production activation runs under Don's normal OS home. The runner removes only its owned scratch root. It does **not** qualify acquired signing/quarantine, AppKit focus, production-home installation, agent discovery or native frame isolation.

Pending release gates: unchanged packaged native/isolation smoke (blocked same-name AX driver), M1–M5 observations, and fresh independent Verify/review. Do not describe this as a release, merged integration, or an overall acceptance pass.
