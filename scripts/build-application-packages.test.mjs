import assert from "node:assert/strict";
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { buildApplicationPackages } from "./build-application-packages.mjs";
import { BUNDLED_PACKAGES_PATH, readBundledPackages, repositoryRoot } from "./bundled-packages.mjs";

async function packageFixture(root, { sourceDirectory, manifest }) {
  const source = join(root, sourceDirectory);
  await mkdir(join(source, "ui/dist"), { recursive: true });
  await copyFile(join(repositoryRoot, sourceDirectory, "teamarness.package.json"), join(source, "teamarness.package.json"));
  await writeFile(join(source, "ui-contract.json"), '{"schemaVersion":1,"requests":{},"subscriptions":{}}');
  if (manifest.portContract) {
    await copyFile(join(repositoryRoot, sourceDirectory, manifest.portContract.path), join(source, manifest.portContract.path));
  }
  await writeFile(join(source, "ui/dist/index.js"), "export const mount = () => () => {};\n");
  for (const { path } of manifest.ui.assets) {
    await writeFile(join(source, "ui/dist", path.slice("ui/".length)), `asset:${manifest.identity}\n`);
  }
  const backend = join(root, `${manifest.identity}-backend`);
  await writeFile(backend, `${manifest.identity} backend`);
  return backend;
}

test("application builder stages every bundled Package and writes one complete index", async () => {
  const root = await mkdtemp(join(tmpdir(), "teamarness-application-packages-"));
  try {
    const bundled = await readBundledPackages(repositoryRoot);
    await mkdir(join(root, "packages"), { recursive: true });
    await copyFile(join(repositoryRoot, BUNDLED_PACKAGES_PATH), join(root, BUNDLED_PACKAGES_PATH));
    const backendPaths = {};
    for (const entry of bundled) backendPaths[entry.identity] = await packageFixture(root, entry);

    const result = await buildApplicationPackages({ repositoryRoot: root, target: "aarch64-apple-darwin", skipBuilds: true, skipUiBuild: true, backendPaths });

    assert.equal(
      await readFile(result.indexPath, "utf8"),
      await readFile(join(repositoryRoot, "packages/application-packages.v1.json"), "utf8"),
    );
    assert.deepEqual(result.directories.map((directory) => directory.split("/").at(-1)), bundled.map(({ identity }) => identity));
    for (const [index, { identity }] of bundled.entries()) {
      assert.equal(JSON.parse(await readFile(join(result.directories[index], "teamarness.package.json"))).identity, identity);
      assert.equal(JSON.parse(await readFile(join(root, "target/debug/packages", identity, "teamarness.package.json"))).identity, identity);
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("explicit resource roots isolate target output and Linux release never codesigns", async () => {
  const root = await mkdtemp(join(tmpdir(), "teamarness-application-native-"));
  try {
    const bundled = await readBundledPackages(repositoryRoot);
    await mkdir(join(root, "packages"), { recursive: true });
    await copyFile(join(repositoryRoot, BUNDLED_PACKAGES_PATH), join(root, BUNDLED_PACKAGES_PATH));
    await writeFile(join(root, "packages/application-packages.v1.json"), "source index preserved");
    const backendPaths = {};
    for (const entry of bundled) backendPaths[entry.identity] = await packageFixture(root, entry);
    const desktop = await buildApplicationPackages({ repositoryRoot: root, target: "aarch64-apple-darwin", skipBuilds: true, skipUiBuild: true, backendPaths });
    const original = await readFile(join(desktop.directories[0], "teamarness.package.json"));
    const outputRoot = join(root, "headless/linux/packages");
    const result = await buildApplicationPackages({ repositoryRoot: root, outputRoot, target: "x86_64-unknown-linux-gnu",
      skipBuilds: true, skipUiBuild: true, backendPaths, release: true });
    assert.equal(result.indexPath, join(outputRoot, "application-packages.v1.json"));
    assert.equal(result.directories[0], join(outputRoot, bundled[0].identity));
    assert.equal(JSON.parse(await readFile(join(result.directories[0], "teamarness.package.json"))).backends[0].target, "x86_64-unknown-linux-gnu");
    assert.deepEqual(await readFile(join(desktop.directories[0], "teamarness.package.json")), original);
    assert.deepEqual(await readFile(join(root, "target/debug/packages", bundled[0].identity, "teamarness.package.json")), original);
    await assert.rejects(buildApplicationPackages({ repositoryRoot: root, outputRoot, target: "unsupported", skipBuilds: true }), /Unsupported Backend target/);
    assert.equal(JSON.parse(await readFile(result.indexPath)).packages.length, bundled.length);
  } finally { await rm(root, { recursive: true, force: true }); }
});
