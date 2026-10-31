# Local Package registration

A local registration makes one built Package Directory on this device available to Teamarness. It trusts the native code at one canonical absolute directory. Install a Personal Package from the Shell. The Shell writes the registration for you, and the Package starts without an application restart. A hand-written registration file still works; see [Write the registration file by hand](#advanced-write-the-registration-file-by-hand).

## Prepare the Package Directory

Build and inspect the directory by following the [Package author guide](package-author-guide.md). Keep it outside the application bundle. Install the built Package Directory, not the project root. For a generated project, that is `dist/<identity>/`.

A registration trusts future rebuilt bytes at that same canonical directory. Teamarness still verifies the current manifest, Backend, UI, asset, and contract digests during every admission. If an admitted Backend byte changes while Teamarness is running, spawn fails with `artifact_changed`. To admit a deliberate rebuild, select **Reload** on the Package page.

## Install a Personal Package

1. Open the Workspace page: select **Workspace settings** beside the Workspace name.
2. In the **Package Instances** section, select **+** (**Add Personal Package**).
3. Select **Choose folder** and pick the Package Directory in the folder picker. To type the location instead, select **Paste a path** and enter the absolute path of the Package Directory.
4. Check the preview. It shows the name, Package Identity, version, folder, declared capabilities, and provided and consumed Ports. It also shows the description, author, and homepage when the manifest declares them. Select **Continue**.
5. Read the native-code risks: filesystem, network, and process. Select **Trust and install** only for code you have reviewed.

The Shell shows "Installed on this device. It appears in all your Workspaces." The Package runs at once under **Personal Packages**, in every Workspace on this Installation. Team Packages keep running, and their Backend processes are not restarted. A Package UI closes while you use the Workspace page and opens again when you return to it.

If you install a folder that is already registered, the preview says so. Installing it again restarts the Package with the current build. If the Package Identity is already registered from another folder, installing replaces that registration.

If Teamarness refuses the folder, nothing is registered. The Shell names the failed rule, for example a folder without a valid `teamarness.package.json`, or a file that changed after the manifest was written. `teamarness-package-validate` gives the same verdict without the application: run `npm run validate:dist` in a generated project, or `cargo run -p teamarness-package-contract --bin teamarness-package-validate -- directory <path> --smoke` from a Teamarness checkout.

## Manage a Package from its Package page

Every Package has a Package page. To open it, do one of these:

- Select the Package name in the **Package Instances** section of the Workspace page.
- Select the Package name in **Needs attention on this device**.

The sidebar shows each Package as its name and its status, for example "Available". Selecting a Package in the sidebar opens its Package UI, not its Package page.

The page starts with a header: the kind of Package ("Team Package" or "Personal Package"), the Package name, and its description. Below the header are the **About** tab and, when the Package declares a settings view, the **Configuration** tab. The page has no back button. Use the sidebar or the Workspace settings button to leave it.

The **About** tab has these groups:

- **Details:** Package Identity, version, author, homepage, source, trust, and, for a Personal Package, the Package folder.
- **Capabilities and Ports:** declared capabilities, provided Ports, and consumed Ports with their availability.
- **This Workspace:** the Instance state in this Workspace, or why the registration could not start.
- **Diagnostics:** shown only when the Package Host reported a diagnostic for the Package.
- **Manage:** the controls this Package allows.

Results of these controls appear in a message area below the tabs. A success message hides after 5 seconds. An error stays until you select **Dismiss** or take the next action. The **Package Instances** section of the Workspace page shows the results of **Reload**, **Remove**, **Recover Backend**, and **Dispose** in a message area at the top of the section, in the same way.

A Personal Package page has these controls:

| Control | What it does |
|---|---|
| **Reload** | Admits the current bytes of the Package folder in every Workspace. Use it after each rebuild. The process generation continues, and the Package Data Directory and settings are kept. |
| **Restart** | Starts a new Backend process for this Workspace's Instance only. |
| **Pause** and **Resume** | Pause sends the Backend a suspend request. The process keeps running, and Resume continues it. |
| **Start** | Starts an Instance that has not started yet, for example an on-demand Package. |
| **Remove** | Stops the Package in every Workspace and removes its registration. Its data stays on this device, so installing it again resumes where it left off. |

A Team Package page shows the same information but no Personal controls. It offers **Recover Backend** and **Dispose** only when the Instance needs recovery.

If the Package declares a settings view, the page shows the **Configuration** tab. It shows the settings view of the Package in the current Workspace, below the page header and tabs. Select **About** to return to the Package information; the settings view closes. A Package without an Instance in the current Workspace shows no **Configuration** tab.

## Fix a refused registration

A registration that Teamarness could not admit is listed under **Needs attention on this device** on the Workspace page, with a readable reason. Its Package page shows the same reason and the Package diagnostics.

1. Correct the Package Directory, for example by rebuilding it.
2. Select **Reload**.

To stop using it, select **Remove**. Do not weaken file permissions or change the expected identity merely to silence a mismatch.

## Understand the two tiers

Admission has two tiers (ADR 0036). The tier decides when a change takes effect. It is not a trust verdict: both tiers grant `trusted_native_code_v1`.

- The **Team tier** holds the Packages bundled with Teamarness and any override of one of them. It is admitted when Teamarness starts and does not change while it runs.
- The **Personal tier** holds every other local registration. You install, reload, and remove Personal Packages while Teamarness runs.

Port routing follows the tiers:

- A Team Package never uses a Port that a Personal Package provides. If only a Personal Package provides a contract, a Team consumer sees that Port as unavailable.
- Personal Providers are invisible to Team callers. This includes a Team Package that acts as a Workspace Agent: Agent discovery does not list a Personal Provider's contract, and an Agent call to it is refused. A Personal Agent harness sees Team and Personal Providers.
- A Personal Package can use Ports from Team and Personal Packages.
- A Personal Package that provides a Port contract a Team Package already provides is refused with `port_provider_conflict`.
- A live install or reload that provides a Port contract a running Personal Package already provides is also refused with `port_provider_conflict`. Remove the running provider first.
- When a live change adds or removes a provider, Teamarness restarts only the Personal consumers whose route changed.
- A Personal Provider that loses a required Port of its own keeps running, but Teamarness does not serve it: no consumer is routed to it, and Agent discovery does not offer it. Its routes return when the dependency returns.

The Shell presents Package Source `bundled` as `Team` and Package Source `local` as `Personal`. Package Source remains separate from trust, registration scope, Workspace enablement, synchronization, sharing, networking, and collaboration.

## Override a Team Package

A local Package with the Package Identity of a bundled Package replaces it only after two confirmations, and only after Teamarness restarts.

1. Select **+** and choose the folder of the local build. The preview says that the Package replaces a Team Package.
2. Read the native-code risks and select **Trust and continue**.
3. Read the second confirmation and select the replace button. It names the Team Package, for example **Replace Tutorial after restart**.
4. Select **Restart Teamarness**, or select **Later** to apply the override at the next start.

Until the restart, **Needs attention on this device** lists the override with "Applies after restart" and a **Restart Teamarness** button. The Team Package keeps running, and its Package page still labels its source as "Team". To change your mind before the restart, select **Cancel override**, then confirm with **Cancel override** again. No restart is needed.

After the restart, the override runs in the Team tier, so the sidebar lists it under **Team Packages**, because the sidebar groups Packages by tier. In the **Package Instances** section of the Workspace page, its source label follows Package Source, so it reads `Personal`. Its Package page is labelled "Team Package", and the source in **Details** reads "Team, replaced on this device by an override". It has no Personal controls.

To return to the bundled Package, select **Remove override** on the Package page or under **Needs attention on this device**. The override then shows "Applies after restart" and keeps running until you select **Restart Teamarness**.

No source wins by file order or Package version. If a declared override is invalid, that Package Identity stays unavailable. Teamarness does not silently fall back to bundled code. Unrelated valid Packages and the Core Runtime continue.

## Data continuity

The Package Data Directory is keyed by Installation, Workspace, and Package Identity. Source path and Package version do not select it. A bundled Package, its local override, a removed and reinstalled Personal Package, and a later bundled version therefore use the same durable directory for the same identity tuple.

To test continuity without inspecting private database contents:

1. Start on bundled code and make one visible Package state change.
2. Override the Package with a local build of the same identity, then select **Restart Teamarness**.
3. Confirm the Package page says "Team, replaced on this device by an override", confirm its trust, and inspect the visible state.
4. Select **Remove override**, then select **Restart Teamarness**.
5. Confirm the Package page says `Team` and inspect the same visible state.

For a Personal Package, make a visible change, select **Remove**, and install the same folder again. The state returns.

Removing or disposing of a Package does not delete its Package Data Directory. Teamarness has no Package-data deletion, backup, or migration service.

## Promote or remove

To promote a Package, give its complete tested Package Directory to the application build as described in [Promotion to application distribution](package-author-guide.md#promotion-to-application-distribution). Compare all declared bytes and digests before bundling.

To stop using a Personal Package, select **Remove** on its Package page. Remote Package fetching, signing and notarization, Gatekeeper remediation, and automatic source updates are not provided.

## Advanced: write the registration file by hand

The Shell and a hand-written file use the same file and the same format. Teamarness reads a hand-written change only when it starts. After that start, you can reload or remove a hand-registered Personal Package from its Package page like any other.

### Find the installation file

The file lives under the selected application data directory:

```text
<app-data>/packages/registrations.v1.json
```

Named development profiles use `.teamarness-dev/<profile>` as `<app-data>`. An expert `--data-dir` launch uses the absolute directory supplied to that option. Ordinary packaged launches use the platform application-data directory selected by Tauri.

Start Teamarness once with that data directory before writing the file. The Core Runtime refuses a data directory that holds unrelated files (`TEAM.SYS.ID-R07`, ADR 0017), so it must be one Teamarness has already created.

The registration file must be a regular, non-symlink file owned by the current user. Its parent must be private and the file owner-only. On macOS:

```sh
mkdir -p '<app-data>/packages'
chmod 700 '<app-data>' '<app-data>/packages'
chmod 600 '<app-data>/packages/registrations.v1.json'
```

Replace `<app-data>` and the Package Directory below with absolute paths. Do not paste an invitation, token, key, or database content into this file.

### Register one local Package

```json
{
  "schemaVersion": 1,
  "registrations": [
    {
      "directory": "/absolute/path/to/package-directory",
      "expectedIdentity": "com.example.my-package",
      "trust": {
        "kind": "trusted_native_code_v1",
        "acceptedRisks": ["filesystem", "network", "process"]
      }
    }
  ]
}
```

`expectedIdentity` must exactly match the Package Manifest. The complete `trusted_native_code_v1` evidence is explicit acknowledgement that the Backend can use the current user's ambient filesystem, network, and process authority. Process separation is not a sandbox.

The Package UI runs in its own sandboxed frame inside the Shell webview. It cannot reach the Shell's DOM, browser storage, the network, or any Tauri command, and it talks to Teamarness only through its mount-scoped bridge. Frame isolation does not contain a UI that blocks its thread, and it does not limit what the Backend can do. Register only code you trust.

### Override a bundled Package by hand

Add `"overrideApplication": true` to an entry whose `expectedIdentity` is the identity of a bundled Package:

```json
{
  "schemaVersion": 1,
  "registrations": [
    {
      "directory": "/absolute/path/to/package-directory",
      "expectedIdentity": "dev.teamarness.reference",
      "trust": {
        "kind": "trusted_native_code_v1",
        "acceptedRisks": ["filesystem", "network", "process"]
      },
      "overrideApplication": true
    }
  ]
}
```

Teamarness accepts a replacement only when the directory is canonical and readable, the manifest identity equals `expectedIdentity`, trust evidence is complete, and `overrideApplication` is true. An entry with a bundled identity and without `overrideApplication` is refused with `override_required`, and the bundled Package with that identity also stays unavailable until you correct the entry. **Needs attention on this device** lists such an entry with **Remove** and without **Reload**. **Remove** deletes the entry from the file, and the bundled Package returns after you select **Restart Teamarness**.

### What the Shell writes

The Shell writes the same version 1 document. Each entry it writes has the canonical directory, the expected identity, the three accepted risks, and `"overrideApplication"` set to `true` or `false`. A write keeps every other entry and replaces the entry with the same identity. It is atomic: a `0600` temporary file in the `0700` `packages/` directory, synced and renamed into place.

The Shell never rewrites a file that is already invalid, for example one with group or world permissions, a symlink, an unknown field, or malformed JSON. It refuses with `registration_file_invalid` and leaves the file unchanged. Fix or delete the file, then install again.

If you edit the file by hand while Teamarness runs, your entry stays in the file when the Shell next writes it, but it takes effect only at the next start. Two writers at the same moment are not coordinated: the last write wins.

### Diagnose a rejected hand-written registration

Check **Needs attention on this device** and the Package page. Common failures include a relative or symlink path, group or world permissions on the registration file, missing trust evidence, identity mismatch, an omitted bundled override, duplicate local registrations, unsupported target, missing file, digest mismatch, malformed manifest, or invalid UI contract.

Correct the source or the registration, then restart Teamarness. For a Personal registration whose file entry is correct, select **Reload** instead. Keep the expected identity tied to the Package you reviewed.
