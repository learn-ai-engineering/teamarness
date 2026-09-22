import assert from "node:assert/strict";
import { cp, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";

import {
  APPLICATION_INDEX_PATH,
  BUNDLED_PACKAGES_PATH,
  TAURI_CONFIG_PATH,
  checkBundledPackages,
  readBundledPackages,
  repositoryRoot,
  writeBundledPackages,
} from "./bundled-packages.mjs";

async function copyInputs(root) {
  const bundled = await readBundledPackages(repositoryRoot);
  for (const path of [BUNDLED_PACKAGES_PATH, APPLICATION_INDEX_PATH, TAURI_CONFIG_PATH]) {
    await mkdir(dirname(join(root, path)), { recursive: true });
    await cp(join(repositoryRoot, path), join(root, path));
  }
  for (const { sourceDirectory } of bundled) {
    await mkdir(join(root, sourceDirectory), { recursive: true });
    await cp(join(repositoryRoot, sourceDirectory, "teamarness.package.json"), join(root, sourceDirectory, "teamarness.package.json"));
  }
  return bundled;
}

test("repository Tauri configuration and application index match the bundled Package list", async () => {
  assert.deepEqual(await checkBundledPackages(repositoryRoot), []);
});

test("check reports divergence and write regenerates only the derived blocks", async () => {
  const root = await mkdtemp(join(tmpdir(), "teamarness-bundled-packages-"));
  try {
    const bundled = await copyInputs(root);
    const list = JSON.parse(await readFile(join(root, BUNDLED_PACKAGES_PATH), "utf8"));
    list.packages.pop();
    await writeFile(join(root, BUNDLED_PACKAGES_PATH), JSON.stringify(list));

    const problems = await checkBundledPackages(root);
    assert.equal(problems.length, 3);
    assert.match(problems.join("\n"), /bundle\.resources/);
    assert.match(problems.join("\n"), /bundle\.externalBin/);
    assert.match(problems.join("\n"), /application-packages\.v1\.json/);

    const before = await readFile(join(root, TAURI_CONFIG_PATH), "utf8");
    assert.deepEqual(await writeBundledPackages(root), []);
    assert.deepEqual(await checkBundledPackages(root), [`${APPLICATION_INDEX_PATH} does not match ${BUNDLED_PACKAGES_PATH}`]);
    const after = await readFile(join(root, TAURI_CONFIG_PATH), "utf8");
    const removed = bundled.at(-1).identity;
    assert.ok(before.includes(removed));
    assert.ok(!after.includes(removed));
    assert.equal(after.includes('"targets": ["app", "dmg"]'), before.includes('"targets": ["app", "dmg"]'));
    await writeBundledPackages(root);
    assert.equal(await readFile(join(root, TAURI_CONFIG_PATH), "utf8"), after);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("list entries must name a Backend that matches the manifest", async () => {
  const root = await mkdtemp(join(tmpdir(), "teamarness-bundled-packages-backend-"));
  try {
    await copyInputs(root);
    const list = JSON.parse(await readFile(join(root, BUNDLED_PACKAGES_PATH), "utf8"));
    list.packages[0].cargoPackage = "teamarness-wrong-package";
    await writeFile(join(root, BUNDLED_PACKAGES_PATH), JSON.stringify(list));
    await assert.rejects(checkBundledPackages(root), /must declare one Backend at backend\/teamarness-wrong-package-<target>/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
