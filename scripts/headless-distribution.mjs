import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { lstat, readFile, readdir } from "node:fs/promises";
import { join } from "node:path";
import { applicationIndexDocument } from "./bundled-packages.mjs";
import { validateBackendTarget, verifyPackageDirectory } from "./package-directory.mjs";

async function verifyNativeExecutable(path, target) {
  const details = await lstat(path);
  if (!details.isFile() || (details.mode & 0o777) !== 0o700) throw new Error(`Native executable must be a private regular executable: ${path}`);
  const bytes = await readFile(path);
  const matches = target === "aarch64-apple-darwin"
    ? bytes.length >= 32 && bytes.readUInt32LE(0) === 0xfeedfacf && bytes.readUInt32LE(4) === 0x0100000c
    : bytes.length >= 64 && bytes.subarray(0, 6).equals(Buffer.from([0x7f, 0x45, 0x4c, 0x46, 2, 1])) && bytes.readUInt16LE(18) === 62;
  if (!matches) throw new Error(`Native executable bytes do not match ${target}: ${path}`);
  return createHash("sha256").update(bytes).digest("hex");
}

// Checks the final layout, not source manifest digests or Cargo's unsigned copies.
export async function verifyHeadlessDistribution({ directory, target, packages }) {
  validateBackendTarget(target);
  assert.deepEqual((await readdir(directory)).sort(), ["packages", "teamarness-headless"]);
  const executableSha256 = await verifyNativeExecutable(join(directory, "teamarness-headless"), target);
  const packageRoot = join(directory, "packages");
  assert.deepEqual((await readdir(packageRoot)).sort(), ["application-packages.v1.json", ...packages.map(({ identity }) => identity)].sort());
  assert.deepEqual(JSON.parse(await readFile(join(packageRoot, "application-packages.v1.json"))), applicationIndexDocument(packages));
  const backends = [];
  for (const { identity } of packages) {
    const packageDirectory = join(packageRoot, identity);
    const verified = await verifyPackageDirectory(packageDirectory);
    assert.equal(verified.identity, identity);
    const manifest = JSON.parse(await readFile(join(packageDirectory, "teamarness.package.json")));
    assert.equal(manifest.backends.length, 1);
    assert.equal(manifest.backends[0].target, target);
    const actualFiles = await readdir(packageDirectory, { recursive: true, withFileTypes: true });
    const files = actualFiles.filter((entry) => !entry.isDirectory()).map((entry) => {
      if (!entry.isFile()) throw new Error("Distribution contains a nonregular file");
      return join(entry.parentPath, entry.name).slice(packageDirectory.length + 1);
    });
    assert.deepEqual(files.sort(), ["teamarness.package.json", ...verified.files].sort());
    backends.push({ identity, sha256: await verifyNativeExecutable(join(packageDirectory, manifest.backends[0].path), target) });
  }
  return { target, executableSha256, identities: packages.map(({ identity }) => identity), backends };
}
