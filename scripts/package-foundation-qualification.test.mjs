import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import { test } from "node:test";

import { applicationIndexDocument, readBundledPackages, tauriBundleConfiguration } from "./bundled-packages.mjs";
import { waitForDesktopCleanup } from "./packaged-app-cleanup.mjs";

const root = new URL("../", import.meta.url);
const read = async (path) => readFile(new URL(path, root), "utf8");

async function rustFiles(directory) {
  const entries = await readdir(new URL(`${directory}/`, root), { withFileTypes: true, recursive: true });
  const files = entries.filter((entry) => entry.isFile() && entry.name.endsWith(".rs"));
  return Promise.all(files.map(async (entry) => {
    const path = `${entry.parentPath.slice(new URL(root).pathname.length)}/${entry.name}`;
    return { path, source: await readFile(new URL(`${entry.parentPath}/${entry.name}`, "file://"), "utf8") };
  }));
}

async function sourceFiles(directory) {
  const entries = await readdir(new URL(`${directory}/`, root), { withFileTypes: true, recursive: true });
  const files = entries.filter((entry) => entry.isFile() && entry.name.endsWith(".ts"));
  return Promise.all(files.map(async (entry) => {
    const path = `${entry.parentPath.slice(new URL(root).pathname.length)}/${entry.name}`;
    return { path, source: await readFile(new URL(`${entry.parentPath}/${entry.name}`, "file://"), "utf8") };
  }));
}

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
    "sandboxed frame",
    "There is no hot reload",
    "npm run validate",
    "typedPackageClient",
    "Workspace enablement",
    "synchronization",
    "sharing",
    "collaboration",
  ]) assert.ok(guide.includes(phrase), `guide must include ${phrase}`);
  const authorGuide = await read("docs/package-author-guide.md");
  for (const phrase of ["teamarness-package-validate", "npm run validate", "typedPackageClient"]) {
    assert.ok(authorGuide.includes(phrase), `author guide must include ${phrase}`);
  }
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
  assert.deepEqual(styleDirective, ["style-src", "'self'"]);

  for (const path of [
    "packages/teamarness-package-ui-sdk/src/index.ts",
    "packages/reference-package/ui/src/index.ts",
    "packages/tutorial/ui/src/index.ts",
    "apps/teamarness-desktop/src/package-frame/runtime.ts",
  ]) {
    const source = await read(path);
    assert.doesNotMatch(source, /createElement\(\s*["']style["']/, `${path} must not create inline style elements`);
    assert.doesNotMatch(source, /\bstyle\.textContent\b/, `${path} must not write style text content`);
    assert.doesNotMatch(source, /setAttribute\(\s*["']style["']/, `${path} must not set style attributes`);
    assert.doesNotMatch(source, /\.style(?:\.|\[|\s*=)/, `${path} must not mutate style attributes`);
    assert.match(
      source,
      /adoptedStyleSheets|adoptPackageStyles\(/,
      `${path} must use adopted stylesheets, directly or through adoptPackageStyles`,
    );
  }
});

const PACKAGE_UI_DIRECTORIES = [
  ["packages/reference-package/ui", "ReferenceContract"],
  ["packages/tutorial/ui", "TutorialContract"],
  ["packages/conversations-package/ui", "ConversationsContract"],
  ["packages/repo-aware/ui", "RepoAwareContract"],
];

test("Package UIs use contract-typed requests and shared style adoption", async () => {
  for (const [directory, typeName] of PACKAGE_UI_DIRECTORIES) {
    const sources = (await sourceFiles(`${directory}/src`)).filter(({ path }) =>
      !path.endsWith(".test.ts") && !path.endsWith("/contract.generated.ts"));
    assert.ok(sources.length > 0, `${directory} was not read`);
    for (const { path, source } of sources) {
      assert.doesNotMatch(
        source,
        /request(<[^>]*>)?\([^)]*\)\s*as\s/,
        `${path} must not cast a request answer; the contract types it`,
      );
      assert.doesNotMatch(source, /\.request<\w/, `${path} must not name a response type by hand`);
      for (const styling of ["new StyleSheet()", "globalThis.CSSStyleSheet", "adoptedStyleSheets ="]) {
        assert.ok(!source.includes(styling), `${path} must adopt styles through adoptPackageStyles`);
      }
    }

    const generated = await read(`${directory}/src/contract.generated.ts`);
    assert.match(generated, /Generated from ui-contract\.json by teamarness-contract-types/);
    assert.match(generated, new RegExp(`export type ${typeName} = \\{`));

    const manifest = JSON.parse(await read(`${directory}/package.json`));
    assert.match(
      manifest.scripts.typecheck,
      /teamarness-contract-types .*--check/,
      `${directory} must check its generated contract types before tsc`,
    );
  }
});

const BACKEND_DIRECTORIES = [
  "packages/reference-package/backend",
  "packages/tutorial/backend",
  "packages/conversations-package/backend",
  "packages/repo-aware/backend",
  "templates/package-foundation/v1/backend",
];

// This test drives the real Backend executable over its stdin lifeline, which is process
// behaviour the harness cannot express, so it builds its own frames.
const HAND_BUILT_FRAME_EXEMPTIONS = [
  "packages/conversations-package/backend/tests/process_lifeline.rs",
];

test("Backends use the typed router and tests use the harness", async () => {
  for (const directory of BACKEND_DIRECTORIES) {
    const sources = await rustFiles(`${directory}/src`);
    assert.ok(sources.length > 0, `${directory}/src was not read`);
    for (const { path, source } of sources) {
      assert.doesNotMatch(source, /fn object</, `${path} must not keep a private JSON helper`);
      assert.doesNotMatch(source, /fn empty_object/, `${path} must not keep a private JSON helper`);
      assert.ok(!source.includes("impl Backend for"), `${path} must route through Router`);
      assert.ok(
        !source.includes(".data_directory().get("),
        `${path} must read typed initialization values`,
      );
    }

    const tests = await rustFiles(`${directory}/tests`);
    assert.ok(tests.length > 0, `${directory}/tests was not read`);
    for (const { path, source } of tests) {
      const normalized = path.replaceAll("//", "/");
      if (HAND_BUILT_FRAME_EXEMPTIONS.some((exempt) => normalized.endsWith(exempt))) continue;
      assert.ok(
        !source.includes('"kind": "initialize"'),
        `${path} must drive the Backend through the SDK harness, not hand-built frames`,
      );
    }
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

  const probeNavigation = source.indexOf('"UI isolation probe navigation"');
  const probeResult = source.indexOf('"UI isolation probe result"');
  assert.ok(probeNavigation > backendWait, "the isolation probe launch must follow the bundled Backend inspection");
  assert.ok(probeResult > probeNavigation, "the isolation probe must navigate before reading its verdict");
  assert.match(source, /prepareUiIsolationProbe/);
  assert.match(source, /ui-isolation-breach/);
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
    "dev.teamarness.documents",
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

test("Package UIs run only in sandboxed frames and the Shell realm loads no Package code", async () => {
  const mount = await read("apps/teamarness-desktop/src/package-mount.ts");
  assert.match(mount, /"allow-scripts"/);

  const desktopSources = await sourceFiles("apps/teamarness-desktop/src");
  assert.ok(desktopSources.some(({ path }) => path.endsWith("/package-mount.ts")), "desktop sources were not read");
  assert.ok(desktopSources.some(({ path }) => path.includes("/package-frame/")), "frame sources were not read");
  assert.ok(desktopSources.length >= 10, "desktop source scan found too few files");
  for (const { path, source } of desktopSources) {
    for (const token of ["allow-same-origin", "allow-top-navigation", "allow-popups", "allow-forms"]) {
      assert.ok(!source.includes(token), `${path} must not widen the Package UI Frame sandbox with ${token}`);
    }
  }
  const shellRealm = desktopSources.filter(({ path }) =>
    !path.includes("/package-frame/") && !path.endsWith(".test.ts"));
  for (const { path, source } of shellRealm) {
    assert.ok(!source.includes("import("), `${path} must not load modules into the Shell realm`);
    for (const listener of ['addEventListener("error"', 'addEventListener("unhandledrejection"', 'addEventListener("message"']) {
      assert.ok(!source.includes(listener), `${path} must not install the window listener ${listener}`);
    }
  }

  const csp = JSON.parse(await read("apps/teamarness-desktop/src-tauri/tauri.conf.json")).app.security.csp;
  const scriptDirective = csp.split(";").map((directive) => directive.trim()).find((directive) => directive.startsWith("script-src"));
  assert.equal(scriptDirective, "script-src 'self'");
  assert.ok(csp.includes("frame-src teamarness-package:"));

  const sdkManifest = JSON.parse(await read("packages/teamarness-package-ui-sdk/package.json"));
  assert.equal(sdkManifest.dependencies?.["@tauri-apps/api"], undefined);
  const authorTrees = [
    "packages/teamarness-package-ui-sdk/src",
    "packages/reference-package/ui/src",
    "packages/tutorial/ui/src",
    "packages/conversations-package/ui/src",
    "packages/repo-aware/ui/src",
    "templates/package-foundation/v1/ui/src",
    "apps/teamarness-desktop/src/package-frame",
  ];
  for (const tree of authorTrees) {
    const sources = await sourceFiles(tree);
    assert.ok(sources.length > 0, `${tree} was not read`);
    for (const { path, source } of sources) {
      assert.ok(!source.includes("@tauri-apps"), `${path} must not reach Tauri`);
    }
  }
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
    ["README.md", "immutable-record-invalid.golden.json", "immutable-record-log-1.1.schema.json", "immutable-record-log.schema.json", "immutable-record-log.test.mjs", "immutable-record-replay.golden.json", "immutable-record.golden.json", "membership.schema.json", "person-names.schema.json", "signing-envelope.golden.json", "signing.schema.json", "workspace-context.schema.json"],
  );

  const productionPaths = [
    "apps/teamarness-desktop/src/package-bridge.ts",
    "apps/teamarness-desktop/src/package-contracts.ts",
    "apps/teamarness-desktop/src/package-mount.ts",
    "apps/teamarness-desktop/src/package-port-host.ts",
    "apps/teamarness-desktop/src/package-frame/protocol.ts",
    "apps/teamarness-desktop/src/package-frame/runtime.ts",
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
