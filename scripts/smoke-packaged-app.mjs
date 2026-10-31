import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { chmod, mkdtemp, readdir, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { STAGING_ROOT, readBundledPackages, repositoryRoot } from "./bundled-packages.mjs";
import { waitForDesktopCleanup } from "./packaged-app-cleanup.mjs";
import { compareDeclaredPackageBytes } from "./package-directory.mjs";
import { prepareUiIsolationProbe } from "./ui-isolation-probe/prepare.mjs";
import { buildExhibitsFixtureConsumer, prepareExhibitsFixtureConsumer } from "./exhibits-fixture-consumer/prepare.mjs";

const app = join(repositoryRoot, "target/release/bundle/macos/Teamarness.app");
const executable = join(app, "Contents/MacOS/teamarness-desktop");
const bundledPackages = (await readBundledPackages(repositoryRoot)).map(({ identity, manifest }) => ({
  identity,
  displayName: manifest.presentation.displayName,
  resources: join(app, "Contents/Resources/packages", identity),
  staged: join(repositoryRoot, STAGING_ROOT, identity),
  backendPath: manifest.backends[0].path,
  onDemand: manifest.activation === "on_demand",
}));
const exhibitsPackage = bundledPackages.find(({ identity }) => identity === "dev.teamarness.exhibits");
assert.ok(exhibitsPackage, "the Exhibits Package is not bundled");

// The checks the Exhibits revision 1 fixture reports, each as visible `PASS <name>` text.
const EXHIBIT_RENDERING_CHECKS = [
  "inline-script", "inline-style", "relative-stylesheet", "css-import", "png-image", "svg-image", "inline-svg",
  "module-static-import", "module-dynamic-import", "fetch-own-json", "wasm-instantiate-streaming",
  "wasm-compile-bytes", "navigated-to-page-2-and-back",
];
// The checks the content isolation probe reports, in the Exhibits viewer and in a frame
// created without a `sandbox` attribute.
const CONTENT_ISOLATION_CHECKS = [
  "opaque-origin", "no-tauri-internals", "no-tauri-global", "no-window-ipc", "parent-document-blocked",
  "top-document-blocked", "top-location-blocked", "storage-blocked", "ipc-fetch-blocked",
  "forged-bridge-connect-ignored",
];
const dmg = join(repositoryRoot, "target/release/bundle/dmg/Teamarness_0.1.0_aarch64.dmg");
const diagnosticOutputLimit = 4_096;

class SmokeTimeoutError extends Error {}

if (process.platform !== "darwin" || process.arch !== "arm64") {
  throw new Error("The packaged application smoke requires Apple silicon macOS");
}

await assertFile(executable);
await assertFile(dmg);
for (const { staged, resources } of bundledPackages) {
  await compareDeclaredPackageBytes(staged, resources);
}
await inspectDmg();

const dataDirectory = await mkdtemp(join(tmpdir(), "teamarness-packaged-smoke-"));
try {
  await smokeLaunch(dataDirectory, true);
  await smokeLaunch(dataDirectory, false);
  await probeLaunch(dataDirectory);
  await exhibitsLaunch(dataDirectory);
  console.log(
    "Packaged app smoke passed twice with admitted UI, protocol traffic, and clean process reaping, "
    + "the WKWebView UI isolation probe reached neither Tauri nor the Shell, "
    + "and Exhibits rendered a Draft, swapped the open Draft to its next revision, and kept its content "
    + "from reaching either (in its viewer, and in a frame without a sandbox attribute, which inherits "
    + "the Package frame's sandbox; the CSP sandbox header is proven by the content scheme's tests).",
  );
} finally {
  await chmod(dataDirectory, 0o700).catch(() => {});
  await makeWritable(dataDirectory);
  await rm(dataDirectory, { recursive: true, force: true });
}

async function withDesktop(dataDirectory, body) {
  const appStdout = createBoundedOutput();
  const appStderr = createBoundedOutput();
  const diagnostics = {
    activeStage: "desktop process",
    lastAccessibilityFailure: "",
    privateValues: [dataDirectory],
    appStdout,
    appStderr,
  };
  const desktop = spawn(executable, ["--data-dir", dataDirectory], {
    cwd: repositoryRoot,
    detached: false,
    stdio: ["ignore", "pipe", "pipe"],
  });
  desktop.stdout.on("data", appStdout.append);
  desktop.stderr.on("data", appStderr.append);

  let childPid;
  const backendPids = [];
  const atStage = async (stage, operation) => {
    diagnostics.activeStage = stage;
    try {
      return await operation();
    } catch (error) {
      if (error instanceof SmokeTimeoutError) {
        throw new Error(formatTimeoutDiagnostics(error, diagnostics, childPid));
      }
      throw error;
    }
  };
  const waitForAccessible = (source, label) => atStage(label, () =>
    waitForAppleScript(source, label, (failure) => {
      diagnostics.lastAccessibilityFailure = failure;
    }));

  try {
    childPid = await atStage("desktop process", () => waitFor(() => desktop.pid, "desktop process"));
    return await body({ childPid, desktop, diagnostics, atStage, waitForAccessible, backendPids });
  } finally {
    if (childPid && processExists(childPid)) desktop.kill("SIGTERM");
    await waitForDesktopCleanup(
      () => atStage("desktop cleanup", () => waitForExit(desktop, "desktop cleanup")),
      (signal) => desktop.kill(signal),
    );
    for (const backendPid of backendPids) {
      await atStage("Package process cleanup", () =>
        waitFor(() => !processExists(backendPid), `Package process ${backendPid} cleanup`, 5_000));
    }
  }
}

async function quitDesktop({ childPid, desktop, diagnostics, atStage, backendPids }) {
  diagnostics.activeStage = "desktop shutdown request";
  await runAppleScript(`
    tell application "System Events"
      set appProcess to first process whose unix id is ${childPid}
      tell appProcess
        set frontmost to true
        keystroke "q" using command down
      end tell
    end tell
  `, "desktop shutdown");
  await atStage("desktop graceful shutdown", () => waitForExit(desktop, "desktop graceful shutdown"));
  for (const backendPid of backendPids) {
    await atStage("Package process reaping", () =>
      waitFor(() => !processExists(backendPid), `Package process ${backendPid} reaping`));
  }
}

async function smokeLaunch(dataDirectory, createWorkspace) {
  await withDesktop(dataDirectory, async ({ childPid, desktop, diagnostics, atStage, waitForAccessible, backendPids }) => {
    await waitForAccessible(accessibilityScript(childPid, `
      return name of window 1
    `), "desktop window");
    await waitForAccessible(accessibilityScript(childPid, `
      set workspaceSwitcher to my namedDescendant(appUi, "Workspace switcher", "AXGroup")
      return name of workspaceSwitcher
    `), "desktop Shell readiness");

    if (createWorkspace) {
      await waitForAccessible(accessibilityScript(childPid, `
        set selectedContent to my subroleDescendant(appUi, "AXLandmarkMain")
        set workspaceNameField to my namedDescendant(selectedContent, "Workspace name", "AXTextField")
        return name of workspaceNameField
      `), "Workspace creation form");
      diagnostics.activeStage = "Workspace creation";
      await runAppleScript(accessibilityScript(childPid, `
        set workspaceSwitcher to my namedDescendant(appUi, "Workspace switcher", "AXGroup")
        set activeWorkspaceNavigation to my namedDescendant(appUi, "Active Workspace navigation", "AXGroup")
        set selectedContent to my subroleDescendant(appUi, "AXLandmarkMain")
        set workspaceNameField to my namedDescendant(selectedContent, "Workspace name", "AXTextField")
        set createButton to my namedDescendant(selectedContent, "Create", "AXButton")
        set focused of workspaceNameField to true
        keystroke "Packaged smoke"
        set personNameField to my namedDescendant(selectedContent, "Your name in this Workspace", "AXTextField")
        set focused of personNameField to true
        keystroke "Smoke Person"
        delay 0.3
        click createButton
      `), "Workspace creation");
      assert.equal(await registrationExists(dataDirectory), false, "fresh bundled Tutorial created a registration file");
    }

    await waitForAccessible(accessibilityScript(childPid, `
      set activeWorkspaceNavigation to my namedDescendant(appUi, "Active Workspace navigation", "AXGroup")
      set teamPackages to my namedDescendant(activeWorkspaceNavigation, "Team Packages", "AXGroup")
      set referencePackage to my namedDescendantStartingWith(teamPackages, "Reference Package", "AXButton")
      return name of referencePackage
    `), "bundled reference Package navigation");

    // The Package Host eagerly activates every admitted Package that is not `on_demand`, so each
    // of those bundled Backends must launch. An `on_demand` Backend (Exhibits) starts when it is
    // first used; the Exhibits stage observes it.
    for (const { identity, displayName, resources, backendPath } of bundledPackages.filter(({ onDemand }) => !onDemand)) {
      const backend = join(resources, backendPath);
      const stage = `bundled Package Backend ${identity}`;
      const backendPid = await atStage(stage, () => waitFor(() => bundledBackend(childPid, backend), stage));
      backendPids.push(backendPid);
      assertBundledBackend(backendPid, childPid, backend, displayName);
    }

    await waitForAccessible(accessibilityScript(childPid, `
      set activeWorkspaceNavigation to my namedDescendant(appUi, "Active Workspace navigation", "AXGroup")
      set teamPackages to my namedDescendant(activeWorkspaceNavigation, "Team Packages", "AXGroup")
      set exhibitsNavigation to my namedDescendantStartingWith(teamPackages, "${exhibitsPackage.displayName}", "AXButton")
      return name of exhibitsNavigation
    `), "bundled Exhibits navigation");

    await waitForAccessible(accessibilityScript(childPid, `
      set activeWorkspaceNavigation to my namedDescendant(appUi, "Active Workspace navigation", "AXGroup")
      set teamPackages to my namedDescendant(activeWorkspaceNavigation, "Team Packages", "AXGroup")
      set tutorialPackage to my namedDescendantStartingWith(teamPackages, "Tutorial", "AXButton")
      return name of tutorialPackage
    `), "bundled Tutorial navigation");
    diagnostics.activeStage = "bundled Tutorial selection";
    await runAppleScript(accessibilityScript(childPid, `
      set activeWorkspaceNavigation to my namedDescendant(appUi, "Active Workspace navigation", "AXGroup")
      set teamPackages to my namedDescendant(activeWorkspaceNavigation, "Team Packages", "AXGroup")
      set tutorialPackage to my namedDescendantStartingWith(teamPackages, "Tutorial", "AXButton")
      click tutorialPackage
    `), "bundled Tutorial selection");
    await atStage("bundled Tutorial UI request response", () => waitFor(async () => {
      const contents = spawnSync("osascript", ["-e", accessibilityScript(childPid, `
        set selectedContent to my subroleDescendant(appUi, "AXLandmarkMain")
        set packageInterface to my namedDescendant(selectedContent, "Package interface", "AXGroup")
        return entire contents of packageInterface
      `)], { encoding: "utf8" });
      if (contents.status !== 0) {
        diagnostics.lastAccessibilityFailure = appleScriptFailure(contents);
        return undefined;
      }
      return contents.stdout.includes("Generate")
        && contents.stdout.includes("Save")
        && contents.stdout.includes("Waves")
        && contents.stdout.includes("Garden");
    }));

    diagnostics.activeStage = "bundled reference Package selection";
    await runAppleScript(accessibilityScript(childPid, `
      set activeWorkspaceNavigation to my namedDescendant(appUi, "Active Workspace navigation", "AXGroup")
      set teamPackages to my namedDescendant(activeWorkspaceNavigation, "Team Packages", "AXGroup")
      set referencePackage to my namedDescendantStartingWith(teamPackages, "Reference Package", "AXButton")
      click referencePackage
    `), "bundled reference Package selection");
    await atStage("custom-scheme Package UI", () => waitFor(async () => {
      const contents = spawnSync("osascript", ["-e", accessibilityScript(childPid, `
        set selectedContent to my subroleDescendant(appUi, "AXLandmarkMain")
        set packageInterface to my namedDescendant(selectedContent, "Package interface", "AXGroup")
        return entire contents of packageInterface
      `)], { encoding: "utf8" });
      if (contents.status !== 0) {
        diagnostics.lastAccessibilityFailure = appleScriptFailure(contents);
        return undefined;
      }
      const visible = contents.stdout;
      return visible.includes("UI kit Ready")
        && visible.includes("Signing round trip")
        && visible.includes("Verified")
        && visible.includes("Increment counter");
    }, "custom-scheme Package UI"));

    await quitDesktop({ childPid, desktop, diagnostics, atStage, backendPids });
  });
}

async function probeLaunch(dataDirectory) {
  const { displayName } = await prepareUiIsolationProbe(dataDirectory);
  await withDesktop(dataDirectory, async ({ childPid, desktop, diagnostics, atStage, waitForAccessible, backendPids }) => {
    await waitForAccessible(accessibilityScript(childPid, `
      set workspaceSwitcher to my namedDescendant(appUi, "Workspace switcher", "AXGroup")
      return name of workspaceSwitcher
    `), "desktop Shell readiness");

    await waitForAccessible(accessibilityScript(childPid, `
      set activeWorkspaceNavigation to my namedDescendant(appUi, "Active Workspace navigation", "AXGroup")
      set personalPackages to my namedDescendant(activeWorkspaceNavigation, "Personal Packages", "AXGroup")
      set probePackage to my namedDescendantStartingWith(personalPackages, "${displayName}", "AXButton")
      return name of probePackage
    `), "UI isolation probe navigation");
    diagnostics.activeStage = "UI isolation probe selection";
    await runAppleScript(accessibilityScript(childPid, `
      set activeWorkspaceNavigation to my namedDescendant(appUi, "Active Workspace navigation", "AXGroup")
      set personalPackages to my namedDescendant(activeWorkspaceNavigation, "Personal Packages", "AXGroup")
      set probePackage to my namedDescendantStartingWith(personalPackages, "${displayName}", "AXButton")
      click probePackage
    `), "UI isolation probe selection");

    await atStage("UI isolation probe result", () => waitFor(async () => {
      const contents = spawnSync("osascript", ["-e", accessibilityScript(childPid, `
        set selectedContent to my subroleDescendant(appUi, "AXLandmarkMain")
        set packageInterface to my namedDescendant(selectedContent, "Package interface", "AXGroup")
        return entire contents of packageInterface
      `)], { encoding: "utf8" });
      if (contents.status !== 0) {
        diagnostics.lastAccessibilityFailure = appleScriptFailure(contents);
        return undefined;
      }
      if (contents.stdout.includes("Isolation probe FAIL")) {
        const failed = contents.stdout.split(",")
          .map((entry) => entry.trim())
          .filter((entry) => entry.endsWith(": fail"));
        throw new Error(`The packaged UI isolation probe failed: ${failed.join(", ") || contents.stdout.trim()}`);
      }
      return contents.stdout.includes("Isolation probe PASS");
    }, "UI isolation probe result"));

    await quitDesktop({ childPid, desktop, diagnostics, atStage, backendPids });
  });

  const database = await findCoreDatabase(dataDirectory);
  assert.equal(
    countWorkspaces(database, "name LIKE 'ui-isolation-breach%'"),
    0,
    "a Package UI Frame reached a Shell command in the packaged application",
  );
  assert.equal(
    countWorkspaces(database, "name = 'Packaged smoke'"),
    1,
    "the isolation probe database check did not observe the smoke Workspace",
  );
}

/**
 * The Exhibits stage. The test-only fixture consumer publishes the content isolation probe and
 * fixture revision 1 through the real Exhibits Ports, and frames its own content view of the
 * probe without a `sandbox` attribute. Exhibits then lists both Drafts and opens each in its
 * sandboxed viewer frame. With revision 1 open, a delayed publish of revision 2 must change the
 * open view without reselecting it. Every verdict is read through Accessibility, and the Core
 * database must hold no Workspace a content frame tried to create.
 */
async function exhibitsLaunch(dataDirectory) {
  const backend = buildExhibitsFixtureConsumer();
  const { displayName } = await prepareExhibitsFixtureConsumer(dataDirectory, { backend });
  await withDesktop(dataDirectory, async ({ childPid, desktop, diagnostics, atStage, waitForAccessible, backendPids }) => {
    const contents = () => {
      const result = spawnSync("osascript", ["-e", accessibilityScript(childPid, `
        set selectedContent to my subroleDescendant(appUi, "AXLandmarkMain")
        set packageInterface to my namedDescendant(selectedContent, "Package interface", "AXGroup")
        return entire contents of packageInterface
      `)], { encoding: "utf8" });
      if (result.status !== 0) {
        diagnostics.lastAccessibilityFailure = appleScriptFailure(result);
        return undefined;
      }
      return result.stdout;
    };
    const selectPackage = async (group, name, label) => {
      const find = `
        set activeWorkspaceNavigation to my namedDescendant(appUi, "Active Workspace navigation", "AXGroup")
        set packageGroup to my namedDescendant(activeWorkspaceNavigation, "${group}", "AXGroup")
        set packageButton to my namedDescendantStartingWith(packageGroup, "${name}", "AXButton")`;
      await waitForAccessible(accessibilityScript(childPid, `${find}
        return name of packageButton`), `${label} navigation`);
      diagnostics.activeStage = `${label} selection`;
      await runAppleScript(accessibilityScript(childPid, `${find}
        click packageButton`), `${label} selection`);
    };
    const clickInPackage = async (name, label) => {
      const find = `
        set selectedContent to my subroleDescendant(appUi, "AXLandmarkMain")
        set packageInterface to my namedDescendant(selectedContent, "Package interface", "AXGroup")
        set target to my namedDescendantStartingWith(packageInterface, "${name}", "AXButton")`;
      await waitForAccessible(accessibilityScript(childPid, `${find}
        return name of target`), label);
      diagnostics.activeStage = label;
      await runAppleScript(accessibilityScript(childPid, `${find}
        click target`), label);
    };
    const waitForText = (text, label) => atStage(label, () => waitFor(() => {
      const visible = contents();
      if (visible?.includes("failed: ")) {
        const failed = visible.split(",").map((entry) => entry.trim()).filter((entry) => entry.includes("failed: "));
        throw new Error(`${label}: ${failed.join(", ")}`);
      }
      return visible?.includes(text);
    }, label));
    // Waits for a probe or fixture verdict, then requires `PASS <name>` for every expected check.
    const waitForVerdict = (verdict, checks, label) => atStage(label, () => waitFor(() => {
      const visible = contents();
      if (!visible) return undefined;
      const failed = [...visible.matchAll(/FAIL ([a-z0-9-]+)/gu)].map((match) => match[1]);
      if (failed.length > 0 || /probe FAIL/u.test(visible)) {
        throw new Error(`${label} failed: ${failed.join(", ") || "verdict FAIL"}`);
      }
      if (!visible.includes(verdict)) return undefined;
      const missing = checks.filter((name) => !visible.includes(`PASS ${name}`));
      if (missing.length > 0) return undefined;
      return true;
    }, label, 45_000));

    await waitForAccessible(accessibilityScript(childPid, `
      set workspaceSwitcher to my namedDescendant(appUi, "Workspace switcher", "AXGroup")
      return name of workspaceSwitcher
    `), "desktop Shell readiness");

    await selectPackage("Personal Packages", displayName, "Exhibits fixture consumer");
    await clickInPackage("Publish isolation probe", "isolation probe publication");
    await waitForText("Published fixture-isolation-probe revision 1", "isolation probe publication result");

    // The publication went through the Exhibits Ports, so the on-demand Exhibits Backend runs
    // from its bundled Package Directory now.
    const exhibitsBackend = join(exhibitsPackage.resources, exhibitsPackage.backendPath);
    const exhibitsPid = await atStage("on-demand Exhibits Backend", () =>
      waitFor(() => bundledBackend(childPid, exhibitsBackend), "on-demand Exhibits Backend"));
    backendPids.push(exhibitsPid);
    assertBundledBackend(exhibitsPid, childPid, exhibitsBackend, exhibitsPackage.displayName);

    await clickInPackage("Publish revision 1", "revision 1 publication");
    await waitForText("Published fixture-revision-1 revision 1", "revision 1 publication result");
    await clickInPackage("Open unsandboxed frame probe", "unsandboxed frame probe");
    await waitForVerdict("Unsandboxed content frame probe PASS", CONTENT_ISOLATION_CHECKS, "unsandboxed content frame probe");

    await selectPackage("Team Packages", exhibitsPackage.displayName, "Exhibits");
    await clickInPackage("Exhibit isolation probe", "Exhibits isolation probe Draft");
    await waitForVerdict("Content isolation probe PASS", CONTENT_ISOLATION_CHECKS, "Exhibits content isolation probe");
    await clickInPackage("Exhibit fixture (revision 1)", "Exhibits revision 1 Draft");
    await waitForText("Exhibit fixture revision 1", "Exhibits revision 1 entry page");
    await waitForVerdict("PASS navigated-to-page-2-and-back", EXHIBIT_RENDERING_CHECKS, "Exhibits revision 1 rendering");

    // Live update (AC3). The consumer schedules revision 2 of the same Draft 30 s ahead, and
    // Exhibits is shown with revision 1 open before it lands. The open view must then change to
    // revision 2 with no further click: the viewer swapped its frame on `exhibits.changed`.
    await selectPackage("Personal Packages", displayName, "Exhibits fixture consumer before the live update");
    await clickInPackage("Publish revision 2 in 30 s", "delayed revision 2 publication");
    await waitForText("Scheduled fixture-revision-1 in 30000 ms", "delayed revision 2 scheduling");
    await selectPackage("Team Packages", exhibitsPackage.displayName, "Exhibits for the live update");
    await clickInPackage("Exhibit fixture (revision 1)", "Exhibits revision 1 Draft before the live update");
    await waitForText("Exhibit fixture revision 1", "Exhibits revision 1 open before the live update");
    await atStage("Exhibits live update to revision 2", () => waitFor(() => {
      const visible = contents();
      return visible?.includes("Exhibit fixture revision 2") && !visible.includes("Exhibit fixture revision 1");
    }, "Exhibits live update to revision 2", 45_000));

    await quitDesktop({ childPid, desktop, diagnostics, atStage, backendPids });
  });

  const database = await findCoreDatabase(dataDirectory);
  assert.equal(
    countWorkspaces(database, "name LIKE 'content-isolation-breach%'"),
    0,
    "an Exhibit content frame reached a Shell command in the packaged application",
  );
  assert.equal(
    countWorkspaces(database, "name = 'Packaged smoke'"),
    1,
    "the content isolation database check did not observe the smoke Workspace",
  );
}

async function findCoreDatabase(dataDirectory, depth = 0) {
  if (depth > 4) return undefined;
  const entries = await readdir(dataDirectory, { withFileTypes: true }).catch(() => []);
  for (const entry of entries) {
    if (entry.isFile() && entry.name === "core.sqlite3") return join(dataDirectory, entry.name);
  }
  for (const entry of entries) {
    if (!entry.isDirectory()) continue;
    const found = await findCoreDatabase(join(dataDirectory, entry.name), depth + 1);
    if (found) return found;
  }
  if (depth === 0) throw new Error("The packaged smoke found no core.sqlite3 under its data directory");
  return undefined;
}

function countWorkspaces(database, predicate) {
  const result = spawnSync("sqlite3", ["-readonly", database, `SELECT count(*) FROM workspaces WHERE ${predicate}`], {
    encoding: "utf8",
  });
  if (result.status !== 0) throw new Error("The packaged smoke could not read the Core database");
  return Number(result.stdout.trim());
}

async function inspectDmg() {
  const mount = await mkdtemp(join(tmpdir(), "teamarness-dmg-smoke-"));
  try {
    const attached = spawnSync("hdiutil", ["attach", "-readonly", "-nobrowse", "-mountpoint", mount, dmg], {
      encoding: "utf8",
    });
    if (attached.status !== 0) throw new Error("The DMG could not be mounted");
    const entries = await readdir(mount);
    assert.ok(entries.includes("Teamarness.app"), "The DMG does not contain Teamarness.app");
  } finally {
    spawnSync("hdiutil", ["detach", mount, "-quiet"], { stdio: "ignore" });
    await rm(mount, { recursive: true, force: true });
  }
}

function bundledBackend(parentPid, executablePath) {
  return processRows().find(({ parentPid: value, command }) =>
    value === parentPid && command.startsWith(executablePath))?.pid;
}

function assertBundledBackend(pid, parentPid, executablePath, packageName) {
  const processRow = processRows().find(({ pid: value }) => value === pid);
  assert.ok(processRow, `${packageName} Backend process disappeared before inspection`);
  assert.equal(processRow.parentPid, parentPid, `${packageName} Backend is not a direct child of the app`);
  assert.equal(processRow.processGroup, pid, `${packageName} Backend does not own a process group`);
  assert.ok(
    processRow.command.startsWith(executablePath),
    `${packageName} Backend did not resolve from its bundled Package Directory`,
  );
}

function processRows() {
  const result = spawnSync("ps", ["-axo", "pid=,ppid=,pgid=,command="], { encoding: "utf8" });
  if (result.status !== 0) throw new Error("Process inspection failed");
  return result.stdout.trim().split("\n").flatMap((line) => {
    const match = line.match(/^\s*(\d+)\s+(\d+)\s+(\d+)\s+(.*)$/);
    return match ? [{
      pid: Number(match[1]),
      parentPid: Number(match[2]),
      processGroup: Number(match[3]),
      command: match[4],
    }] : [];
  });
}

function childProcessTable(rootPid, privateValues) {
  if (!rootPid) return "pid ppid pgid command\n<desktop process unavailable>";
  try {
    const rows = processRows();
    const included = new Set([rootPid]);
    let changed = true;
    while (changed) {
      changed = false;
      for (const row of rows) {
        if (included.has(row.parentPid) && !included.has(row.pid)) {
          included.add(row.pid);
          changed = true;
        }
      }
    }
    const selected = rows.filter(({ pid }) => included.has(pid)).slice(0, 32);
    const body = selected.length === 0
      ? ["<no desktop process rows>"]
      : selected.map(({ pid, parentPid, processGroup, command }) =>
        `${pid} ${parentPid} ${processGroup} ${sanitizeDiagnosticText(command, privateValues).slice(0, 300)}`);
    return ["pid ppid pgid command", ...body].join("\n");
  } catch (error) {
    return `pid ppid pgid command\n<process inspection unavailable: ${sanitizeDiagnosticText(String(error), privateValues)}>`;
  }
}

function createBoundedOutput(limit = diagnosticOutputLimit) {
  let output = "";
  return {
    append(chunk) {
      output = `${output}${chunk.toString("utf8")}`.slice(-limit);
    },
    read() {
      return output;
    },
  };
}

function formatTimeoutDiagnostics(error, diagnostics, childPid) {
  const safe = (value) => sanitizeDiagnosticText(value || "<none>", diagnostics.privateValues);
  return [
    error.message,
    `Active stage: ${safe(diagnostics.activeStage)}`,
    `Last accessibility failure: ${safe(diagnostics.lastAccessibilityFailure)}`,
    `App stdout, last ${diagnosticOutputLimit} characters: ${safe(diagnostics.appStdout.read())}`,
    `App stderr, last ${diagnosticOutputLimit} characters: ${safe(diagnostics.appStderr.read())}`,
    `Child-process table:\n${childProcessTable(childPid, diagnostics.privateValues)}`,
  ].join("\n");
}

function sanitizeDiagnosticText(value, privateValues) {
  let safe = String(value);
  for (const privateValue of privateValues) {
    if (privateValue) safe = safe.replaceAll(privateValue, "<private-data-directory>");
  }
  return safe
    .replace(/teamarness-workspace-v1\.[^\s"'<>]+/giu, "<redacted-workspace-invitation>")
    .replace(/((?:workspace[ _-]?invitation|invitation|private[ _-]?key|secret(?:[ _-]?key)?|database(?:[ _-]?(?:content|value))?)\s*[:=]\s*)("[^"]*"|'[^']*'|[^\s,}]+)/giu, "$1<redacted>")
    .replace(/\b[0-9a-f]{32,}\b/giu, "<redacted-long-value>")
    .replace(/\b[A-Za-z0-9+/_-]{48,}={0,2}\b/gu, "<redacted-long-value>");
}

function accessibilityScript(pid, body) {
  return `
    on namedDescendant(rootElement, expectedName, expectedRole)
      tell application "System Events" to set descendants to entire contents of rootElement
      repeat with candidateReference in descendants
        try
          set candidateElement to contents of candidateReference
          tell application "System Events"
            set candidateName to name of candidateElement
            set candidateRole to role of candidateElement
          end tell
          if candidateName is expectedName and candidateRole is expectedRole then return candidateElement
        end try
      end repeat
      error "Missing accessible " & expectedRole & " named " & expectedName
    end namedDescendant

    on namedDescendantStartingWith(rootElement, expectedPrefix, expectedRole)
      tell application "System Events" to set descendants to entire contents of rootElement
      repeat with candidateReference in descendants
        try
          set candidateElement to contents of candidateReference
          tell application "System Events"
            set candidateName to name of candidateElement
            set candidateRole to role of candidateElement
          end tell
          if candidateName starts with expectedPrefix and candidateRole is expectedRole then return candidateElement
        end try
      end repeat
      error "Missing accessible " & expectedRole & " beginning with " & expectedPrefix
    end namedDescendantStartingWith

    on subroleDescendant(rootElement, expectedSubrole)
      tell application "System Events" to set descendants to entire contents of rootElement
      repeat with candidateReference in descendants
        try
          set candidateElement to contents of candidateReference
          tell application "System Events" to set candidateSubrole to subrole of candidateElement
          if candidateSubrole is expectedSubrole then return candidateElement
        end try
      end repeat
      error "Missing accessible landmark " & expectedSubrole
    end subroleDescendant

    tell application "System Events"
      set appProcess to first process whose unix id is ${pid}
      tell appProcess
        set frontmost to true
        set appUi to UI element 1 of scroll area 1 of group 1 of group 1 of window 1
        ${body}
      end tell
    end tell
  `;
}

async function runAppleScript(source, label) {
  const result = spawnSync("osascript", ["-e", source], { encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`${label} accessibility action failed: ${appleScriptFailure(result)}`);
  }
  return result.stdout.trim();
}

async function waitForAppleScript(source, label, recordFailure) {
  return waitFor(async () => {
    const result = spawnSync("osascript", ["-e", source], { encoding: "utf8" });
    if (result.status !== 0) {
      recordFailure(appleScriptFailure(result));
      return undefined;
    }
    return result.stdout.trim() || undefined;
  }, label);
}

function appleScriptFailure(result) {
  return result.stderr.trim() || result.error?.message || `osascript exited with status ${result.status}`;
}

async function waitFor(operation, label, timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const value = await operation();
    if (value) return value;
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new SmokeTimeoutError(`Timed out waiting for ${label}`);
}

async function registrationExists(dataDirectory) {
  try {
    await stat(join(dataDirectory, "packages/registrations.v1.json"));
    return true;
  } catch {
    return false;
  }
}

async function waitForExit(child, label) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  await Promise.race([
    new Promise((resolve) => child.once("exit", resolve)),
    new Promise((_, reject) => setTimeout(() => reject(new SmokeTimeoutError(`Timed out waiting for ${label}`)), 15_000)),
  ]);
}

function processExists(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

async function assertFile(path) {
  assert.ok((await stat(path)).isFile(), `Missing packaged artifact: ${path}`);
}

async function makeWritable(path) {
  const entries = await readdir(path, { withFileTypes: true }).catch(() => []);
  for (const entry of entries) {
    const child = join(path, entry.name);
    if (entry.isDirectory()) await makeWritable(child);
    await chmod(child, entry.isDirectory() ? 0o700 : 0o600).catch(() => {});
  }
}
