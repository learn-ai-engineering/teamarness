import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { APPLICATION_PACKAGE_DECLARATIONS, buildApplicationPackages } from "./build-application-packages.mjs";

const zeros = "0".repeat(64);

async function packageFixture(root, name, identity, displayName, asset = false) {
  const packageRoot = join(root, `packages/${name}`);
  await mkdir(join(packageRoot, "ui/dist"), { recursive: true });
  await mkdir(join(packageRoot, "backend"), { recursive: true });
  await writeFile(join(packageRoot, "ui-contract.json"), '{"schemaVersion":1,"requests":{},"subscriptions":{}}');
  await writeFile(join(packageRoot, "ui/dist/index.js"), "export const mount = () => () => {};\n");
  if (asset) await writeFile(join(packageRoot, "ui/dist/probe.svg"), "<svg/>\n");
  const backend = join(packageRoot, "backend/fake");
  await writeFile(backend, `${name} backend`);
  await writeFile(join(packageRoot, "teamarness.package.json"), JSON.stringify({
    schemaVersion: 1,
    identity,
    version: "1.0.0",
    presentation: { displayName },
    protocol: { major: 1, minMinor: 0, maxMinor: 0 },
    backends: [{ target: "aarch64-apple-darwin", path: `backend/${name}-aarch64-apple-darwin`, size: 0, sha256: zeros }],
    ui: { entrypoint: { path: "ui/index.js", size: 0, sha256: zeros }, assets: asset ? [{ path: "ui/probe.svg", mediaType: "image/svg+xml", size: 0, sha256: zeros }] : [], contract: { path: "ui/contract.json", sha256: zeros } },
    capabilities: [],
  }));
  return backend;
}

test("application builder stages Reference then Tutorial and writes one complete index", async () => {
  const root = await mkdtemp(join(tmpdir(), "teamarness-application-packages-"));
  const referenceBackend = await packageFixture(root, "reference-package", "dev.teamarness.reference", "Reference", true);
  const tutorialBackend = await packageFixture(root, "tutorial", "dev.teamarness.tutorial", "Tutorial");
  const result = await buildApplicationPackages({
    repositoryRoot: root,
    skipBuilds: true,
    skipUiBuild: true,
    referenceBackendPath: referenceBackend,
    tutorialBackendPath: tutorialBackend,
  });
  const index = JSON.parse(await readFile(result.indexPath, "utf8"));
  assert.deepEqual(index.packages, APPLICATION_PACKAGE_DECLARATIONS.map(({ directory }) => ({ directory })));
  assert.deepEqual(result.directories.map((directory) => directory.split("/").at(-1)), ["dev.teamarness.reference", "dev.teamarness.tutorial"]);
  assert.equal(JSON.parse(await readFile(join(result.directories[0], "teamarness.package.json"))).identity, "dev.teamarness.reference");
  assert.equal(JSON.parse(await readFile(join(result.directories[1], "teamarness.package.json"))).identity, "dev.teamarness.tutorial");
});
