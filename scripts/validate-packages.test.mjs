import assert from "node:assert/strict";
import { cp, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";

import { readBundledPackages, repositoryRoot } from "./bundled-packages.mjs";
import { validateBundledPackages } from "./validate-packages.mjs";

const temporaryRoots = [];

after(async () => {
  await Promise.all(temporaryRoots.map((path) => rm(path, { recursive: true, force: true })));
});

test("validates every bundled Package source and staged directory", async () => {
  const ran = await validateBundledPackages();
  const packages = await readBundledPackages();
  const sources = ran.filter(({ mode }) => mode === "source");
  assert.equal(sources.length, packages.length);
  assert.deepEqual(
    sources.map(({ identity }) => identity).sort(),
    packages.map(({ identity }) => identity).sort(),
  );
  // `npm run check` builds before it tests, so the staged directories usually exist; when they do
  // not, only the source check runs.
  for (const { identity, mode } of ran) {
    assert.ok(mode === "source" || packages.some((entry) => entry.identity === identity));
  }
});

test("reports the Package and rule for an invalid source", async () => {
  const root = await mkdtemp(join(tmpdir(), "teamarness-validate-"));
  temporaryRoots.push(root);
  const sourceDirectory = "packages/tutorial";
  await cp(join(repositoryRoot, sourceDirectory), join(root, sourceDirectory), { recursive: true });
  const contractPath = join(root, sourceDirectory, "ui-contract.json");
  const contract = JSON.parse(await readFile(contractPath, "utf8"));
  contract.$defs.remote = { $ref: "https://example.com/schema.json" };
  await writeFile(contractPath, JSON.stringify(contract, null, 2));

  await assert.rejects(
    () =>
      validateBundledPackages({
        repositoryRoot,
        smoke: false,
        packages: [{ identity: "dev.teamarness.tutorial", sourceDirectory: join(root, sourceDirectory) }],
      }),
    (error) => {
      assert.match(error.message, /dev\.teamarness\.tutorial/);
      assert.match(error.message, /ui_contract\.reference_not_local/);
      return true;
    },
  );
});
