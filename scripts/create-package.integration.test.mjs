import assert from "node:assert/strict";
import { copyFile, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { execFile } from "node:child_process";
import test from "node:test";

import { buildPackage, resolveBundledPackage } from "./build-package.mjs";
import { createPackageProject, repositoryRoot } from "./create-package.mjs";

const run = promisify(execFile);

test("generated foundation validates, builds, rejects an invalid contract, and runs beside Tutorial", { timeout: 900_000 }, async () => {
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
    const built = await run("npm", ["run", "build"], { cwd: project });
    assert.match(built.stdout.trimEnd(), /\nok$/, "the assembled Package Directory must validate");

    const directory = join(project, "dist/dev.teamarness.emojart");
    const manifest = JSON.parse(await readFile(join(directory, "teamarness.package.json"), "utf8"));
    assert.equal(manifest.identity, "dev.teamarness.emojart");
    assert.equal(manifest.presentation.displayName, displayName);
    assert.equal(manifest.backends[0].size > 0, true);
    assert.match(manifest.backends[0].sha256, /^[0-9a-f]{64}$/);
    assert.equal(manifest.ui.entrypoint.size > 0, true);
    assert.match(manifest.ui.entrypoint.sha256, /^[0-9a-f]{64}$/);
    assert.match(manifest.ui.contract.sha256, /^[0-9a-f]{64}$/);

    const provenance = JSON.parse(await readFile(join(project, "teamarness-authoring.json"), "utf8"));
    assert.equal(provenance.templateVersion, 2);
    assert.match(provenance.sdkRevision, /^([0-9a-f]{40}|unknown)$/);
    for (const tree of ["package-contract", "package-protocol", "package-sdk", "package-ui-sdk"]) {
      assert.match(provenance.vendorDigests[tree], /^sha256:[0-9a-f]{64}$/);
    }

    // An invalid UI contract is refused with the rule that failed, before anything is built.
    const contract = join(project, "ui-contract.json");
    const intact = await readFile(contract, "utf8");
    await copyFile(contract, `${contract}.intact`);
    const broken = JSON.parse(intact);
    broken.requests["emojart.read"].response.properties.message = { $ref: "https://example.com/schema" };
    await writeFile(contract, `${JSON.stringify(broken, null, 2)}\n`);
    const refused = await run("npm", ["run", "validate"], { cwd: project }).then(() => null, (error) => error);
    assert.ok(refused !== null, "a remote $ref must be refused");
    assert.match(
      `${refused.stdout}${refused.stderr}`,
      /ui_contract_violation\/ui_contract\.reference_not_local/,
    );
    await writeFile(contract, intact);
    await rm(`${contract}.intact`);
    await run("npm", ["run", "validate"], { cwd: project });

    // Phase 1 behaviour: the build honours a custom Cargo target directory.
    await run("npm", ["run", "build"], {
      cwd: project,
      env: { ...process.env, CARGO_TARGET_DIR: join(parent, "custom target") },
    });

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
