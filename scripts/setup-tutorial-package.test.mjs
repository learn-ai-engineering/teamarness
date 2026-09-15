import assert from "node:assert/strict";
import { lstat, mkdir, mkdtemp, readFile, realpath, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { setupTutorialPackage } from "./setup-tutorial-package.mjs";

async function createPackage(root) {
  const directory = join(root, "target/teamarness-packages-local/dev.teamarness.tutorial");
  await mkdir(directory, { recursive: true });
  return directory;
}

function fakeBuild(directory) {
  return async () => directory;
}

test("builds Tutorial and writes one private canonical registration for a named profile", async () => {
  const root = await mkdtemp(join(tmpdir(), "teamarness-tutorial-setup-"));
  try {
    const directory = await createPackage(root);
    const registrationPath = await setupTutorialPackage({
      profileName: "tutorial-preview",
      repositoryRoot: root,
      buildPackage: fakeBuild(directory),
    });
    const registration = JSON.parse(await readFile(registrationPath, "utf8"));

    assert.equal(registrationPath, join(await realpath(root), ".teamarness-dev/tutorial-preview/packages/registrations.v1.json"));
    assert.deepEqual(registration, {
      schemaVersion: 1,
      registrations: [{
        directory: await realpath(directory),
        expectedIdentity: "dev.teamarness.tutorial",
        trust: {
          kind: "trusted_native_code_v1",
          acceptedRisks: ["filesystem", "network", "process"],
        },
      }],
    });
    for (const path of [join(root, ".teamarness-dev/tutorial-preview"), join(root, ".teamarness-dev/tutorial-preview/packages")]) {
      assert.equal((await stat(path)).mode & 0o777, 0o700);
    }
    assert.equal((await stat(registrationPath)).mode & 0o777, 0o600);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("replaces its Tutorial registration without changing unrelated registrations", async () => {
  const root = await mkdtemp(join(tmpdir(), "teamarness-tutorial-setup-"));
  try {
    const directory = await createPackage(root);
    const packagesDirectory = join(root, ".teamarness-dev/tutorial-preview/packages");
    await mkdir(packagesDirectory, { recursive: true });
    await writeFile(join(packagesDirectory, "registrations.v1.json"), JSON.stringify({
      schemaVersion: 1,
      registrations: [
        {
          directory: "/opt/other-package",
          expectedIdentity: "dev.example.other",
          trust: { kind: "trusted_native_code_v1", acceptedRisks: ["filesystem", "network", "process"] },
        },
        {
          directory: "/stale/tutorial",
          expectedIdentity: "dev.teamarness.tutorial",
          trust: { kind: "trusted_native_code_v1", acceptedRisks: ["filesystem", "network", "process"] },
        },
      ],
    }));

    const registrationPath = await setupTutorialPackage({
      profileName: "tutorial-preview",
      repositoryRoot: root,
      buildPackage: fakeBuild(directory),
    });
    await setupTutorialPackage({
      profileName: "tutorial-preview",
      repositoryRoot: root,
      buildPackage: fakeBuild(directory),
    });
    const registration = JSON.parse(await readFile(registrationPath, "utf8"));

    assert.equal(registration.registrations.length, 2);
    assert.deepEqual(registration.registrations[0], {
      directory: "/opt/other-package",
      expectedIdentity: "dev.example.other",
      trust: { kind: "trusted_native_code_v1", acceptedRisks: ["filesystem", "network", "process"] },
    });
    assert.equal(registration.registrations[1].directory, await realpath(directory));
    assert.equal(registration.registrations[1].expectedIdentity, "dev.teamarness.tutorial");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("rejects unsafe profile names and malformed registration files without overwriting them", async () => {
  const root = await mkdtemp(join(tmpdir(), "teamarness-tutorial-setup-"));
  try {
    const directory = await createPackage(root);
    await assert.rejects(
      setupTutorialPackage({ profileName: "../outside", repositoryRoot: root, buildPackage: fakeBuild(directory) }),
      /Profile names/,
    );

    const packagesDirectory = join(root, ".teamarness-dev/tutorial-preview/packages");
    await mkdir(packagesDirectory, { recursive: true });
    const registrationPath = join(packagesDirectory, "registrations.v1.json");
    await writeFile(registrationPath, "not json\n");
    await assert.rejects(
      setupTutorialPackage({ profileName: "tutorial-preview", repositoryRoot: root, buildPackage: fakeBuild(directory) }),
      /valid registrations document/,
    );
    assert.equal(await readFile(registrationPath, "utf8"), "not json\n");
    assert.equal((await lstat(registrationPath)).isFile(), true);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
