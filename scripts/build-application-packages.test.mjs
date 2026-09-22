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

    const result = await buildApplicationPackages({ repositoryRoot: root, skipBuilds: true, skipUiBuild: true, backendPaths });

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
