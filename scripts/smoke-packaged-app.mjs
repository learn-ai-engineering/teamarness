import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { chmod, mkdtemp, readdir, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { repositoryRoot } from "./build-application-packages.mjs";
import { compareDeclaredPackageBytes } from "./package-directory.mjs";

const app = join(repositoryRoot, "target/release/bundle/macos/Teamarness.app");
const executable = join(app, "Contents/MacOS/teamarness-desktop");
const resources = join(app, "Contents/Resources/packages/dev.teamarness.reference");
const tutorialResources = join(app, "Contents/Resources/packages/dev.teamarness.tutorial");
const staged = join(repositoryRoot, "target/teamarness-packages/dev.teamarness.reference");
const stagedTutorial = join(repositoryRoot, "target/teamarness-packages/dev.teamarness.tutorial");
const dmg = join(repositoryRoot, "target/release/bundle/dmg/Teamarness_0.1.0_aarch64.dmg");

if (process.platform !== "darwin" || process.arch !== "arm64") {
  throw new Error("The packaged application smoke requires Apple silicon macOS");
}

await assertFile(executable);
await assertFile(dmg);
await compareDeclaredPackageBytes(staged, resources);
await compareDeclaredPackageBytes(stagedTutorial, tutorialResources);
await inspectDmg();

const dataDirectory = await mkdtemp(join(tmpdir(), "teamarness-packaged-smoke-"));
try {
  await smokeLaunch(dataDirectory, true);
  await smokeLaunch(dataDirectory, false);
  console.log("Packaged app smoke passed twice with admitted UI, protocol traffic, and clean process reaping.");
} finally {
  await chmod(dataDirectory, 0o700).catch(() => {});
  await makeWritable(dataDirectory);
  await rm(dataDirectory, { recursive: true, force: true });
}

async function smokeLaunch(dataDirectory, createWorkspace) {
  const desktop = spawn(executable, ["--data-dir", dataDirectory], {
    cwd: repositoryRoot,
    detached: false,
    stdio: "ignore",
  });
  const childPid = await waitFor(() => desktop.pid, "desktop process");
  const backendPids = [];
  try {
    await waitForAppleScript(accessibilityScript(childPid, `
      return name of window 1
    `), "desktop window");

    if (createWorkspace) {
      await runAppleScript(accessibilityScript(childPid, `
        set workspaceSwitcher to my namedDescendant(appUi, "Workspace switcher", "AXGroup")
        set activeWorkspaceNavigation to my namedDescendant(appUi, "Active Workspace navigation", "AXGroup")
        set selectedContent to my subroleDescendant(appUi, "AXLandmarkMain")
        set workspaceNameField to my namedDescendant(selectedContent, "Workspace name", "AXTextField")
        set createButton to my namedDescendant(selectedContent, "Create", "AXButton")
        set focused of workspaceNameField to true
        keystroke "Packaged smoke"
        click createButton
      `), "Workspace creation");
      assert.equal(await registrationExists(dataDirectory), false, "fresh bundled Tutorial created a registration file");
    }

    const referenceBackend = join(resources, "backend/teamarness-reference-package-aarch64-apple-darwin");
    const tutorialBackend = join(tutorialResources, "backend/teamarness-tutorial-package-aarch64-apple-darwin");
    const referenceBackendPid = await waitFor(
      () => bundledBackend(childPid, referenceBackend),
      "bundled Reference Backend",
    );
    backendPids.push(referenceBackendPid);
    assertBundledBackend(referenceBackendPid, childPid, referenceBackend, "Reference");
    const tutorialBackendPid = await waitFor(
      () => bundledBackend(childPid, tutorialBackend),
      "bundled Tutorial Backend",
    );
    backendPids.push(tutorialBackendPid);
    assertBundledBackend(tutorialBackendPid, childPid, tutorialBackend, "Tutorial");

    await waitForAppleScript(accessibilityScript(childPid, `
      set activeWorkspaceNavigation to my namedDescendant(appUi, "Active Workspace navigation", "AXGroup")
      set teamPackages to my namedDescendant(activeWorkspaceNavigation, "Team Packages", "AXGroup")
      set tutorialPackage to my namedDescendantStartingWith(teamPackages, "Tutorial", "AXButton")
      return name of tutorialPackage
    `), "bundled Tutorial navigation");
    await runAppleScript(accessibilityScript(childPid, `
      set activeWorkspaceNavigation to my namedDescendant(appUi, "Active Workspace navigation", "AXGroup")
      set teamPackages to my namedDescendant(activeWorkspaceNavigation, "Team Packages", "AXGroup")
      set tutorialPackage to my namedDescendantStartingWith(teamPackages, "Tutorial", "AXButton")
      click tutorialPackage
    `), "bundled Tutorial selection");
    await waitFor(async () => {
      const contents = spawnSync("osascript", ["-e", accessibilityScript(childPid, `
        set selectedContent to my subroleDescendant(appUi, "AXLandmarkMain")
        set packageInterface to my namedDescendant(selectedContent, "Package interface", "AXGroup")
        return entire contents of packageInterface
      `)], { encoding: "utf8" });
      return contents.status === 0
        && contents.stdout.includes("Generate")
        && contents.stdout.includes("Save")
        && contents.stdout.includes("Waves")
        && contents.stdout.includes("Garden");
    }, "bundled Tutorial UI request response");

    await waitForAppleScript(accessibilityScript(childPid, `
      set activeWorkspaceNavigation to my namedDescendant(appUi, "Active Workspace navigation", "AXGroup")
      set teamPackages to my namedDescendant(activeWorkspaceNavigation, "Team Packages", "AXGroup")
      set referencePackage to my namedDescendantStartingWith(teamPackages, "Reference Package", "AXButton")
      return name of referencePackage
    `), "bundled reference Package navigation");
    await runAppleScript(accessibilityScript(childPid, `
      set activeWorkspaceNavigation to my namedDescendant(appUi, "Active Workspace navigation", "AXGroup")
      set teamPackages to my namedDescendant(activeWorkspaceNavigation, "Team Packages", "AXGroup")
      set referencePackage to my namedDescendantStartingWith(teamPackages, "Reference Package", "AXButton")
      click referencePackage
    `), "bundled reference Package selection");
    let lastPackageUiFailure = "";
    try {
      await waitFor(async () => {
        const contents = spawnSync("osascript", ["-e", accessibilityScript(childPid, `
          set selectedContent to my subroleDescendant(appUi, "AXLandmarkMain")
          set packageInterface to my namedDescendant(selectedContent, "Package interface", "AXGroup")
          return entire contents of packageInterface
        `)], { encoding: "utf8" });
        if (contents.status !== 0) {
          lastPackageUiFailure = appleScriptFailure(contents);
          return undefined;
        }
        const visible = contents.stdout;
        return visible.includes("Signing round trip")
          && visible.includes("Verified")
          && visible.includes("Increment counter");
      }, "custom-scheme Package UI");
    } catch (error) {
      const detail = lastPackageUiFailure ? `; last accessibility error: ${lastPackageUiFailure}` : "";
      throw new Error(`${error instanceof Error ? error.message : String(error)}${detail}`);
    }

    await runAppleScript(`
      tell application "System Events"
        set appProcess to first process whose unix id is ${childPid}
        tell appProcess
          set frontmost to true
          keystroke "q" using command down
        end tell
      end tell
    `, "desktop shutdown");
    await waitForExit(desktop, "desktop graceful shutdown");
    for (const backendPid of backendPids) {
      await waitFor(() => !processExists(backendPid), `Package process ${backendPid} reaping`);
    }
  } finally {
    if (processExists(childPid)) desktop.kill("SIGTERM");
    await waitForExit(desktop, "desktop cleanup").catch(() => desktop.kill("SIGKILL"));
    for (const backendPid of backendPids) {
      await waitFor(() => !processExists(backendPid), `Package process ${backendPid} cleanup`, 5_000);
    }
  }
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

async function waitForAppleScript(source, label) {
  let lastFailure = "";
  try {
    return await waitFor(async () => {
      const result = spawnSync("osascript", ["-e", source], { encoding: "utf8" });
      if (result.status !== 0) {
        lastFailure = appleScriptFailure(result);
        return undefined;
      }
      return result.stdout.trim() || undefined;
    }, label);
  } catch (error) {
    const detail = lastFailure ? `; last accessibility error: ${lastFailure}` : "";
    throw new Error(`${error instanceof Error ? error.message : String(error)}${detail}`);
  }
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
  throw new Error(`Timed out waiting for ${label}`);
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
    new Promise((_, reject) => setTimeout(() => reject(new Error(`Timed out waiting for ${label}`)), 15_000)),
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
