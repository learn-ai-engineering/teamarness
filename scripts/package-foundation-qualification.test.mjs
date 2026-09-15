import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import { test } from "node:test";

const root = new URL("../", import.meta.url);
const read = async (path) => readFile(new URL(path, root), "utf8");

test("Package authoring guidance distinguishes checked-in, bundled, and local sources", async () => {
  const guide = await read("docs/build-your-first-package.md");
  const readme = await read("README.md");
  for (const phrase of [
    "Checked-in source",
    "Bundled Package",
    "Learner-owned local Package",
    "packages/tutorial/",
    "packages/dev.teamarness.tutorial/",
    "dev.teamarness.tutorial",
    "dev.teamarness.emojart",
    "npm run create:package --",
    "--output",
    "--identity dev.teamarness.emojart",
    "--display-name Emojart",
    "teamarness-authoring.json",
    "vendor/teamarness/",
    "Cargo.lock",
    "package-lock.json",
    "aarch64-apple-darwin",
    "registrations.v1.json",
    "restart",
    "trusted_native_code_v1",
    "not a sandbox",
    "same-webview",
    "There is no hot reload",
    "Workspace enablement",
    "synchronization",
    "sharing",
    "collaboration",
  ]) assert.ok(guide.includes(phrase), `guide must include ${phrase}`);
  assert.ok(readme.includes("npm run create:package --"));
  assert.equal((guide.match(/Agent brief/g) ?? []).length, 5);
  assert.equal((guide.match(/### Human checkpoint/g) ?? []).length, 5);
  assert.equal((guide.match(/```mermaid/g) ?? []).length, 2);
});

test("generated foundation and Tutorial policy remain independent", async () => {
  const packageJson = JSON.parse(await read("package.json"));
  assert.equal(packageJson.scripts["create:package"], "node scripts/create-package.mjs");
  assert.equal(packageJson.scripts["build:application-packages"], "node scripts/build-application-packages.mjs");
  assert.equal(packageJson.scripts[["setup", "tutorial"].join(":")], undefined);
  assert.ok(!packageJson.workspaces.includes(["packages", "emojart", "ui"].join("/")));
  assert.ok((await readdir(new URL("packages/", root))).includes("tutorial"));

  const index = JSON.parse(await read("packages/application-packages.v1.json"));
  assert.deepEqual(index.packages, [
    { directory: "packages/dev.teamarness.reference" },
    { directory: "packages/dev.teamarness.tutorial" },
  ]);
  const tutorial = JSON.parse(await read("packages/tutorial/ui-contract.json"));
  assert.deepEqual(Object.keys(tutorial.requests).sort(), ["tutorial.generate", "tutorial.read", "tutorial.save"]);
  assert.deepEqual(tutorial.subscriptions, {});
  assert.deepEqual(tutorial["$defs"].algorithmId.enum, ["waves", "ripples", "spiral", "patchwork"]);
  assert.deepEqual(tutorial["$defs"].paletteId.enum, ["garden", "ocean", "space", "fruit"]);
  for (const emoji of ["🪻", "🌷", "🐝", "🪼", "🐙", "🐳", "⭐", "🪐", "🚀", "🍇", "🍉", "🍒"]) {
    assert.ok(JSON.stringify(tutorial).includes(emoji), `contract must include ${emoji}`);
  }
});

test("delivery configuration includes both bundled Package Directories and source badges", async () => {
  const config = JSON.parse(await read("apps/teamarness-desktop/src-tauri/tauri.conf.json"));
  assert.ok(config.bundle.resources["../../../target/teamarness-packages/dev.teamarness.reference/"]);
  assert.ok(config.bundle.resources["../../../target/teamarness-packages/dev.teamarness.tutorial/"]);
  assert.deepEqual(config.bundle.externalBin, [
    "../../../target/teamarness-packages/dev.teamarness.reference/backend/teamarness-reference-package",
    "../../../target/teamarness-packages/dev.teamarness.tutorial/backend/teamarness-tutorial-package",
  ]);
  const shell = await read("apps/teamarness-desktop/src/package-shell.ts");
  assert.match(shell, /Package source/);
  assert.match(shell, /Bundled/);
  assert.match(shell, /Local/);
  assert.match(shell, /catalog\?\.source/);
});

test("repository authoring records the temporary vendoring policy", async () => {
  const decision = await read("context/.decisions/0008-vendored-package-authoring.md");
  const roadmap = await read("context/roadmap.md");
  for (const phrase of ["vendored", "versioned", "upgrade policy", "no updater", "production authoring"]) {
    assert.ok(decision.toLowerCase().includes(phrase), `decision must include ${phrase}`);
  }
  assert.match(roadmap, /published.*Rust.*TypeScript.*SDK/i);
  assert.match(roadmap, /upgrade policy/i);
});
