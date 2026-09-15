import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { buildEmojartPackage } from "./build-emojart-package.mjs";

const digestPlaceholder = "0".repeat(64);

test("build stages only the installation-local Emojart Package Directory", async () => {
  const root = await mkdtemp(join(tmpdir(), "teamarness-emojart-build-"));
  await mkdir(join(root, "packages/emojart/ui/dist"), { recursive: true });
  await mkdir(join(root, "packages/emojart/backend"), { recursive: true });
  await writeFile(join(root, "packages/application-packages.v1.json"), '{"untouched":true}\n');
  await writeFile(join(root, "packages/emojart/ui-contract.json"), '{"schemaVersion":1,"$defs":{},"requests":{},"subscriptions":{}}');
  await writeFile(join(root, "packages/emojart/ui/dist/index.js"), "export const mount = () => () => {};\n");
  await writeFile(join(root, "packages/emojart/backend/fake"), "backend bytes");
  await writeFile(join(root, "packages/emojart/teamarness.package.json"), JSON.stringify({
    schemaVersion: 1,
    identity: "dev.teamarness.emojart",
    version: "1.0.0",
    presentation: { displayName: "Emojart" },
    protocol: { major: 1, minMinor: 0, maxMinor: 0 },
    backends: [{ target: "aarch64-apple-darwin", path: "backend/teamarness-emojart-package-aarch64-apple-darwin", size: 0, sha256: digestPlaceholder }],
    ui: { entrypoint: { path: "ui/index.js", size: 0, sha256: digestPlaceholder }, assets: [], contract: { path: "ui/contract.json", sha256: digestPlaceholder } },
    capabilities: [],
  }));

  const output = await buildEmojartPackage({
    repositoryRoot: root,
    backendPath: join(root, "packages/emojart/backend/fake"),
    skipBuilds: true,
  });

  assert.equal(output, join(root, "target/teamarness-packages-local/dev.teamarness.emojart"));
  assert.equal(await readFile(join(root, "packages/application-packages.v1.json"), "utf8"), '{"untouched":true}\n');
  const manifest = JSON.parse(await readFile(join(output, "teamarness.package.json"), "utf8"));
  assert.equal(manifest.identity, "dev.teamarness.emojart");
  assert.ok(manifest.backends[0].size > 0);
  assert.match(manifest.backends[0].sha256, /^[0-9a-f]{64}$/);
  assert.ok(manifest.ui.entrypoint.size > 0);
  assert.match(manifest.ui.entrypoint.sha256, /^[0-9a-f]{64}$/);
  assert.match(manifest.ui.contract.sha256, /^[0-9a-f]{64}$/);
  assert.equal((await stat(join(output, manifest.backends[0].path))).mode & 0o777, 0o700);
  assert.equal((await stat(join(output, manifest.ui.entrypoint.path))).mode & 0o777, 0o600);
});
