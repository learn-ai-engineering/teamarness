import { spawnSync } from "node:child_process";
import { readFile, stat } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { STAGING_ROOT, readBundledPackages } from "./bundled-packages.mjs";
import { stagePackageDirectory, verifyPackageDirectory } from "./package-directory.mjs";

const scriptPath = fileURLToPath(import.meta.url);
export const repositoryRoot = resolve(dirname(scriptPath), "..");

export async function buildPackage({
  repositoryRoot: root = repositoryRoot,
  sourceDirectory,
  cargoPackage,
  expectedIdentity,
  backendPath,
  outputDirectory,
  skipBuilds = false,
  skipUiBuild = false,
  version,
} = {}) {
  if (!sourceDirectory) throw new Error("Package sourceDirectory is required");
  const source = join(root, sourceDirectory);
  const manifestPath = join(source, "teamarness.package.json");
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  const identity = expectedIdentity ?? manifest.identity;
  if (manifest.identity !== identity) {
    throw new Error(`Unexpected Package Identity in ${sourceDirectory}: ${manifest.identity} (expected ${identity})`);
  }
  const output = outputDirectory ?? join(root, STAGING_ROOT, identity);

  let builtBackend = backendPath;
  if (!skipBuilds) {
    if (!cargoPackage) throw new Error(`Package ${identity} cargoPackage is required to build its Backend`);
    run(root, "cargo", ["build", "-p", cargoPackage, "--release", "--locked"]);
    builtBackend = join(root, "target/release", cargoPackage);
  }
  if (!skipUiBuild) {
    const uiPackage = JSON.parse(await readFile(join(source, "ui/package.json"), "utf8"));
    run(root, "npm", ["--workspace", uiPackage.name, "run", "build"]);
  }
  if (!builtBackend || !(await isFile(builtBackend))) throw new Error(`Package ${identity} Backend was not built`);

  const uiAssets = [];
  for (const { path } of manifest.ui.assets) {
    const sourcePath = uiBuildOutput(source, path);
    if (!(await isFile(sourcePath))) throw new Error(`Package ${identity} UI asset build input is missing: ${path}`);
    uiAssets.push({ path, sourcePath });
  }
  const staged = await stagePackageDirectory({
    manifestPath,
    contractPath: join(source, "ui-contract.json"),
    backendPath: builtBackend,
    uiEntrypointPath: uiBuildOutput(source, manifest.ui.entrypoint.path),
    uiAssets,
    outputDirectory: output,
    version,
  });
  const verified = await verifyPackageDirectory(output);
  if (staged.identity !== identity || verified.identity !== identity) {
    throw new Error(`Unexpected staged Package Identity: ${verified.identity} (expected ${identity})`);
  }
  return output;
}

export async function resolveBundledPackage(selector, root = repositoryRoot) {
  const packages = await readBundledPackages(root);
  const normalized = selector?.replace(/\/+$/, "");
  const match = packages.find(({ identity, sourceDirectory }) => identity === normalized || sourceDirectory === normalized);
  if (!match) {
    const known = packages.map(({ identity }) => identity).join(", ");
    throw new Error(`Unknown bundled Package: ${selector ?? "(none)"}. Use an identity or source directory: ${known}`);
  }
  return match;
}

// Package-relative UI paths such as ui/style.css are built into ui/dist/style.css.
function uiBuildOutput(source, packagePath) {
  if (!packagePath.startsWith("ui/")) throw new Error(`Package UI path must be under ui/: ${packagePath}`);
  return join(source, "ui/dist", packagePath.slice("ui/".length));
}

async function isFile(path) {
  return (await stat(path).catch(() => undefined))?.isFile() === true;
}

function run(root, command, args) {
  const result = spawnSync(command, args, { cwd: root, stdio: "inherit", shell: false });
  if (result.status !== 0) throw new Error(`${command} failed with status ${result.status ?? "unknown"}`);
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  resolveBundledPackage(process.argv[2])
    .then(({ sourceDirectory, cargoPackage, identity }) =>
      buildPackage({ sourceDirectory, cargoPackage, expectedIdentity: identity }))
    .then((directory) => console.log(`Package staged and verified: ${directory}`))
    .catch((error) => { console.error(error.message); process.exitCode = 1; });
}
