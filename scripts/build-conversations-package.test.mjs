import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { buildConversationsPackage } from "./build-conversations-package.mjs";

const zeros = "0".repeat(64);

test("Conversations builder stages one Package Directory without editing application metadata", async () => {
  const root = await mkdtemp(join(tmpdir(), "teamarness-conversations-build-"));
  await mkdir(join(root, "packages/conversations-package/ui/dist"), { recursive: true });
  await mkdir(join(root, "packages/conversations-package/backend"), { recursive: true });
  await writeFile(join(root, "packages/application-packages.v1.json"), '{"untouched":true}\n');
  await writeFile(join(root, "packages/conversations-package/ui-contract.json"), '{"schemaVersion":1,"requests":{},"subscriptions":{}}');
  await writeFile(join(root, "packages/conversations-package/ui/dist/index.js"), "export const mount = () => () => {};\n");
  await writeFile(join(root, "packages/conversations-package/ui/dist/style.css"), ".conversations {}\n");
  const backend = join(root, "packages/conversations-package/backend/fake");
  await writeFile(backend, "backend bytes");
  await writeFile(join(root, "packages/conversations-package/teamarness.package.json"), JSON.stringify({
    schemaVersion: 1,
    identity: "dev.teamarness.conversations",
    version: "1.0.0",
    presentation: { displayName: "Conversations" },
    protocol: { major: 1, minMinor: 0, maxMinor: 0 },
    backends: [{ target: "aarch64-apple-darwin", path: "backend/teamarness-conversations-package-aarch64-apple-darwin", size: 0, sha256: zeros }],
    ui: { entrypoint: { path: "ui/index.js", size: 0, sha256: zeros }, assets: [{ path: "ui/style.css", mediaType: "text/css", size: 0, sha256: zeros }], contract: { path: "ui-contract.json", sha256: zeros } },
    capabilities: [],
  }));
  const output = join(root, "target/teamarness-packages/dev.teamarness.conversations");
  await buildConversationsPackage({ repositoryRoot: root, backendPath: backend, outputDirectory: output, skipBuilds: true, skipUiBuild: true });
  assert.equal(await readFile(join(root, "packages/application-packages.v1.json"), "utf8"), '{"untouched":true}\n');
  const manifest = JSON.parse(await readFile(join(output, "teamarness.package.json"), "utf8"));
  assert.equal(manifest.identity, "dev.teamarness.conversations");
  assert.equal(manifest.ui.assets[0].size, ".conversations {}\n".length);
  assert.equal((await stat(join(output, manifest.backends[0].path))).mode & 0o777, 0o700);
});
