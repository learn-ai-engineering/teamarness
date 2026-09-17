import assert from "node:assert/strict";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { execFile } from "node:child_process";
import test from "node:test";

import { buildPackage, resolveBundledPackage } from "./build-package.mjs";
import { createPackageProject, repositoryRoot } from "./create-package.mjs";

const run = promisify(execFile);

test("generated foundation passes its locked checks and assembles a Package Directory", { timeout: 900_000 }, async () => {
  const parent = await mkdtemp(join(tmpdir(), "teamarness-create-package-integration-"));
  const displayName = 'Emojart "$&" __REQUEST_NAMESPACE__ __PACKAGE_DISPLAY_NAME_RUST__ \\ Gallery';
  try {
    const project = await createPackageProject({
      output: join(parent, "emojart project"),
      identity: "dev.teamarness.emojart",
      displayName,
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
    assert.equal(manifest.presentation.displayName, displayName);
    assert.equal(manifest.backends[0].size > 0, true);
    assert.match(manifest.backends[0].sha256, /^[0-9a-f]{64}$/);
    assert.equal(manifest.ui.entrypoint.size > 0, true);
    assert.match(manifest.ui.entrypoint.sha256, /^[0-9a-f]{64}$/);
    assert.match(manifest.ui.contract.sha256, /^[0-9a-f]{64}$/);

    const tutorial = await resolveBundledPackage("dev.teamarness.tutorial");
    const tutorialDirectory = await buildPackage({
      sourceDirectory: tutorial.sourceDirectory,
      cargoPackage: tutorial.cargoPackage,
      expectedIdentity: tutorial.identity,
      outputDirectory: join(parent, "bundled tutorial"),
    });
    await run(
      "cargo",
      [
        "test",
        "-p",
        "teamarness-package-host",
        "--test",
        "external_package_authoring",
        "--locked",
        "--",
        "--ignored",
      ],
      {
        cwd: repositoryRoot,
        env: {
          ...process.env,
          TEAMARNESS_BUNDLED_TUTORIAL_DIRECTORY: tutorialDirectory,
          TEAMARNESS_GENERATED_PACKAGE_DIRECTORY: directory,
          TEAMARNESS_GENERATED_PACKAGE_DISPLAY_NAME: displayName,
        },
      },
    );
  } finally {
    await rm(parent, { recursive: true, force: true });
  }
});
