import assert from "node:assert/strict";
import { mkdtemp, readFile, readdir, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  createPackageProject,
  parseArgs,
  repositoryRoot,
  validateArguments,
} from "./create-package.mjs";

async function temporaryDirectory() {
  return mkdtemp(join(tmpdir(), "teamarness-create-package-test-"));
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

test("validates identity and display name before writing", () => {
  assert.throws(() => validateArguments({ output: "/tmp/out", identity: "Demo", displayName: "Demo" }), /identity/);
  assert.throws(() => validateArguments({ output: "/tmp/out", identity: "dev.example.demo", displayName: "" }), /display-name/);
  assert.throws(() => validateArguments({ output: "/tmp/out", identity: "dev.example.demo", displayName: "x".repeat(129) }), /display-name/);
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
    assert.match(await readFile(join(output, "Cargo.toml"), "utf8"), /edition = "2024"/);
    assert.match(await readFile(join(output, "package-lock.json"), "utf8"), /"lockfileVersion": 3/);
    for (const source of [
      "crates/teamarness-package-protocol/src/lib.rs",
      "crates/teamarness-package-sdk/src/lib.rs",
      "packages/teamarness-package-ui-sdk/src/index.ts",
    ]) {
      assert.equal((await readFile(join(repositoryRoot, source))).toString(), (await readFile(join(output, "vendor/teamarness", source.replace(/^crates\/teamarness-package-protocol\//, "package-protocol/").replace(/^crates\/teamarness-package-sdk\//, "package-sdk/").replace(/^packages\/teamarness-package-ui-sdk\//, "package-ui-sdk/")))).toString());
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
