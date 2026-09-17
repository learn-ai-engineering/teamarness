import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { buildRepoAwarePackage } from "./build-repo-aware-package.mjs";

const zeros = "0".repeat(64);

test("Repo-aware builder stages one Package Directory without editing application metadata", async () => {
  const root = await mkdtemp(join(tmpdir(), "teamarness-repo-aware-build-"));
  await mkdir(join(root, "packages/repo-aware/ui/dist"), { recursive: true });
  await mkdir(join(root, "packages/repo-aware/backend"), { recursive: true });
  await writeFile(join(root, "packages/application-packages.v1.json"), '{"untouched":true}\n');
  await writeFile(join(root, "packages/repo-aware/ui-contract.json"), '{"schemaVersion":1,"requests":{},"subscriptions":{}}');
  await writeFile(join(root, "packages/repo-aware/ui/dist/index.js"), "export const mount = () => () => {};\n");
  const backend = join(root, "packages/repo-aware/backend/fake");
  await writeFile(backend, "backend bytes");
  await writeFile(join(root, "packages/repo-aware/teamarness.package.json"), JSON.stringify({
    schemaVersion: 1,
    identity: "dev.teamarness.repo-aware",
    version: "1.0.0",
    presentation: { displayName: "Repo-aware Engineering" },
    protocol: { major: 1, minMinor: 0, maxMinor: 0 },
    backends: [{ target: "aarch64-apple-darwin", path: "backend/teamarness-repo-aware-package-aarch64-apple-darwin", size: 0, sha256: zeros }],
    ui: { entrypoint: { path: "ui/index.js", size: 0, sha256: zeros }, assets: [], contract: { path: "ui/contract.json", sha256: zeros } },
    capabilities: [],
    extensions: { "dev.teamarness.build": { generatedFields: ["backends.0.size", "backends.0.sha256", "ui.entrypoint.size", "ui.entrypoint.sha256", "ui.contract.sha256"] } },
  }));
  const output = join(root, "target/teamarness-packages/dev.teamarness.repo-aware");
  await buildRepoAwarePackage({ repositoryRoot: root, backendPath: backend, outputDirectory: output, skipBuilds: true, skipUiBuild: true });
  assert.equal(await readFile(join(root, "packages/application-packages.v1.json"), "utf8"), '{"untouched":true}\n');
  const manifest = JSON.parse(await readFile(join(output, "teamarness.package.json"), "utf8"));
  assert.equal(manifest.identity, "dev.teamarness.repo-aware");
  assert.equal(manifest.ui.entrypoint.size, "export const mount = () => () => {};\n".length);
  assert.equal((await stat(join(output, manifest.backends[0].path))).mode & 0o777, 0o700);
  assert.deepEqual(manifest.extensions, { "dev.teamarness.build": { generatedFields: ["backends.0.size", "backends.0.sha256", "ui.entrypoint.size", "ui.entrypoint.sha256", "ui.contract.sha256"] } });
});

test("Repo-aware builder rejects a manifest with an unexpected Package Identity", async () => {
  const root = await mkdtemp(join(tmpdir(), "teamarness-repo-aware-build-identity-"));
  await mkdir(join(root, "packages/repo-aware/ui/dist"), { recursive: true });
  await writeFile(join(root, "packages/repo-aware/ui-contract.json"), '{"schemaVersion":1,"requests":{},"subscriptions":{}}');
  await writeFile(join(root, "packages/repo-aware/ui/dist/index.js"), "export const mount = () => () => {};\n");
  const backend = join(root, "fake-backend");
  await writeFile(backend, "backend bytes");
  await writeFile(join(root, "packages/repo-aware/teamarness.package.json"), JSON.stringify({
    schemaVersion: 1,
    identity: "dev.teamarness.not-repo-aware",
    version: "1.0.0",
    presentation: { displayName: "Wrong" },
    protocol: { major: 1, minMinor: 0, maxMinor: 0 },
    backends: [{ target: "aarch64-apple-darwin", path: "backend/teamarness-repo-aware-package-aarch64-apple-darwin", size: 0, sha256: zeros }],
    ui: { entrypoint: { path: "ui/index.js", size: 0, sha256: zeros }, assets: [], contract: { path: "ui/contract.json", sha256: zeros } },
    capabilities: [],
  }));
  await assert.rejects(
    buildRepoAwarePackage({ repositoryRoot: root, backendPath: backend, outputDirectory: join(root, "out"), skipBuilds: true, skipUiBuild: true }),
    /Unexpected Repo-aware Package Identity/,
  );
});
