import assert from "node:assert/strict";
import { copyFile, mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { BUNDLED_PACKAGES_PATH, readBundledPackages, repositoryRoot } from "./bundled-packages.mjs";
import { buildPackage, cargoTargetDirectory, resolveBundledPackage } from "./build-package.mjs";

const bundledPackages = await readBundledPackages(repositoryRoot);

// Copies the real manifest and UI workspace name, then fakes every build input the manifest declares.
async function packageFixture(root, { sourceDirectory }) {
  const source = join(root, sourceDirectory);
  await mkdir(join(source, "ui/dist"), { recursive: true });
  await copyFile(join(repositoryRoot, sourceDirectory, "teamarness.package.json"), join(source, "teamarness.package.json"));
  await copyFile(join(repositoryRoot, sourceDirectory, "ui/package.json"), join(source, "ui/package.json"));
  await writeFile(join(source, "ui-contract.json"), '{"schemaVersion":1,"$defs":{},"requests":{},"subscriptions":{}}');
  await writeFile(join(source, "ui/dist/index.js"), "export const mount = () => () => {};\n");
  const manifest = JSON.parse(await readFile(join(source, "teamarness.package.json"), "utf8"));
  for (const { path } of manifest.ui.assets) {
    await writeFile(join(source, "ui/dist", path.slice("ui/".length)), `asset:${path}\n`);
  }
  const backend = join(root, `${manifest.identity}-backend`);
  await writeFile(backend, "backend bytes");
  return { manifest, backend };
}

async function withRoot(prefix, callback) {
  const root = await mkdtemp(join(tmpdir(), prefix));
  try {
    await mkdir(join(root, "packages"), { recursive: true });
    await writeFile(join(root, "packages/application-packages.v1.json"), '{"untouched":true}\n');
    await copyFile(join(repositoryRoot, BUNDLED_PACKAGES_PATH), join(root, BUNDLED_PACKAGES_PATH));
    await callback(root);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}

for (const entry of bundledPackages) {
  test(`builder stages the ${entry.identity} Package Directory without editing application metadata`, async () => {
    await withRoot("teamarness-package-build-", async (root) => {
      const { manifest: source, backend } = await packageFixture(root, entry);
      const output = await buildPackage({
        repositoryRoot: root,
        sourceDirectory: entry.sourceDirectory,
        cargoPackage: entry.cargoPackage,
        expectedIdentity: entry.identity,
        backendPath: backend,
        skipBuilds: true,
        skipUiBuild: true,
      });

      assert.equal(output, join(root, "target/teamarness-packages", entry.identity));
      assert.equal(await readFile(join(root, "packages/application-packages.v1.json"), "utf8"), '{"untouched":true}\n');
      const manifest = JSON.parse(await readFile(join(output, "teamarness.package.json"), "utf8"));
      assert.equal(manifest.identity, entry.identity);
      assert.equal(manifest.backends[0].size, "backend bytes".length);
      assert.match(manifest.backends[0].sha256, /^[0-9a-f]{64}$/);
      assert.equal(manifest.ui.entrypoint.size, "export const mount = () => () => {};\n".length);
      assert.match(manifest.ui.contract.sha256, /^[0-9a-f]{64}$/);
      assert.deepEqual(manifest.ui.assets.map(({ path, size }) => ({ path, size })),
        source.ui.assets.map(({ path }) => ({ path, size: `asset:${path}\n`.length })));
      assert.deepEqual(manifest.extensions, source.extensions);
      assert.equal((await stat(join(output, manifest.backends[0].path))).mode & 0o777, 0o700);
      assert.equal((await stat(join(output, manifest.ui.entrypoint.path))).mode & 0o777, 0o600);
    });
  });
}

test("builder applies an explicit Package version", async () => {
  await withRoot("teamarness-package-build-version-", async (root) => {
    const [entry] = bundledPackages;
    const { backend } = await packageFixture(root, entry);
    const output = await buildPackage({
      repositoryRoot: root,
      sourceDirectory: entry.sourceDirectory,
      backendPath: backend,
      skipBuilds: true,
      skipUiBuild: true,
      version: "9.8.7",
    });
    assert.equal(JSON.parse(await readFile(join(output, "teamarness.package.json"), "utf8")).version, "9.8.7");
  });
});

test("builder rejects a manifest with an unexpected Package Identity", async () => {
  await withRoot("teamarness-package-build-identity-", async (root) => {
    const [entry] = bundledPackages;
    const { backend } = await packageFixture(root, entry);
    await assert.rejects(
      buildPackage({
        repositoryRoot: root,
        sourceDirectory: entry.sourceDirectory,
        expectedIdentity: "dev.teamarness.not-this-package",
        backendPath: backend,
        outputDirectory: join(root, "out"),
        skipBuilds: true,
        skipUiBuild: true,
      }),
      /Unexpected Package Identity/,
    );
  });
});

test("builder rejects a declared UI asset without a build output", async () => {
  const entry = bundledPackages.find(({ manifest }) => manifest.ui.assets.length > 0);
  assert.ok(entry, "at least one bundled Package declares a UI asset");
  await withRoot("teamarness-package-build-asset-", async (root) => {
    const { manifest, backend } = await packageFixture(root, entry);
    await rm(join(root, entry.sourceDirectory, "ui/dist", manifest.ui.assets[0].path.slice("ui/".length)));
    await assert.rejects(
      buildPackage({
        repositoryRoot: root,
        sourceDirectory: entry.sourceDirectory,
        backendPath: backend,
        outputDirectory: join(root, "out"),
        skipBuilds: true,
        skipUiBuild: true,
      }),
      new RegExp(`UI asset build input is missing: ${manifest.ui.assets[0].path}`),
    );
  });
});

test("command line resolves bundled Packages by identity or source directory", async () => {
  for (const entry of bundledPackages) {
    assert.equal((await resolveBundledPackage(entry.identity)).sourceDirectory, entry.sourceDirectory);
    assert.equal((await resolveBundledPackage(`${entry.sourceDirectory}/`)).identity, entry.identity);
  }
  await assert.rejects(resolveBundledPackage("dev.teamarness.unknown"), /Unknown bundled Package/);
});

test("Backend lookup follows a custom CARGO_TARGET_DIR", async () => {
  const targetDirectory = await mkdtemp(join(tmpdir(), "teamarness-cargo-target-"));
  try {
    assert.equal(cargoTargetDirectory(repositoryRoot, { ...process.env, CARGO_TARGET_DIR: targetDirectory }), targetDirectory);
    const { CARGO_TARGET_DIR: _ignored, ...defaultEnv } = process.env;
    assert.equal(cargoTargetDirectory(repositoryRoot, defaultEnv), join(repositoryRoot, "target"));
  } finally {
    await rm(targetDirectory, { recursive: true, force: true });
  }
});
