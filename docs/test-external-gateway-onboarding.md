# External Gateway onboarding: Don's installed qualification

**Status: M1–M5 PENDING; packaged native/isolation smoke BLOCKED.** Source/fixture success is not installed acceptance. The accepted native-smoke addendum stops further unattended accessibility/focus experiments. Do not close an unrelated same-name app or change OS/window-manager settings to obtain a pass. Final Verify remains blocked until real native/isolation observations pass.

## Prepare once (Don owns the environment and observations)

1. Use a **disposable macOS OS account with its own real home**, on Apple Silicon. A named development profile, changing `$HOME`, or only `--data-dir` is insufficient: production resolves the effective OS user's home and writes user-local outputs. Never do this journey in Don's ordinary account. Keep unrelated apps/settings unchanged.
2. Obtain the exact final `Teamarness.app`, DMG and assembled `dev.teamarness.external-gateway` Package Directory from the Phase-8 evidence. Copy/acquire them into that account, preserving acquisition/quarantine metadata. Do not rebuild, patch executable bytes, strip quarantine, disable Gatekeeper, change signing, or use a fixture Package. Make the H/G source checkouts and Node/npm/Rust unavailable for this journey. System libraries/tools are allowed.
3. In that account, set explicit paths below (replace placeholders with acquired paths). Record the OS/architecture, app and Package versions, final H/G revisions from evidence, **every acquired artifact's SHA-256**, transfer route and signing/quarantine state. Recheck hashes against Phase-8 evidence before launch. Do not record a bearer, connection file contents, invitation, or private key.

   ```sh
   APP='/absolute/acquired/Teamarness.app'
   PKG='/absolute/acquired/dev.teamarness.external-gateway'
   DMG='/absolute/acquired/Teamarness.dmg'
   PROFILE="$HOME/onboarding-qualification/app-data"
   mkdir -p "$PROFILE"; chmod 700 "$PROFILE"
   sw_vers; uname -m
   shasum -a 256 "$DMG" "$APP/Contents/MacOS/teamarness-desktop"
   find "$PKG" -type f -exec shasum -a 256 {} \;
   codesign -dv --verbose=4 "$APP" 2>&1
   codesign --verify --deep --strict --verbose=2 "$APP" 2>&1
   xattr -lr "$APP"; xattr -lr "$PKG"; xattr -l "$DMG"
   spctl --assess --type execute --verbose=4 "$APP" 2>&1
   "$APP/Contents/MacOS/teamarness-desktop" --data-dir "$PROFILE"
   ```

   These are observations, not requirements that ad-hoc artifacts have Developer ID/notarization. If OS execution blocks the acquired app/Package/helper, record the exact safe error and stop that gate; security bypass is not a repair. Record DMG mounting/acquisition behavior too. Keep evidence outside project/skill/credential directories.
4. In the isolated app, create a disposable Workspace and record its ID as `WS`. Install the acquired **production** Package from its directory via the existing Personal Package UI and explicit filesystem/network/process trust consent. Create native Backlog Epic and Documents Collection prerequisites. No Agent Package or Binding is needed. Enable only representative exact tools, not all tools. Record selections, not credentials.
5. Record results at the workstream's `verify/manual-onboarding.md`: dated M1–M5 checklist, PASS/FAIL/BLOCKED/PENDING, actual steps/artifact hashes/harness version and safe observations. Inspect screenshots for secrets before saving. Never paste a bearer into chat, prompts, terminals or evidence; do not `cat` client files. Record file **presence/mode** only.

## M1 — Acquired, source-free native setup

- Open Gateway main view and Agent setup. Choose an empty folder supported by the intended external harness (not an invented universal discovery folder). Confirm native folder chooser opens. Cancel once: no selected skill write or credential consent. Choose again; leave the chooser open **over 30 seconds** while another view/Workspace and gateway status/read remain responsive. Record focus, title-bar and return-focus observations; do not diagnose AeroSpace from appearance.
- With a pending chooser, close its view; repeat with Package Pause, Reload, and owned parent-only process death in this disposable account. Observe helper exit/no late selected folder. Never kill an unrelated app. If safe parent-only death cannot be arranged, mark that check pending rather than claiming it.
- Choose a folder, inspect exact canonical destination, preview, then apply. Separate **command installed / skill installed / access configured** results must be truthful. Record absolute command path as `CLI="$HOME/.local/bin/tmx"`. Leave `.local/bin` out of PATH and use the absolute command. With source/build tools unavailable, run:

  ```sh
  CLI="$HOME/.local/bin/tmx"
  "$CLI" --help
  "$CLI" --version
  "$CLI" --licenses
  "$CLI" workspaces
  WS='the-recorded-workspace-id'
  "$CLI" --workspace "$WS" status
  ```

  Confirm no runtime/source lookup and correct Package 1.1.0/CLI major 1. Offline listing is last-observed local metadata, not a liveness claim.

## M2 — Consent, interrupted observation, files and recovery

- Cancel preview before apply: no setup effects. Apply once, immediately close/remount the UI: accepted work continues, status finds the same operation; retry does not duplicate managed tokens. Repeat an interrupted/lost observation and owned process restart, then inspect status before any new consent. Never claim interrupted publication rolled back.
- On a fresh isolated test state, put a harmless unrelated executable named `tmx` at the fixed command destination before setup. Preview/apply must refuse without altering it. Move the collision out yourself and re-preview; never consent to blind overwrite. Edit the installed skill, reselect it and verify refusal/preservation; choose a new empty folder to continue. Replace/change a previewed selected folder before apply: require reselect/new preview.
- Exercise a second isolated app data directory for the same Workspace **within this disposable OS account**. Both resolve the same user-local store. Confirm duplicate Installation/continuity/origin refuses, rather than silently winning. Disconnect the original before deliberate transfer. If original is unavailable, only manually remove its exact private connection record after inspecting the conflict; removal cannot revoke the old token. Never delete the whole store or policy as recovery.
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

- Move the acquired Package Directory using explicit app registration/reload; the copied CLI must still work. For an **actually admitted newer compatible recognized** Package, inspect automatic owned refresh. Record both versions/digests. Equal bytes are a no-op. Same-version/different-byte development artifacts require explicit preview/repair, never automatic replacement. Modified/unrecognized command bytes refuse; preserve and externally resolve collisions. Test interrupted executable/receipt publication and status/recovery without adopting arbitrary bytes. If no newer admitted artifact is available, this subgate is pending, not simulated with edited production binaries.
- Keep a managed idle event observation and unrelated manual-token/other-Workspace access. Rotate through app-led setup: old generation closes promptly, no overlap; fresh CLI uses the published replacement. Manually revoke managed access while idle; activation/reload/CLI/repair must not silently mint it again. New access requires explicit consent. Disable only the exact event, then Pause/Resume, Reload, Stop listener, and Disconnect while idle. Observe each closure without requiring another domain event; unrelated access remains authorized where applicable. Reload restarts all Instances of that Personal Package, so a temporary transport gap is not durable unrelated revocation.
- Occupy the saved port with an owned disposable listener, observe desired Enabled versus stopped/no running URL. Dismiss changes presentation only. Free it, explicitly retry the **same saved port**; no random fallback. If Backend port mutation is exercised, app-led port-only repair preserves intact token generation. Catalog refresh while observing must not restart listener/stream.
- Disconnect revokes before local record cleanup; cleanup-pending is not complete and retry must not mint access. Revoke before Package removal. Retained CLI/records do not keep a listener alive; deleting a local file is not revocation. Never delete policy to repair. Remove only owned disposable account/test state after evidence; no normal-profile installation or broad cleanup.

## M5 — Actual revised A / Configuration / focus

Compare acquired production UI with the separately approved revised A, in both main and the **real Shell Configuration tab**. Click the gear: it must open this Package's Configuration in this Workspace, with Shell header/tabs/focus, shared flat control body and token controls below; no imitation tabs or internal Settings switch. Inspect enabled-before-expand, explicit exact tools, destructive disable confirmation, adjacent information/refresh, no port editor, bind retry and useful partial/error/storage/missing-Provider/schema-review presentation. A host without navigation must show a disabled gear/update explanation, not navigate via a URL or parent DOM.

Close/unmount while secrets/picker/catalog work are present: those clear; accepted setup and an unrelated listener continue. Check stale mount/Workspace changes do not target a later Workspace. Record actual packaged WKWebView focus and gear behavior. Prototype approval and source UI tests are not Don's production UI acceptance.

## Automated reproduction (maintainer, separate from M1–M5)

From H, on the exact committed pair with clean G:

```sh
node scripts/verify-external-gateway.mjs --project /Users/don/hypr/ai-course-root/teamarness-local-packages/external-gateway
```

The runner freshly checks/builds locked G, checks canonical four-tree/SDK ancestry, qualifies actual production offline modes, builds unassembled Backend/CLI fixtures from that G revision, stages and admits their real hashed bytes in a **separate** fixture Package Directory, and provisions a canonical 0700 private root before any fixture runs. Backend homes are per Installation under that root; CLI processes receive the corresponding injected home. All discovered external cases must execute with zero ignored/filtered results. Production/fixture hashes are distinct and checked before/after. H dirty state includes untracked files truthfully, without reading them. No production activation runs under Don's normal OS home. The runner removes only its owned scratch root. It does **not** qualify acquired signing/quarantine, AppKit focus, production-home installation, agent discovery or native frame isolation.

Pending release gates: unchanged packaged native/isolation smoke (blocked same-name AX driver), M1–M5 observations, and fresh independent Verify/review. Do not describe this as a release, merged integration, or an overall acceptance pass.
