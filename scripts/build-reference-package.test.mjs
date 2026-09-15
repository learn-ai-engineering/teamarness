import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { buildReferencePackage } from "./build-reference-package.mjs";

const zeros = "0".repeat(64);

test("Reference builder stages one Package Directory without editing application metadata", async () => {
  const root = await mkdtemp(join(tmpdir(), "teamarness-reference-build-"));
  await mkdir(join(root, "packages/reference-package/ui/dist"), { recursive: true });
  await mkdir(join(root, "packages/reference-package/backend"), { recursive: true });
  await writeFile(join(root, "packages/application-packages.v1.json"), '{"untouched":true}\n');
  await writeFile(join(root, "packages/reference-package/ui-contract.json"), '{"schemaVersion":1,"requests":{},"subscriptions":{}}');
  await writeFile(join(root, "packages/reference-package/ui/dist/index.js"), "export const mount = () => () => {};\n");
  await writeFile(join(root, "packages/reference-package/ui/dist/probe.svg"), "<svg/>\n");
  const backend = join(root, "packages/reference-package/backend/fake");
  await writeFile(backend, "backend bytes");
  await writeFile(join(root, "packages/reference-package/teamarness.package.json"), JSON.stringify({
    schemaVersion: 1,
    identity: "dev.teamarness.reference",
    version: "1.0.0",
    presentation: { displayName: "Reference" },
    protocol: { major: 1, minMinor: 0, maxMinor: 0 },
    backends: [{ target: "aarch64-apple-darwin", path: "backend/teamarness-reference-package-aarch64-apple-darwin", size: 0, sha256: zeros }],
    ui: { entrypoint: { path: "ui/index.js", size: 0, sha256: zeros }, assets: [{ path: "ui/probe.svg", mediaType: "image/svg+xml", size: 0, sha256: zeros }], contract: { path: "ui/contract.json", sha256: zeros } },
    capabilities: [],
  }));
  const output = join(root, "target/teamarness-packages/dev.teamarness.reference");
  await buildReferencePackage({ repositoryRoot: root, backendPath: backend, outputDirectory: output, skipBuilds: true, skipUiBuild: true });
  assert.equal(await readFile(join(root, "packages/application-packages.v1.json"), "utf8"), '{"untouched":true}\n');
  const manifest = JSON.parse(await readFile(join(output, "teamarness.package.json"), "utf8"));
  assert.equal(manifest.identity, "dev.teamarness.reference");
  assert.equal(manifest.ui.assets[0].size, 7);
  assert.equal((await stat(join(output, manifest.backends[0].path))).mode & 0o777, 0o700);
});
