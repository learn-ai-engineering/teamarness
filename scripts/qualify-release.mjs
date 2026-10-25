import { copyFile, lstat, mkdir, mkdtemp, readFile, readdir, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { repositoryRoot } from "./bundled-packages.mjs";
import { assetNames, createManifest, TARGETS, validateManifest } from "./release/manifest.mjs";
import { checkVersion } from "./release/version.mjs";
import { checkAssetDirectory, extractArchive, fileDigest, native, qualifyBundle, withMountedDmg } from "./release/artifacts.mjs";

export async function qualifyRelease({ root = repositoryRoot, directory, tag, publicKey, collect = false, notes = "" }) {
  if (process.platform !== "darwin" || process.arch !== "arm64") throw new Error("Release qualification requires native Apple Silicon macOS");
  await checkVersion({ root, tag });
  const names = assetNames(tag);
  const allowed = join(root, "target/release/distribution");
  if (resolve(directory) !== resolve(allowed)) throw new Error("Expected allowlisted target/release/distribution output directory");
  await mkdir(directory, { recursive: true, mode: 0o700 });
  if (await realpath(directory) !== join(await realpath(root), "target/release/distribution")) throw new Error("Output directory must not use symlinks");
  const existing = await readdir(directory);
  if (existing.some((name) => ![...names, "qualification.json"].includes(name))) throw new Error("Unexpected release assets in output directory");
  await rm(join(directory, "qualification.json"), { force: true }); // A failed rerun can never retain a passing record.
  const app = join(root, "target/release/bundle/macos/Teamarness.app");
  if (collect) {
    for (const name of names.slice(0, 3)) {
      const source = join(root, "target/release/bundle", name.endsWith(".dmg") ? "dmg" : "macos", name);
      if (!(await lstat(source)).isFile()) throw new Error(`Missing built release asset: ${name}`);
      // Replace only these named assets, never copy a broad target directory.
      await rm(join(directory, name), { force: true });
      await copyFile(source, join(directory, name));
    }
    const signature = await readFile(join(directory, names[2]), "utf8");
    await rm(join(directory, "latest.json"), { force: true });
    await writeFile(join(directory, "latest.json"), `${JSON.stringify(createManifest({ tag, signature, notes }), null, 2)}\n`, { mode: 0o600 });
  }
  await checkAssetDirectory({ root, directory, tag });
  const signature = await readFile(join(directory, names[2]), "utf8");
  const manifestPath = join(directory, "latest.json");
  const manifest = validateManifest(JSON.parse(await readFile(manifestPath, "utf8")), { tag, signature });
  const keyInfo = await lstat(publicKey);
  if (!keyInfo.isFile() || keyInfo.size > 4096) throw new Error("Expected bounded regular public-key file");
  const archive = join(directory, names[1]);
  for (const target of TARGETS) {
    console.log(native("cargo", ["run", "--quiet", "--locked", "-p", "teamarness-desktop", "--example", "qualify-update", "--", "--manifest", manifestPath, "--archive", archive, "--public-key", publicKey, "--tag", tag, "--target", target], { root }));
  }
  const temporary = await mkdtemp(join(tmpdir(), "teamarness-release-extract-"));
  try {
    const appDigest = await qualifyBundle({ root, app, tag });
    const extracted = await extractArchive(archive, join(temporary, "archive"));
    if (await qualifyBundle({ root, app: extracted, tag }) !== appDigest) throw new Error("Updater archive application differs from qualified build");
    await withMountedDmg(join(directory, names[0]), async (mounted) => {
      if (await qualifyBundle({ root, app: mounted, tag }) !== appDigest) throw new Error("DMG application differs from qualified build");
    });
    const digests = Object.fromEntries(await Promise.all(names.map(async (name) => [name, await fileDigest(join(directory, name))])));
    const record = {
      schemaVersion: 1,
      sourceSha: native("git", ["rev-parse", "HEAD"], { root }),
      workingTreeDirty: native("git", ["status", "--porcelain"], { root }) !== "",
      tag, version: manifest.version, identifier: "dev.teamarness.desktop", architecture: "arm64", appDigest, digests,
    };
    await writeFile(join(directory, "qualification.json"), `${JSON.stringify(record, null, 2)}\n`, { mode: 0o600 });
    await checkAssetDirectory({ root, directory, tag });
    console.log(`Qualified ${tag}: app, readonly DMG, signed updater archive and every bundled Package. Only ${names.join(", ")} are public assets.`);
    return record;
  } finally { await rm(temporary, { recursive: true, force: true }); }
}

const HELP = `Noninteractive local release qualification (no Accessibility, network, publication or installation).
Usage: npm run verify:release-artifacts -- --directory target/release/distribution --tag vX.Y.Z --public-key FILE [--collect] [--notes FILE]
--collect copies only the exact built DMG/archive/signature and generates bounded latest.json.
Without --collect, validate existing assets without replacing them. qualification.json is regenerated only on success.
Only target/release/distribution is allowlisted. Public key is a reviewed build input, never a private key.
The CI-only qualification.json records source SHA, dirty-source status and digests; never publish it.
Requires Rust 1.99.1, Node 24.20.0 and native Apple Silicon macOS.`;

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const run = async () => {
    const args = process.argv.slice(2);
    if (args.length === 1 && args[0] === "--help") { console.log(HELP); return; }
    const options = {};
    for (let at = 0; at < args.length; at++) {
      const flag = args[at];
      if (flag === "--collect" && options.collect === undefined) options.collect = true;
      else if (["--directory", "--tag", "--public-key", "--notes"].includes(flag) && args[at + 1] && options[flag] === undefined) options[flag] = args[++at];
      else throw new Error("Invalid or duplicate arguments; use --help");
    }
    for (const flag of ["--directory", "--tag", "--public-key"]) if (!options[flag]) throw new Error("Missing required arguments; use --help");
    let notes = "";
    if (options["--notes"]) {
      const info = await lstat(options["--notes"]);
      if (!info.isFile() || info.size > 4096) throw new Error("Notes must be a regular file of at most 4 KiB");
      notes = await readFile(options["--notes"], "utf8");
    }
    await qualifyRelease({ directory: resolve(options["--directory"]), tag: options["--tag"], publicKey: resolve(options["--public-key"]), collect: options.collect, notes });
  };
  run().catch((error) => { console.error(error.message); process.exitCode = 1; });
}
