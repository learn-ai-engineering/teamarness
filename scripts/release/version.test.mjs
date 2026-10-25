import assert from "node:assert/strict";
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { repositoryRoot } from "../bundled-packages.mjs";
import { checkVersion, setVersion, stableVersion, tagVersion, VERSION_FILES } from "./version.mjs";

async function fixture(body) {
  const root = await mkdtemp(join(tmpdir(), "release-version-"));
  try {
    for (const file of VERSION_FILES) {
      await mkdir(dirname(join(root, file)), { recursive: true });
      await copyFile(join(repositoryRoot, file), join(root, file));
    }
    await body(root);
  } finally { await rm(root, { recursive: true, force: true }); }
}

test("canonical stable versions and tags only", () => {
  for (const version of ["0.1.0", "1.2.3", "999.0.0"]) {
    assert.equal(stableVersion(version), version);
    assert.equal(tagVersion(`v${version}`), version);
  }
  for (const value of ["01.2.3", "1.2", "1.2.3-rc.1", "1.2.3+x", "v1.2.3", "1.2.3\n", "9007199254740992.0.0"]) assert.throws(() => stableVersion(value));
  for (const tag of ["1.2.3", "vv1.2.3", "v01.2.3", "v1.2.3+build"]) assert.throws(() => tagVersion(tag));
});

// Verifies: TEAM.DEL-R01. Desktop versions never change independent Package versions.
test("setter changes only the five desktop version entries, preserving all other bytes", async () => fixture(async (root) => {
  const before = await Promise.all(VERSION_FILES.map((file) => readFile(join(root, file), "utf8")));
  const current = await checkVersion({ root });
  await setVersion("9.8.7", { root });
  assert.equal(await checkVersion({ root, tag: "v9.8.7" }), "9.8.7");
  for (const [index, file] of VERSION_FILES.entries()) {
    const after = await readFile(join(root, file), "utf8");
    const first = before[index].indexOf(current, file === "Cargo.lock" ? before[index].indexOf('name = "teamarness-desktop"') : file === "package-lock.json" ? before[index].indexOf('"apps/teamarness-desktop"') : 0);
    assert.equal(after, before[index].slice(0, first) + "9.8.7" + before[index].slice(first + current.length));
  }
  await setVersion(current, { root });
  for (const [index, file] of VERSION_FILES.entries()) assert.equal(await readFile(join(root, file), "utf8"), before[index]);
}));

for (const file of VERSION_FILES) test(`version drift rejected in ${file}`, async () => fixture(async (root) => {
  const before = await readFile(join(root, file), "utf8");
  const start = file === "Cargo.lock" ? before.indexOf('name = "teamarness-desktop"') : file === "package-lock.json" ? before.indexOf('"apps/teamarness-desktop"') : 0;
  const version = await checkVersion({ root });
  await writeFile(join(root, file), before.slice(0, start) + before.slice(start).replace(version, "9.8.7"));
  await assert.rejects(checkVersion({ root }), /version/);
}));

for (const file of VERSION_FILES) test(`duplicate version declarations rejected in ${file}`, async () => fixture(async (root) => {
  const path = join(root, file);
  const before = await readFile(path, "utf8");
  const version = await checkVersion({ root });
  let after;
  if (file.endsWith(".json")) {
    if (file === "package-lock.json") after = before.replace('"apps/teamarness-desktop": {', `"apps/teamarness-desktop": {"version":"${version}",`);
    else after = before.replace(/}\s*$/, `,"version":"${version}"}`);
  } else {
    const start = file === "Cargo.lock" ? before.indexOf('name = "teamarness-desktop"') : 0;
    after = before.slice(0, start) + before.slice(start).replace(`version = "${version}"`, `version = "${version}"\nversion = "${version}"`);
  }
  await writeFile(path, after);
  await assert.rejects(checkVersion({ root }), /version/);
  await assert.rejects(setVersion("9.8.7", { root }), /version/);
  assert.equal(await readFile(path, "utf8"), after);
}));

test("tag disagreement and absent version declarations fail closed before writes", async () => fixture(async (root) => {
  await assert.rejects(checkVersion({ root, tag: "v99.0.0" }), /version/);
  const file = join(root, "apps/teamarness-desktop/src-tauri/Cargo.toml");
  const before = await readFile(file, "utf8");
  await writeFile(file, before.replace('version = ', 'missing = '));
  await assert.rejects(setVersion("9.8.7", { root }), /version/);
  assert.equal(await readFile(join(root, VERSION_FILES[0]), "utf8"), await readFile(join(repositoryRoot, VERSION_FILES[0]), "utf8"));
}));
