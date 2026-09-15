# Local Package registration

Installation-local registration is an operator action outside the Teamarness UI. It trusts native code at one canonical absolute directory. Teamarness reads the file only at application startup.

## Prepare the Package Directory

Build and inspect the directory by following the [Package author guide](package-author-guide.md). Keep it outside the application bundle. The manifest identity must match the identity you intend to register.

A registration trusts future rebuilt bytes at that same canonical directory. Teamarness still verifies the current manifest, Backend, UI, asset, and contract digests during every admission. If any admitted Backend byte changes while Teamarness is running, spawn fails with `artifact_changed`. Restart the application to admit a deliberate rebuild. There is no hot reload.

## Find the installation file

Create this file under the selected application data directory:

```text
<app-data>/packages/registrations.v1.json
```

Named development profiles use `.teamarness-dev/<profile>` as `<app-data>`. An expert `--data-dir` launch uses the absolute directory supplied to that option. Ordinary packaged launches use the platform application-data directory selected by Tauri.

The registration file must be a regular, non-symlink file owned by the current user. On macOS, make its parent private and the file owner-only:

```sh
mkdir -p '<app-data>/packages'
chmod 700 '<app-data>' '<app-data>/packages'
chmod 600 '<app-data>/packages/registrations.v1.json'
```

Replace `<app-data>` and the Package Directory below with absolute paths. Do not paste an invitation, token, key, or database content into this file.

## Register one local Package

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

The Package UI runs in the Shell's main webview. Its Shadow Root limits styles, but the code retains DOM and known Tauri-command authority. It is not a UI sandbox. Register only code you trust.

## Override a bundled Package

A local Package with the same Package Identity as an application Package needs an explicit override:

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

No source wins by file order or Package version. Teamarness accepts a replacement only when the directory is canonical and readable, the manifest identity equals `expectedIdentity`, trust evidence is complete, and `overrideApplication` is true. If a declared override is invalid, that Package Identity stays unavailable. Teamarness does not silently fall back to bundled code. Unrelated valid Packages and the Core Runtime continue.

Restart Teamarness after creating, changing, or removing the registration. The Shell labels the admitted source `Local` and its trust `trusted_native_code_v1`. Bundled Tutorial is admitted from application resources without this file and is labeled `Bundled`. Confirm both labels before testing the local build. Source kind is not a security verdict and does not grant Workspace enablement, synchronization, sharing, networking, or collaboration.

## Data continuity

The Package Data Directory is keyed by Installation, Workspace, and Package Identity. Source path and Package version do not select it. A bundled Package, its explicit local override, and a later bundled version therefore use the same durable directory for the same identity tuple.

To test continuity without inspecting private database contents:

1. Start on bundled code and make one visible Package state change.
2. Stop Teamarness cleanly.
3. add the explicit local override and restart;
4. confirm the local source and trust labels and the visible state;
5. stop Teamarness, remove the registration, and restart;
6. confirm the bundled source label and the same visible state.

Disposal stops Package-owned resources but does not delete the Package Data Directory. Teamarness has no Package-data deletion, backup, or migration service in this phase.

## Diagnose a rejected registration

Check the Shell's stable Package diagnostic and recovery action. Common failures include a relative or symlink path, group or world permissions on the registration file, missing trust evidence, identity mismatch, an omitted bundled override, duplicate local registrations, unsupported target, missing file, digest mismatch, malformed manifest, or invalid UI contract.

Correct the source or registration, then restart. Do not weaken file permissions or change `expectedIdentity` merely to silence a mismatch. Keep the expected identity tied to the Package you reviewed.

## Promote or remove

To promote a Package, give its complete tested Package Directory to the application build as described in [Promotion to application distribution](package-author-guide.md#promotion-to-application-distribution). Compare all declared bytes and digests before bundling.

To stop using a local source, remove its registration entry or the owner-only registration file and restart. Removing a registration does not remove Package-owned data. Remote Package fetching, install and update UI, signing and notarization, Gatekeeper remediation, and automatic source updates are not provided.
