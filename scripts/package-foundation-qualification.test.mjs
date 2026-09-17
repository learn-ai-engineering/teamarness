import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import { test } from "node:test";

import { applicationIndexDocument, readBundledPackages, tauriBundleConfiguration } from "./bundled-packages.mjs";
import { waitForDesktopCleanup } from "./packaged-app-cleanup.mjs";

const root = new URL("../", import.meta.url);
const read = async (path) => readFile(new URL(path, root), "utf8");

test("Package authoring guidance follows the local Package development path", async () => {
  const guide = await read("docs/build-your-first-package.md");
  const readme = await read("README.md");
  for (const phrase of [
    "Checked-in Tutorial source",
    "`packages/tutorial/`",
    "`packages/dev.teamarness.tutorial/`",
    "`dev.teamarness.tutorial`",
    "Package Source values `bundled` and `local`",
    "Shell presents bundled Tutorial as `Team` and registered local Emojart as `Personal`",
    "Team` and `Personal` are UI aliases, not trust verdicts",
    "registration scope, Workspace enablement, synchronization, sharing, networking, or collaboration",
    "Teamarness Core Runtime",
    "Package Host",
    "Package UI",
    "Package Backend",
    "Package Data Directory",
    "Save several pieces and click between them",
    "The canonical recipe includes",
    "Changing only the seed must change the cells",
    "A changed seed field with identical cells does not pass",
    "horizontal and vertical sine waves",
    "radial distance from the grid center",
    "polar angle with normalized distance",
    "five seed-stable control points",
    "Do not add independent random noise to each cell",
    "palette dropdown first",
    "no visible labels",
    "$HOME/Projects/teamarness/local-packages",
    "separate project directory",
    "registration entry is the only connection",
    "teamarness.package.json",
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

test("Package UI guidance publishes every theme token and opt-in kit class", async () => {
  const sdk = await read("packages/teamarness-package-ui-sdk/README.md");
  for (const token of [
    "--teamarness-color-background",
    "--teamarness-color-panel",
    "--teamarness-color-text",
    "--teamarness-color-text-muted",
    "--teamarness-color-border",
    "--teamarness-color-accent",
    "--teamarness-color-danger",
    "--teamarness-color-focus",
    "--teamarness-font-body",
    "--teamarness-font-mono",
    "--teamarness-radius-control",
    "--teamarness-radius-panel",
  ]) {
    assert.ok(sdk.includes(token), `UI SDK guidance must document ${token}`);
  }
  for (const className of [
    ".teamarness-control",
    ".teamarness-control--primary",
    ".teamarness-control--secondary",
    ".teamarness-field",
    ".teamarness-input",
    ".teamarness-help",
    ".teamarness-validation",
    ".teamarness-notice",
    ".teamarness-notice--danger",
    ".teamarness-panel",
    ".teamarness-stack",
    ".teamarness-inline",
    ".teamarness-focus",
  ]) {
    assert.ok(sdk.includes(className), `UI SDK guidance must document ${className}`);
  }
});

test("Package styles preserve the strict host policy and avoid inline style APIs", async () => {
  const config = JSON.parse(await read("apps/teamarness-desktop/src-tauri/tauri.conf.json"));
  const styleDirective = config.app.security.csp
    .split(";")
    .map((directive) => directive.trim().split(/\s+/))
    .find(([name]) => name === "style-src");
  assert.deepEqual(styleDirective, ["style-src", "'self'", "teamarness-package:"]);

  for (const path of [
    "packages/teamarness-package-ui-sdk/src/index.ts",
    "packages/reference-package/ui/src/index.ts",
    "packages/tutorial/ui/src/index.ts",
  ]) {
    const source = await read(path);
    assert.doesNotMatch(source, /createElement\(\s*["']style["']/, `${path} must not create inline style elements`);
    assert.doesNotMatch(source, /\bstyle\.textContent\b/, `${path} must not write style text content`);
    assert.doesNotMatch(source, /setAttribute\(\s*["']style["']/, `${path} must not set style attributes`);
    assert.doesNotMatch(source, /\.style(?:\.|\[|\s*=)/, `${path} must not mutate style attributes`);
    assert.match(source, /adoptedStyleSheets/, `${path} must use adopted stylesheets`);
  }
});

test("desktop cleanup preserves its timeout after emergency termination", async () => {
  const timeout = new Error([
    "Timed out waiting for desktop cleanup",
    "Active stage: desktop cleanup",
    "Last accessibility failure: missing window",
    "App stdout, last 4096 characters: bounded stdout",
    "App stderr, last 4096 characters: bounded stderr",
    "Child-process table:",
  ].join("\n"));
  const signals = [];

  await assert.rejects(
    waitForDesktopCleanup(
      async () => { throw timeout; },
      (signal) => signals.push(signal),
    ),
    (error) => error === timeout,
  );
  assert.deepEqual(signals, ["SIGKILL"]);
});

test("packaged smoke waits for Package navigation and captures safe timeout diagnostics", async () => {
  const source = await read("scripts/smoke-packaged-app.mjs");
  const shellReadinessWait = source.indexOf('"desktop Shell readiness"');
  const workspaceCreation = source.indexOf('diagnostics.activeStage = "Workspace creation"');
  const navigationWait = source.indexOf('"bundled reference Package navigation"');
  const backendWait = source.indexOf("`bundled Package Backend ${identity}`");

  assert.ok(shellReadinessWait >= 0, "packaged smoke must wait for desktop Shell readiness");
  assert.ok(workspaceCreation > shellReadinessWait, "packaged smoke must wait for Shell readiness before creating a Workspace");
  assert.ok(navigationWait >= 0, "packaged smoke must wait for bundled Package navigation");
  assert.ok(backendWait > navigationWait, "packaged smoke must wait for navigation before inspecting the Backend");
  assert.match(source, /createBoundedOutput/);
  assert.match(source, /formatTimeoutDiagnostics/);
  assert.match(source, /activeStage/);
  assert.match(source, /lastAccessibilityFailure/);
  assert.match(source, /childProcessTable/);
  assert.match(source, /stdio:\s*\["ignore",\s*"pipe",\s*"pipe"\]/);
});

test("generated foundation and bundled Package policies remain independent", async () => {
  const packageJson = JSON.parse(await read("package.json"));
  const agents = await read("templates/package-foundation/v1/AGENTS.md");
  assert.match(agents, /Teamarness Core Runtime/);
  assert.match(agents, /Package Host/);
  assert.match(agents, /PackageClient/);
  assert.match(agents, /Package Data Directory/);
  assert.match(agents, /Semantic Capabilities/);
  assert.match(agents, /metadata changes while the meaningful output stays fixed/);
  assert.equal(packageJson.scripts["create:package"], "node scripts/create-package.mjs");
  assert.equal(packageJson.scripts["build:application-packages"], "node scripts/build-application-packages.mjs");
  assert.equal(
    packageJson.scripts.build,
    "npm --workspace @teamarness/package-ui-sdk run build && npm run build:application-packages && npm --workspace teamarness-desktop run build",
  );
  assert.equal(packageJson.scripts[["setup", "tutorial"].join(":")], undefined);
  assert.ok(!packageJson.workspaces.includes(["packages", "emojart", "ui"].join("/")));
  assert.ok((await readdir(new URL("packages/", root))).includes("tutorial"));

  const index = JSON.parse(await read("packages/application-packages.v1.json"));
  const bundled = await readBundledPackages();
  assert.deepEqual(bundled.map(({ identity }) => identity), [
    "dev.teamarness.reference",
    "dev.teamarness.tutorial",
    "dev.teamarness.conversations",
    "dev.teamarness.repo-aware",
  ]);
  assert.deepEqual(index, applicationIndexDocument(bundled));
  const tutorial = JSON.parse(await read("packages/tutorial/ui-contract.json"));
  assert.deepEqual(Object.keys(tutorial.requests).sort(), ["tutorial.generate", "tutorial.read", "tutorial.save"]);
  assert.deepEqual(tutorial.subscriptions, {});
  assert.deepEqual(tutorial["$defs"].algorithmId.enum, ["waves", "ripples", "spiral", "patchwork"]);
  assert.deepEqual(tutorial["$defs"].paletteId.enum, ["garden", "ocean", "moon", "fruit"]);
  assert.deepEqual(tutorial["$defs"].paletteChoice.oneOf, [
    { const: { id: "garden", label: "Garden", values: ["🌱", "🌿", "🍀", "🌸", "🌻"] } },
    { const: { id: "ocean", label: "Ocean", values: ["💧", "🐚", "🐠", "🐬", "🌊"] } },
    { const: { id: "moon", label: "Moon", values: ["🌑", "🌒", "🌓", "🌔", "🌕"] } },
    { const: { id: "fruit", label: "Fruit", values: ["🍓", "🍊", "🍋", "🍏", "🫐"] } },
  ]);
});

test("delivery configuration includes all bundled Package Directories and source badges", async () => {
  const config = JSON.parse(await read("apps/teamarness-desktop/src-tauri/tauri.conf.json"));
  const expected = tauriBundleConfiguration(await readBundledPackages());
  assert.deepEqual(config.bundle.resources, expected.resources);
  assert.deepEqual(config.bundle.externalBin, expected.externalBin);
  const shell = await read("apps/teamarness-desktop/src/shell.ts");
  const presentation = await read("apps/teamarness-desktop/src/presentation.ts");
  assert.match(shell, /Package source/);
  assert.match(shell, /Team/);
  assert.match(shell, /Personal/);
  assert.match(presentation, /catalog\.source/);
  assert.match(presentation, /catalog\.source === "local" \? "Personal" : "Team"/);
});

test("repository authoring records the temporary vendoring policy", async () => {
  const decision = await read("context/.decisions/0010-vendored-package-authoring.md");
  const roadmap = await read("context/roadmap.md");
  const authorGuide = await read("docs/package-author-guide.md");
  const registrationGuide = await read("docs/local-package-registration.md");
  assert.match(authorGuide, /Package Source values `bundled` and `local`/);
  assert.match(authorGuide, /`Team` and `Personal`/);
  assert.match(registrationGuide, /Package Source `local` as `Personal`/);
  assert.match(registrationGuide, /Package Source `bundled` as `Team`/);
  for (const guide of [authorGuide, registrationGuide]) {
    for (const boundary of ["trust", "registration scope", "Workspace enablement", "synchronization", "sharing", "networking", "collaboration"]) {
      assert.match(guide, new RegExp(boundary));
    }
  }
  assert.match(authorGuide, /application maintainer/);
  assert.match(authorGuide, /builder remains the only writer/);
  assert.match(authorGuide, /npm run build:application-packages/);
  assert.doesNotMatch(authorGuide, /Check the Shell's source, trust, version/);
  assert.doesNotMatch(authorGuide, /Add its resource-relative directory/);
  for (const phrase of ["vendored", "versioned", "upgrade policy", "no updater", "production authoring"]) {
    assert.ok(decision.toLowerCase().includes(phrase), `decision must include ${phrase}`);
  }
  assert.match(roadmap, /published.*Rust.*TypeScript.*SDK/i);
  assert.match(roadmap, /upgrade policy/i);
  assert.equal(JSON.parse(await read("package-lock.json")).lockfileVersion, 3);
  assert.ok((await read("Cargo.lock")).includes("version = 4"));
});

test("delivery configuration has one local admitted loader and no deferred Package subsystem", async () => {
  const packageJson = JSON.parse(await read("package.json"));
  assert.equal(packageJson.scripts["verify:packaged-app"], "node scripts/smoke-packaged-app.mjs");

  const config = JSON.parse(await read("apps/teamarness-desktop/src-tauri/tauri.conf.json"));
  assert.deepEqual(config.bundle.targets, ["app", "dmg"]);
  assert.deepEqual(config.bundle.resources, tauriBundleConfiguration(await readBundledPackages()).resources);
  assert.ok(config.app.security.csp.includes("teamarness-package:"));
  assert.ok(!config.app.security.csp.includes("file:"));
  assert.ok(!config.app.security.csp.includes("https:"));

  const capabilityEntries = await readdir(new URL("contracts/package-capabilities/v1/", root));
  assert.deepEqual(
    capabilityEntries.sort(),
    ["README.md", "immutable-record-invalid.golden.json", "immutable-record-log.schema.json", "immutable-record-log.test.mjs", "immutable-record.golden.json", "membership.schema.json", "signing-envelope.golden.json", "signing.schema.json", "workspace-context.schema.json"],
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
