import assert from "node:assert/strict";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { execFile } from "node:child_process";
import test from "node:test";

import { createPackageProject } from "./create-package.mjs";

const run = promisify(execFile);

test("generated foundation passes its locked checks and assembles a Package Directory", { timeout: 900_000 }, async () => {
  const parent = await mkdtemp(join(tmpdir(), "teamarness-create-package-integration-"));
  try {
    const project = await createPackageProject({
      output: join(parent, "emojart"),
      identity: "dev.teamarness.emojart",
      displayName: "Emojart",
    });
    await run("cargo", ["test", "--workspace", "--locked"], { cwd: project });
    await run("npm", ["ci", "--ignore-scripts"], { cwd: project });
    await run("npm", ["run", "typecheck"], { cwd: project });
    await run("npm", ["run", "lint"], { cwd: project });
    await run("npm", ["test"], { cwd: project });
    await run("npm", ["run", "build"], { cwd: project });

    const directory = join(project, "dist/dev.teamarness.emojart");
    const manifest = JSON.parse(await readFile(join(directory, "teamarness.package.json"), "utf8"));
    assert.equal(manifest.identity, "dev.teamarness.emojart");
    assert.equal(manifest.backends[0].size > 0, true);
    assert.match(manifest.backends[0].sha256, /^[0-9a-f]{64}$/);
    assert.equal(manifest.ui.entrypoint.size > 0, true);
    assert.match(manifest.ui.entrypoint.sha256, /^[0-9a-f]{64}$/);
    assert.match(manifest.ui.contract.sha256, /^[0-9a-f]{64}$/);
  } finally {
    await rm(parent, { recursive: true, force: true });
  }
});
