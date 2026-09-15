import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import { test } from "node:test";

const root = new URL("../", import.meta.url);
const read = async (path) => readFile(new URL(path, root), "utf8");

const requiredGuidance = {
  "README.md": [
    "Package author guide",
    "Local Package registration",
    "Package Processes have the current user's ambient filesystem, network, and process authority",
    "same-webview Package UI",
  ],
  "docs/package-author-guide.md": [
    "Package Directory",
    "aarch64-apple-darwin",
    "teamarness.package.json",
    "UI contract",
    "teamarness-package://",
    "Promotion to application distribution",
  ],
  "docs/local-package-registration.md": [
    "registrations.v1.json",
    "trusted_native_code_v1",
    "expectedIdentity",
    "overrideApplication",
    "restart",
    "Package Data Directory",
  ],
  "crates/teamarness-package-protocol/README.md": [
    "language-neutral",
    "contracts/package-process/v1",
    "1,048,576",
  ],
  "crates/teamarness-package-host/README.md": [
    "PackageHost::open",
    "PackageHost::snapshot",
    "PackageHost::execute",
    "PackageHost::subscribe",
  ],
  "crates/teamarness-package-sdk/README.md": [
    "Backend",
    "Package::builder",
    "CapabilityClient",
    "Rust",
  ],
};

test("Package author and operator guidance covers the shipped contract", async () => {
  for (const [path, phrases] of Object.entries(requiredGuidance)) {
    const text = await read(path);
    for (const phrase of phrases) {
      assert.ok(text.includes(phrase), `${path} must document ${phrase}`);
    }
  }

  const combined = (await Promise.all(Object.keys(requiredGuidance).map(read))).join("\n");
  for (const phrase of [
    "not a sandbox",
    "same-webview",
    "DOM",
    "Tauri command",
    "signing and notarization",
    "remote Package fetching",
    "hot reload",
  ]) {
    assert.ok(combined.includes(phrase), `guidance must state ${phrase}`);
  }
});

test("Tutorial preview and learner Emojart guide keep both local Packages distinct", async () => {
  const tutorial = await read("docs/build-your-first-package.md");
  const readme = await read("README.md");

  for (const phrase of [
    "Core Runtime",
    "Shell",
    "Package Host",
    "Package Backend",
    "Package UI",
    "Package Data Directory",
    "Package Source",
    "Package Instance",
    "dev.teamarness.tutorial",
    "dev.teamarness.emojart",
    "npm run setup:tutorial -- tutorial-preview",
    "npm run build:tutorial",
    "npm run build:emojart",
    "registrations.v1.json",
    ".teamarness-dev/tutorial-preview/packages/registrations.v1.json",
    "packages/tutorial/",
    "packages/emojart/",
    "Do not delete, rename, or modify Tutorial",
    "Do not delete the Tutorial entry",
    "trusted_native_code_v1",
    "There is no hot reload",
    "not a sandbox",
    "future tutorial",
    "synchronization",
    "Workspace-specific",
    "installation-local Package Source",
    "synchronized team behavior",
    "owner-only",
    "artifact_changed",
    "Digest mismatch",
    "Unsupported target",
    "Invalid UI contract",
  ]) {
    assert.ok(tutorial.includes(phrase), `tutorial must include ${phrase}`);
  }
  assert.match(tutorial, /```mermaid/g);
  assert.equal((tutorial.match(/```mermaid/g) ?? []).length, 2, "tutorial has its two planned diagrams");
  assert.equal((tutorial.match(/### Agent brief/g) ?? []).length, 5, "tutorial has its five copyable agent briefs");
  assert.equal((tutorial.match(/### Human checkpoint/g) ?? []).length, 5, "tutorial has its five human checkpoints");
  assert.ok(readme.includes("Build your first Package"), "README links the tutorial from Package development");
  assert.ok(readme.includes("npm run setup:tutorial -- tutorial-preview"), "README documents the Tutorial setup command");

  const packageJson = JSON.parse(await read("package.json"));
  assert.equal(packageJson.scripts["build:tutorial"], "node scripts/build-tutorial-package.mjs");
  assert.equal(packageJson.scripts["setup:tutorial"], "node scripts/setup-tutorial-package.mjs");
  assert.ok(packageJson.workspaces.includes("packages/tutorial/ui"));
  assert.ok(!packageJson.workspaces.includes("packages/emojart/ui"));

  const packageDirectories = await readdir(new URL("packages/", root));
  assert.ok(packageDirectories.includes("tutorial"));
  assert.ok(!packageDirectories.includes("emojart"), "checked-in Tutorial source does not occupy the learner Emojart directory");

  const tutorialManifest = JSON.parse(await read("packages/tutorial/teamarness.package.json"));
  assert.equal(tutorialManifest.identity, "dev.teamarness.tutorial");
  assert.equal(tutorialManifest.presentation.displayName, "Tutorial");
  const tutorialContract = JSON.parse(await read("packages/tutorial/ui-contract.json"));
  assert.deepEqual(Object.keys(tutorialContract.requests).sort(), ["tutorial.generate", "tutorial.read", "tutorial.save"]);

  const applicationPackages = await read("packages/application-packages.v1.json");
  const tauriConfig = await read("apps/teamarness-desktop/src-tauri/tauri.conf.json");
  for (const identity of ["dev.teamarness.tutorial", "dev.teamarness.emojart"]) {
    assert.ok(!applicationPackages.includes(identity), `${identity} stays out of application Package metadata`);
    assert.ok(!tauriConfig.includes(identity), `${identity} stays out of Tauri bundled resources`);
  }
  for (const identifier of ["tutorial", "emojart"]) {
    assert.ok(!tauriConfig.includes(identifier), `${identifier} stays out of Tauri bundled resources`);
  }
});

test("direct dependencies are exact, lockfiles are committed, and schema retrieval is disabled", async () => {
  const cargo = await read("Cargo.toml");
  const dependencyBlock = cargo.split("[workspace.dependencies]")[1];
  for (const line of dependencyBlock.split("\n").filter((line) => /^[a-z0-9_-]+\s*=/.test(line))) {
    if (line.includes("path =")) continue;
    assert.match(line, /(?:version\s*=\s*)?"=\d+\.\d+\.\d+"/, `Cargo dependency is not exact: ${line}`);
  }
  assert.match(cargo, /jsonschema\s*=\s*\{[^\n]*default-features\s*=\s*false/);

  for (const path of ["package.json", "apps/teamarness-desktop/package.json"]) {
    const packageJson = JSON.parse(await read(path));
    for (const group of ["dependencies", "devDependencies"]) {
      for (const [name, version] of Object.entries(packageJson[group] ?? {})) {
        if (name.startsWith("@teamarness/")) continue;
        assert.match(version, /^\d+\.\d+\.\d+$/, `${path} does not exactly pin ${name}`);
      }
    }
  }
  assert.equal(JSON.parse(await read("package-lock.json")).lockfileVersion, 3);
  assert.ok((await read("Cargo.lock")).includes("version = 4"));
});

test("delivery configuration has one local admitted loader and no deferred Package subsystem", async () => {
  const packageJson = JSON.parse(await read("package.json"));
  assert.equal(packageJson.scripts["verify:packaged-app"], "node scripts/smoke-packaged-app.mjs");

  const config = JSON.parse(await read("apps/teamarness-desktop/src-tauri/tauri.conf.json"));
  assert.deepEqual(config.bundle.targets, ["app", "dmg"]);
  assert.ok(config.bundle.resources["../../../packages/application-packages.v1.json"]);
  assert.ok(config.bundle.resources["../../../target/teamarness-packages/dev.teamarness.reference/"]);
  assert.ok(config.app.security.csp.includes("teamarness-package:"));
  assert.ok(!config.app.security.csp.includes("file:"));
  assert.ok(!config.app.security.csp.includes("https:"));

  const capabilityEntries = await readdir(new URL("contracts/package-capabilities/v1/", root));
  assert.deepEqual(
    capabilityEntries.sort(),
    ["README.md", "membership.schema.json", "signing-envelope.golden.json", "signing.schema.json", "workspace-context.schema.json"],
  );

  const productionPaths = [
    "apps/teamarness-desktop/src/package-bridge.ts",
    "apps/teamarness-desktop/src/package-contracts.ts",
    "apps/teamarness-desktop/src/package-mount.ts",
    "apps/teamarness-desktop/src-tauri/src/package_commands.rs",
    "apps/teamarness-desktop/src-tauri/src/package_dto.rs",
    "apps/teamarness-desktop/src-tauri/src/package_protocol.rs",
    "crates/teamarness-package-host/src/lib.rs",
  ];
  const production = (await Promise.all(productionPaths.map(async (path) =>
    (await read(path)).split("#[cfg(test)]")[0]))).join("\n").toLowerCase();
  for (const forbidden of [
    "reference.read",
    "reference.increment",
    "reference.changes",
    "conversation",
    "webviewwindow",
    "convertfilesrc",
    "source_path",
    "cache_path",
    "data_path",
    "iroh::",
  ]) {
    assert.ok(!production.includes(forbidden), `production Package seam contains ${forbidden}`);
  }
});
