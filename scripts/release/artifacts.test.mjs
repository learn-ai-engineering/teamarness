import assert from "node:assert/strict";
import { chmod, copyFile, cp, mkdir, mkdtemp, readFile, rm, symlink, truncate, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { repositoryRoot } from "../bundled-packages.mjs";
import { compareDeclaredPackageBytes } from "../package-directory.mjs";
import { assetNames, ARCHIVE_LIMIT } from "./manifest.mjs";
import { assertArm64, assertBundleIdentity, checkAssetDirectory, extractArchive, qualifyBundle } from "./artifacts.mjs";

async function fixture(body) {
  const root = await mkdtemp(join(tmpdir(), "release-artifacts-"));
  try { await body(root); } finally { await rm(root, { recursive: true, force: true }); }
}

// Verifies: TEAM.DEL-R01, TEAM.DEL-R02.
test("output contract rejects missing/extra/symlink assets and enforces the archive cap", async () => fixture(async (root) => {
  const directory = join(root, "target/release/distribution");
  await mkdir(directory, { recursive: true });
  const names = assetNames("v1.2.3");
  for (const name of names) await writeFile(join(directory, name), "bytes");
  await checkAssetDirectory({ root, directory, tag: "v1.2.3" });
  await writeFile(join(directory, "secret.key"), "not permitted");
  await assert.rejects(checkAssetDirectory({ root, directory, tag: "v1.2.3" }), /assets/);
  await rm(join(directory, "secret.key"));
  await rm(join(directory, names[0]));
  await assert.rejects(checkAssetDirectory({ root, directory, tag: "v1.2.3" }), /assets/);
  await symlink(join(directory, names[1]), join(directory, names[0]));
  await assert.rejects(checkAssetDirectory({ root, directory, tag: "v1.2.3" }), /regular/);
  await rm(join(directory, names[0])); await writeFile(join(directory, names[0]), "bytes");
  await truncate(join(directory, names[1]), ARCHIVE_LIMIT);
  await checkAssetDirectory({ root, directory, tag: "v1.2.3" });
  await truncate(join(directory, names[1]), ARCHIVE_LIMIT + 1);
  await assert.rejects(checkAssetDirectory({ root, directory, tag: "v1.2.3" }), /cap/);
  await assert.rejects(checkAssetDirectory({ root, directory: root, tag: "v1.2.3" }), /allowlisted/);
}));

test("wrong architecture, identifier and embedded version fail", () => {
  assertArm64("arm64");
  for (const arch of ["x86_64", "x86_64 arm64", ""]) assert.throws(() => assertArm64(arch));
  assertBundleIdentity({ CFBundleIdentifier: "dev.teamarness.desktop", CFBundleShortVersionString: "1.2.3", CFBundleVersion: "1.2.3", CFBundleExecutable: "teamarness-desktop" }, "v1.2.3");
  for (const overrides of [{ CFBundleIdentifier: "other" }, { CFBundleShortVersionString: "1.2.2" }, { CFBundleVersion: "1.2.2" }, { CFBundleExecutable: "other" }]) assert.throws(() => assertBundleIdentity({ CFBundleIdentifier: "dev.teamarness.desktop", CFBundleShortVersionString: "1.2.3", CFBundleVersion: "1.2.3", CFBundleExecutable: "teamarness-desktop", ...overrides }, "v1.2.3"));
});

test("final bundle never skips absent staged Packages", async () => fixture(async (root) => {
  const app = join(root, "Teamarness.app");
  await mkdir(app);
  await assert.rejects(qualifyBundle({ root: repositoryRoot, app, tag: "v0.1.0", stagedRoot: join(root, "missing") }), /staged/);
}));

test("changed Backend bytes or missing declared asset fail exact comparison", async () => fixture(async (root) => {
  const source = join(root, "staged");
  await mkdir(join(source, "backend"), { recursive: true });
  await mkdir(join(source, "ui"));
  const declaration = (path, bytes) => ({ path, size: Buffer.byteLength(bytes), sha256: createHash("sha256").update(bytes).digest("hex") });
  const fixtureManifest = { backends: [declaration("backend/example", "Backend bytes")], ui: { entrypoint: declaration("ui/index.js", "UI bytes"), assets: [], contract: declaration("ui/contract.json", "{}") } };
  await writeFile(join(source, "backend/example"), "Backend bytes");
  await writeFile(join(source, "ui/index.js"), "UI bytes");
  await writeFile(join(source, "ui/contract.json"), "{}");
  await writeFile(join(source, "teamarness.package.json"), JSON.stringify(fixtureManifest));
  const copy = join(root, "package"); await cp(source, copy, { recursive: true });
  const manifest = JSON.parse(await readFile(join(copy, "teamarness.package.json"), "utf8"));
  await compareDeclaredPackageBytes(source, copy);
  const backend = join(copy, manifest.backends[0].path);
  await chmod(backend, 0o700); await writeFile(backend, "altered Backend");
  await assert.rejects(compareDeclaredPackageBytes(source, copy), /byte mismatch/);
  await copyFile(join(source, manifest.backends[0].path), backend);
  await rm(join(copy, manifest.ui.entrypoint.path));
  await assert.rejects(compareDeclaredPackageBytes(source, copy), /ENOENT/);
}));

test("safe extraction rejects traversal, links, duplicate entries and unrelated roots", async () => fixture(async (root) => {
  const { spawnSync } = await import("node:child_process");
  for (const kind of ["traversal", "symlink", "hardlink", "duplicate", "other"]) {
    const archive = join(root, `${kind}.tar.gz`);
    const result = spawnSync("python3", ["-c", `import tarfile,io,sys
with tarfile.open(sys.argv[1], 'w:gz') as t:
 k=sys.argv[2]
 name='../escape' if k=='traversal' else 'Other.app/file' if k=='other' else 'Teamarness.app/file'
 info=tarfile.TarInfo(name); info.size=1
 if k in ('symlink','hardlink'): info.type=tarfile.SYMTYPE if k=='symlink' else tarfile.LNKTYPE; info.linkname='../outside'; info.size=0
 t.addfile(info,io.BytesIO(b'x'))
 if k=='duplicate': t.addfile(info,io.BytesIO(b'x'))`, archive, kind]);
    assert.equal(result.status, 0);
    await assert.rejects(extractArchive(archive, join(root, kind)), /archive|extract/);
  }
}));
