import assert from "node:assert/strict";
import { chmod, mkdir, mkdtemp, readFile, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import {
  compareDeclaredPackageBytes,
  copyPackageDirectory,
  stagePackageDirectory,
} from "./package-directory.mjs";

test("staging records every declared byte and copies a byte-identical local Package", async () => {
  const root = await mkdtemp(join(tmpdir(), "teamarness-package-stage-"));
  const source = join(root, "source");
  const output = join(root, "bundled");
  const local = join(root, "local");
  await mkdir(join(source, "ui"), { recursive: true });
  const backend = Buffer.from("#!/bin/sh\nexit 0\n");
  const entrypoint = Buffer.from("export const mount = () => () => {};\n");
  const contract = Buffer.from('{"schemaVersion":1,"$defs":{},"requests":{},"subscriptions":{}}');
  await writeFile(join(source, "backend"), backend);
  await chmod(join(source, "backend"), 0o700);
  await writeFile(join(source, "ui/index.js"), entrypoint);
  await writeFile(join(source, "contract.json"), contract);
  await writeFile(join(source, "manifest.json"), JSON.stringify({
    schemaVersion: 1,
    identity: "dev.teamarness.example",
    version: "1.0.0",
    presentation: { displayName: "Example Package" },
    protocol: { major: 1, minMinor: 0, maxMinor: 0 },
    backends: [{ target: "aarch64-apple-darwin", path: "backend/example-aarch64-apple-darwin", size: 0, sha256: "0".repeat(64) }],
    ui: { entrypoint: { path: "ui/index.js", size: 0, sha256: "0".repeat(64) }, assets: [], contract: { path: "ui/contract.json", sha256: "0".repeat(64) } },
    capabilities: [],
  }));

  await stagePackageDirectory({
    manifestPath: join(source, "manifest.json"),
    contractPath: join(source, "contract.json"),
    backendPath: join(source, "backend"),
    uiEntrypointPath: join(source, "ui/index.js"),
    outputDirectory: output,
  });
  await copyPackageDirectory(output, local);
  const comparison = await compareDeclaredPackageBytes(output, local);
  assert.equal(comparison.files.length, 3);
  assert.equal(comparison.totalBytes, backend.length + entrypoint.length + contract.length);

  const manifest = JSON.parse(await readFile(join(output, "teamarness.package.json"), "utf8"));
  assert.equal(manifest.backends[0].size, backend.length);
  assert.equal(manifest.ui.entrypoint.size, entrypoint.length);
  assert.equal((await stat(join(output, manifest.backends[0].path))).mode & 0o777, 0o700);
  assert.equal((await stat(join(output, manifest.ui.entrypoint.path))).mode & 0o777, 0o600);
  assert.equal((await stat(join(output, manifest.ui.contract.path))).mode & 0o777, 0o600);
  await writeFile(join(local, "ui/index.js"), "changed");
  await assert.rejects(compareDeclaredPackageBytes(output, local), /byte mismatch/);
});
