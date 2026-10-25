import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { lstat, mkdir, mkdtemp, readFile, readdir, realpath, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { isDeepStrictEqual } from "node:util";
import { APPLICATION_INDEX_PATH, STAGING_ROOT, applicationIndexDocument, readBundledPackages, repositoryRoot } from "../bundled-packages.mjs";
import { compareDeclaredPackageBytes, verifyPackageDirectory } from "../package-directory.mjs";
import { ARCHIVE_LIMIT, FEED_LIMIT, SIGNATURE_LIMIT, assetNames } from "./manifest.mjs";
import { tagVersion } from "./version.mjs";

export function native(command, args, { root = repositoryRoot } = {}) {
  const result = spawnSync(command, args, { cwd: root, encoding: "utf8", maxBuffer: 4 * 1024 * 1024, timeout: 120_000, shell: false });
  if (result.error || result.status !== 0) throw new Error(`${command} qualification failed: ${result.error?.message ?? result.stderr?.slice(-4096) ?? result.status}`);
  return result.stdout.trim();
}

export function assertArm64(architecture) {
  if (architecture.trim() !== "arm64") throw new Error("Expected arm64-only executable architecture");
}

export function assertBundleIdentity(info, tag) {
  const version = tagVersion(tag);
  if (info.CFBundleIdentifier !== "dev.teamarness.desktop" || info.CFBundleShortVersionString !== version
    || info.CFBundleVersion !== version || info.CFBundleExecutable !== "teamarness-desktop") throw new Error("Unexpected application identifier, executable or embedded version");
}

export async function checkAssetDirectory({ root = repositoryRoot, directory, tag }) {
  const allowed = join(root, "target/release/distribution");
  if (resolve(directory) !== resolve(allowed) || await realpath(directory) !== join(await realpath(root), "target/release/distribution")) throw new Error("Expected allowlisted target/release/distribution directory (no symlinks)");
  const names = assetNames(tag);
  const entries = (await readdir(directory)).sort();
  if (!isDeepStrictEqual(entries.filter((name) => name !== "qualification.json"), [...names].sort())) throw new Error("Missing or extra release assets");
  for (const name of entries) {
    const info = await lstat(join(directory, name));
    if (!info.isFile() || info.isSymbolicLink() || info.size === 0) throw new Error(`Release asset must be a nonempty regular file: ${name}`);
    const limit = name.endsWith(".tar.gz") ? ARCHIVE_LIMIT : name === "latest.json" ? FEED_LIMIT : name.endsWith(".sig") ? SIGNATURE_LIMIT : name === "qualification.json" ? FEED_LIMIT : undefined;
    if (limit !== undefined && info.size > limit) throw new Error(`Release asset exceeds byte cap: ${name}`);
  }
  return names;
}

export async function qualifyBundle({ root = repositoryRoot, app, tag, stagedRoot = join(root, STAGING_ROOT) }) {
  const packages = await readBundledPackages(root);
  // Never inherit validate:packages' intentionally permissive absent-stage behavior.
  for (const { identity } of packages) {
    if (!(await lstat(join(stagedRoot, identity)).catch(() => undefined))?.isDirectory()) throw new Error(`Required staged Package is missing: ${identity}`);
  }
  const contents = join(app, "Contents");
  const info = JSON.parse(native("plutil", ["-convert", "json", "-o", "-", join(contents, "Info.plist")], { root }));
  assertBundleIdentity(info, tag);
  const main = join(contents, "MacOS/teamarness-desktop");
  await executable(main, root);
  native("codesign", ["--verify", "--deep", "--strict", app], { root });
  const resources = join(contents, "Resources");
  const index = await readFile(join(resources, APPLICATION_INDEX_PATH));
  if (!index.equals(await readFile(join(root, APPLICATION_INDEX_PATH)))) throw new Error("Application Package index bytes differ from staged build");
  if (!isDeepStrictEqual(JSON.parse(index), applicationIndexDocument(packages))) throw new Error("Unexpected application Package index");
  const expectedDirectories = [...packages.map(({ identity }) => identity), "application-packages.v1.json"].sort();
  if (!isDeepStrictEqual((await readdir(join(resources, "packages"))).sort(), expectedDirectories)) throw new Error("Missing or extra bundled Package directories");
  for (const { identity } of packages) {
    const directory = join(resources, "packages", identity);
    await compareDeclaredPackageBytes(join(stagedRoot, identity), directory);
    await verifyPackageDirectory(directory);
    const manifest = JSON.parse(await readFile(join(directory, "teamarness.package.json"), "utf8"));
    for (const backend of manifest.backends) {
      if (backend.target !== "aarch64-apple-darwin") throw new Error("Unexpected declared Backend target");
      await executable(join(directory, backend.path), root);
    }
    // This is the existing admission validator and actual Backend handshake, not a GUI smoke.
    const report = JSON.parse(native("cargo", ["run", "--quiet", "--locked", "-p", "teamarness-package-contract", "--bin", "teamarness-package-validate", "--", "directory", directory, "--smoke", "--json"], { root }));
    if (report.ok !== true) throw new Error(`Final-bundle Package admission failed: ${identity}`);
    console.log(`Qualified final-bundle Package and Backend: ${identity}`);
  }
  return treeDigest(app);
}

async function executable(path, root) {
  const info = await lstat(path);
  if (!info.isFile() || !(info.mode & 0o111)) throw new Error("Expected executable regular file");
  assertArm64(native("lipo", ["-archs", path], { root }));
  native("codesign", ["--verify", "--strict", path], { root });
}

export async function treeDigest(directory) {
  const files = [];
  async function walk(path, relative) {
    for (const name of (await readdir(path)).sort()) {
      const file = join(path, name), local = relative ? `${relative}/${name}` : name;
      const info = await lstat(file);
      if (info.isSymbolicLink()) throw new Error("Application tree contains a symlink");
      if (info.isDirectory()) await walk(file, local);
      else if (info.isFile()) files.push([local, info.mode & 0o777, await fileDigest(file)]);
      else throw new Error("Application tree contains a special file");
    }
  }
  await walk(directory, "");
  return createHash("sha256").update(JSON.stringify(files)).digest("hex");
}

export async function fileDigest(path) {
  const { createReadStream } = await import("node:fs");
  const hash = createHash("sha256");
  for await (const bytes of createReadStream(path)) hash.update(bytes);
  return hash.digest("hex");
}

// Validate every tar header before extraction. No links, devices, duplicate paths or escapes;
// bound expanded bytes/entry count as well as the compressed updater cap.
export async function extractArchive(archive, destination) {
  if ((await lstat(archive)).size > ARCHIVE_LIMIT) throw new Error("Updater archive exceeds byte cap");
  await mkdir(destination, { mode: 0o700 });
  native("python3", ["-c", `import tarfile,pathlib,sys,os,shutil
seen=set(); total=0
with tarfile.open(sys.argv[1], 'r:gz') as archive:
 for entry in archive:
  name=entry.name
  parts=pathlib.PurePosixPath(name).parts
  if not parts or parts[0]!='Teamarness.app' or name.startswith('/') or '..' in parts or '\\\\' in name or not (entry.isfile() or entry.isdir()) or name in seen or entry.mode & 0o7000:
   raise ValueError('unsafe archive entry')
  seen.add(name); total+=entry.size
  if total>2*1024*1024*1024 or len(seen)>10000: raise ValueError('archive expansion cap')
# Reopen only after validating every header. Extract manually, never follow archive links.
with tarfile.open(sys.argv[1], 'r:gz') as archive:
 for entry in archive:
  out=pathlib.Path(sys.argv[2]).joinpath(*pathlib.PurePosixPath(entry.name).parts)
  if entry.isdir(): out.mkdir(parents=True,exist_ok=True)
  else:
   out.parent.mkdir(parents=True,exist_ok=True)
   with archive.extractfile(entry) as src, open(out,'xb') as dst: shutil.copyfileobj(src,dst)
   os.chmod(out,entry.mode & 0o777)
`, archive, destination]);
  return join(destination, "Teamarness.app");
}

export async function withMountedDmg(dmg, body) {
  const mount = await mkdtemp(join(tmpdir(), "teamarness-release-dmg-"));
  let attached = false;
  try {
    native("hdiutil", ["attach", "-readonly", "-nobrowse", "-noautoopen", "-mountpoint", mount, dmg]);
    attached = true;
    return await body(join(mount, "Teamarness.app"));
  } finally {
    if (attached) native("hdiutil", ["detach", mount]);
    await rm(mount, { recursive: true, force: true });
  }
}
