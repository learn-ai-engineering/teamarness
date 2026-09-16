import assert from "node:assert/strict";
import { mkdtemp, readFile, readdir, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  createPackageProject,
  PACKAGE_IDENTITY_PATTERN,
  parseArgs,
  repositoryRoot,
  validateArguments,
} from "./create-package.mjs";

async function temporaryDirectory() {
  return mkdtemp(join(tmpdir(), "teamarness-create-package-test-"));
}

async function relativeFiles(root, prefix = "") {
  const files = [];
  for (const entry of await readdir(root, { withFileTypes: true })) {
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) files.push(...await relativeFiles(join(root, entry.name), relative));
    else if (entry.isFile()) files.push(relative);
  }
  return files.sort();
}

function dependencyDeclaration(manifest, name) {
  const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = manifest.match(new RegExp(`^${escaped}\\s*=.*$`, "m"));
  assert.ok(match, `missing dependency declaration for ${name}`);
  return match[0];
}

function productionDependencyNames(manifest) {
  const section = manifest.match(/\[dependencies\]\n([\s\S]*?)(?:\n\[|$)/);
  assert.ok(section, "missing dependencies section");
  return section[1]
    .split("\n")
    .map((line) => line.match(/^([a-z0-9-]+)(?:\.workspace)?\s*=/)?.[1])
    .filter(Boolean)
    .sort();
}

test("parseArgs accepts only the stable generator interface", () => {
  assert.deepEqual(parseArgs(["--output", "/tmp/out", "--identity", "dev.example.demo", "--display-name", "Demo"]), {
    output: "/tmp/out",
    identity: "dev.example.demo",
    displayName: "Demo",
  });
  assert.throws(() => parseArgs(["--output", "/tmp/out", "--identity", "dev.example.demo", "--display-name", "Demo", "--extra", "x"]), /Unknown argument/);
  assert.throws(() => parseArgs(["--output", "/tmp/out", "--identity", "dev.example.demo"]), /Missing required argument/);
});

test("generator and manifest schema use the Host Package Identity grammar", async () => {
  const manifestSchema = JSON.parse(await readFile(join(repositoryRoot, "contracts/packages/v1/schemas/manifest.schema.json"), "utf8"));
  const schemaPattern = new RegExp(manifestSchema.$defs.packageIdentity.pattern);
  for (const identity of ["dev.example", "a.b", `a.${"b".repeat(63)}`]) {
    assert.equal(PACKAGE_IDENTITY_PATTERN.test(identity), true, identity);
    assert.equal(schemaPattern.test(identity), true, identity);
  }
  for (const identity of ["dev.example-", "dev.-example", "dev.Example", "single"]) {
    assert.equal(PACKAGE_IDENTITY_PATTERN.test(identity), false, identity);
    assert.equal(schemaPattern.test(identity), false, identity);
  }
});

test("validates identity and display name before writing", () => {
  assert.throws(() => validateArguments({ output: "/tmp/out", identity: "Demo", displayName: "Demo" }), /identity/);
  assert.throws(() => validateArguments({ output: "/tmp/out", identity: "dev.example-", displayName: "Demo" }), /identity/);
  const overlongIdentity = ["a".repeat(63), "b".repeat(63), "c".repeat(63)].join(".");
  assert.equal(overlongIdentity.length, 191);
  assert.throws(() => validateArguments({ output: "/tmp/out", identity: overlongIdentity, displayName: "Demo" }), /identity/);
  for (const displayName of [
    "",
    " Demo",
    "Demo ",
    "Demo\nPackage",
    "é".repeat(65),
    "Demo\ud800",
  ]) {
    assert.throws(
      () => validateArguments({ output: "/tmp/out", identity: "dev.example.demo", displayName }),
      /display-name/,
      JSON.stringify(displayName),
    );
  }
});

test("creates an absent and an empty destination, but never overwrites files", async () => {
  const parent = await temporaryDirectory();
  try {
    const absent = join(parent, "absent");
    await createPackageProject({ output: absent, identity: "dev.teamarness.emojart", displayName: "Emojart" });
    assert.equal(JSON.parse(await readFile(join(absent, "teamarness.package.json"))).identity, "dev.teamarness.emojart");

    const empty = join(parent, "empty");
    await import("node:fs/promises").then(({ mkdir }) => mkdir(empty));
    await createPackageProject({ output: empty, identity: "org.example.local", displayName: "Local" });
    assert.equal(JSON.parse(await readFile(join(empty, "teamarness.package.json"))).identity, "org.example.local");

    const nonEmpty = join(parent, "non-empty");
    await import("node:fs/promises").then(({ mkdir }) => mkdir(nonEmpty));
    await writeFile(join(nonEmpty, "keep.txt"), "keep");
    await assert.rejects(createPackageProject({ output: nonEmpty, identity: "org.example.local", displayName: "Local" }), /empty directory/);
    assert.equal(await readFile(join(nonEmpty, "keep.txt"), "utf8"), "keep");
  } finally {
    await rm(parent, { recursive: true, force: true });
  }
});

test("refuses a symlink destination", async () => {
  const parent = await temporaryDirectory();
  try {
    const target = join(parent, "target");
    const link = join(parent, "link");
    await import("node:fs/promises").then(({ mkdir }) => mkdir(target));
    await symlink(target, link);
    await assert.rejects(createPackageProject({ output: link, identity: "org.example.local", displayName: "Local" }), /symlink/);
    assert.deepEqual(await readdir(target), []);
  } finally {
    await rm(parent, { recursive: true, force: true });
  }
});

test("copies canonical SDK production trees and records exact provenance", async () => {
  const parent = await temporaryDirectory();
  try {
    const output = join(parent, "package");
    await createPackageProject({ output, identity: "dev.teamarness.emojart", displayName: "Emojart" });
    const provenance = JSON.parse(await readFile(join(output, "teamarness-authoring.json"), "utf8"));
    assert.deepEqual(provenance, {
      templateVersion: 1,
      packageProcessProtocol: "1.0",
      uiHostVersion: 1,
      sdkSource: "vendored-from-teamarness-checkout",
      sdkUpgrade: "unsupported-in-place",
      requestNamespace: "emojart",
    });
    const manifest = JSON.parse(await readFile(join(output, "teamarness.package.json"), "utf8"));
    assert.equal(manifest.presentation.displayName, "Emojart");
    assert.equal(manifest.capabilities.length, 0);
    const agentGuide = await readFile(join(output, "AGENTS.md"), "utf8");
    for (const phrase of [
      "Emojart",
      "dev.teamarness.emojart",
      "emojart.read",
      "Teamarness Core Runtime",
      "Package Host",
      "PackageClient",
      "Package Data Directory",
      "Semantic Capabilities",
      "metadata changes while the meaningful output stays fixed",
      "not a sandbox",
      "npm run build",
    ]) assert.ok(agentGuide.includes(phrase), `AGENTS.md must include ${phrase}`);
    assert.equal(agentGuide.includes("__PACKAGE_"), false);
    assert.equal(agentGuide.includes("__REQUEST_NAMESPACE__"), false);
    const generatedCargo = await readFile(join(output, "Cargo.toml"), "utf8");
    assert.match(generatedCargo, /edition = "2024"/);
    assert.match(await readFile(join(output, "package-lock.json"), "utf8"), /"lockfileVersion": 3/);

    for (const [canonical, generated] of [
      ["crates/teamarness-package-protocol/src", "vendor/teamarness/package-protocol/src"],
      ["crates/teamarness-package-sdk/src", "vendor/teamarness/package-sdk/src"],
      ["packages/teamarness-package-ui-sdk/src", "vendor/teamarness/package-ui-sdk/src"],
    ]) {
      const canonicalRoot = join(repositoryRoot, canonical);
      const generatedRoot = join(output, generated);
      const canonicalFiles = await relativeFiles(canonicalRoot);
      assert.deepEqual(await relativeFiles(generatedRoot), canonicalFiles, `${generated} file set drifted`);
      for (const relative of canonicalFiles) {
        assert.deepEqual(
          await readFile(join(generatedRoot, relative)),
          await readFile(join(canonicalRoot, relative)),
          `${generated}/${relative} drifted`,
        );
      }
    }

    const canonicalCargo = await readFile(join(repositoryRoot, "Cargo.toml"), "utf8");
    for (const dependency of ["base64", "serde", "serde_json", "tokio"]) {
      assert.equal(
        dependencyDeclaration(generatedCargo, dependency),
        dependencyDeclaration(canonicalCargo, dependency),
      );
    }

    const canonicalProtocol = await readFile(join(repositoryRoot, "crates/teamarness-package-protocol/Cargo.toml"), "utf8");
    const generatedProtocol = await readFile(join(output, "vendor/teamarness/package-protocol/Cargo.toml"), "utf8");
    assert.deepEqual(productionDependencyNames(generatedProtocol), productionDependencyNames(canonicalProtocol));
    const canonicalBackendSdk = await readFile(join(repositoryRoot, "crates/teamarness-package-sdk/Cargo.toml"), "utf8");
    const generatedBackendSdk = await readFile(join(output, "vendor/teamarness/package-sdk/Cargo.toml"), "utf8");
    assert.deepEqual(productionDependencyNames(generatedBackendSdk), productionDependencyNames(canonicalBackendSdk));
    assert.match(generatedBackendSdk, /teamarness-package-protocol = \{ path = "\.\.\/package-protocol" \}/);

    const canonicalUiSdk = JSON.parse(await readFile(join(repositoryRoot, "packages/teamarness-package-ui-sdk/package.json"), "utf8"));
    const generatedUiSdk = JSON.parse(await readFile(join(output, "vendor/teamarness/package-ui-sdk/package.json"), "utf8"));
    assert.deepEqual(generatedUiSdk.dependencies, canonicalUiSdk.dependencies);
    const canonicalUiGuide = await readFile(join(repositoryRoot, "packages/teamarness-package-ui-sdk/README.md"), "utf8");
    const generatedUiGuide = await readFile(join(output, "vendor/teamarness/package-ui-sdk/README.md"), "utf8");
    assert.match(generatedUiGuide, /generated from the Teamarness checkout/);
    assert.match(generatedUiGuide, /vendored so the project remains buildable/);
    assert.ok(
      generatedUiGuide.endsWith(canonicalUiGuide.slice(canonicalUiGuide.indexOf("\n") + 1).trimStart()),
      "generated UI SDK guidance must embed the canonical guide without a second maintained copy",
    );

    const generatedPackage = JSON.parse(await readFile(join(output, "package.json"), "utf8"));
    assert.match(generatedPackage.scripts.test, /node --test scripts\/assemble-package\.test\.mjs/);
    for (const testPath of [
      "backend/tests/backend.rs",
      "ui/src/index.test.ts",
      "scripts/assemble-package.test.mjs",
    ]) {
      const testSource = await readFile(join(output, testPath), "utf8");
      assert.equal(testSource.includes("expect(true).toBe(true)"), false, `${testPath} must test behavior`);
      assert.equal(testSource.includes("__PACKAGE_"), false, `${testPath} contains an unsubstituted Package token`);
      assert.equal(testSource.includes("__REQUEST_NAMESPACE__"), false, `${testPath} contains an unsubstituted namespace token`);
    }

    const allFiles = [];
    async function collect(path) {
      for (const entry of await import("node:fs/promises").then(({ readdir }) => readdir(path, { withFileTypes: true }))) {
        const child = join(path, entry.name);
        if (entry.isDirectory() && entry.name !== "node_modules" && entry.name !== "target") await collect(child);
        else if (entry.isFile()) allFiles.push(child);
      }
    }
    await collect(output);
    for (const file of allFiles) {
      const text = await readFile(file, "utf8").catch(() => "");
      assert.equal(text.includes(repositoryRoot), false, `absolute checkout path leaked into ${file}`);
    }
  } finally {
    await rm(parent, { recursive: true, force: true });
  }
});
