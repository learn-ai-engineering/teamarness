import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { chmod, cp, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptPath = fileURLToPath(import.meta.url);
export const repositoryRoot = resolve(dirname(scriptPath), "..");
const canonicalDirectory = join(repositoryRoot, "target/teamarness-packages/dev.teamarness.reference");
const canonicalLocalCopy = join(repositoryRoot, "target/teamarness-packages-local/dev.teamarness.reference");

export async function stagePackageDirectory({
  manifestPath,
  contractPath,
  backendPath,
  uiEntrypointPath,
  uiAssets = [],
  outputDirectory,
  version,
}) {
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  const backend = await readFile(backendPath);
  const entrypoint = await readFile(uiEntrypointPath);
  const contract = await readFile(contractPath);
  if (version !== undefined) manifest.version = version;
  manifest.backends[0].size = backend.length;
  manifest.backends[0].sha256 = digest(backend);
  manifest.ui.entrypoint.size = entrypoint.length;
  manifest.ui.entrypoint.sha256 = digest(entrypoint);
  manifest.ui.contract.sha256 = digest(contract);
  if (manifest.ui.assets.length !== uiAssets.length) {
    throw new Error("Every UI asset must have one build input");
  }
  const assets = [];
  for (const input of uiAssets) {
    const declaration = manifest.ui.assets.find(({ path }) => path === input.path);
    if (!declaration) throw new Error(`Undeclared UI asset build input: ${input.path}`);
    const bytes = await readFile(input.sourcePath);
    declaration.size = bytes.length;
    declaration.sha256 = digest(bytes);
    assets.push({ path: input.path, bytes });
  }

  await rm(outputDirectory, { recursive: true, force: true });
  await mkdir(join(outputDirectory, "backend"), { recursive: true, mode: 0o700 });
  await mkdir(join(outputDirectory, "ui"), { recursive: true, mode: 0o700 });
  await writeFile(join(outputDirectory, manifest.backends[0].path), backend, { mode: 0o700 });
  await chmod(join(outputDirectory, manifest.backends[0].path), 0o700);
  await writeFile(join(outputDirectory, manifest.ui.entrypoint.path), entrypoint, { mode: 0o600 });
  for (const asset of assets) {
    await mkdir(dirname(join(outputDirectory, asset.path)), { recursive: true, mode: 0o700 });
    await writeFile(join(outputDirectory, asset.path), asset.bytes, { mode: 0o600 });
  }
  await writeFile(join(outputDirectory, manifest.ui.contract.path), contract, { mode: 0o600 });
  await writeFile(
    join(outputDirectory, "teamarness.package.json"),
    `${JSON.stringify(manifest, null, 2)}\n`,
    { mode: 0o600 },
  );
  return manifest;
}

export async function copyPackageDirectory(source, destination) {
  await rm(destination, { recursive: true, force: true });
  await mkdir(dirname(destination), { recursive: true, mode: 0o700 });
  await cp(source, destination, { recursive: true, preserveTimestamps: true });
}

export async function compareDeclaredPackageBytes(leftDirectory, rightDirectory) {
  const leftManifestBytes = await readFile(join(leftDirectory, "teamarness.package.json"));
  const rightManifestBytes = await readFile(join(rightDirectory, "teamarness.package.json"));
  if (!leftManifestBytes.equals(rightManifestBytes)) throw new Error("manifest byte mismatch");
  const manifest = JSON.parse(leftManifestBytes.toString("utf8"));
  const declarations = [
    ...manifest.backends.map(({ path, size, sha256 }) => ({ path, size, sha256 })),
    { ...manifest.ui.entrypoint },
    ...manifest.ui.assets.map(({ path, size, sha256 }) => ({ path, size, sha256 })),
    { ...manifest.ui.contract, size: undefined },
  ];
  let totalBytes = 0;
  for (const declaration of declarations) {
    const left = await readFile(join(leftDirectory, declaration.path));
    const right = await readFile(join(rightDirectory, declaration.path));
    if (!left.equals(right)) throw new Error(`declared byte mismatch: ${declaration.path}`);
    if (declaration.size !== undefined && declaration.size !== left.length) {
      throw new Error(`declared size mismatch: ${declaration.path}`);
    }
    if (declaration.sha256 !== digest(left) || declaration.sha256 !== digest(right)) {
      throw new Error(`declared digest mismatch: ${declaration.path}`);
    }
    totalBytes += left.length;
  }
  return { files: declarations.map(({ path }) => path), totalBytes };
}

async function buildReferencePackage(arguments_) {
  const options = parseArguments(arguments_);
  let backendPath = options.backend;
  if (!backendPath) {
    run("cargo", ["build", "-p", "teamarness-reference-package", "--release", "--locked"]);
    backendPath = join(repositoryRoot, "target/release/teamarness-reference-package");
  }
  if (!options.skipUiBuild) {
    run("npm", ["--workspace", "@teamarness/reference-package-ui", "run", "build"]);
  }
  const backendDetails = await stat(backendPath);
  if (!backendDetails.isFile()) throw new Error("Reference Package Backend was not built");
  await stagePackageDirectory({
    manifestPath: join(repositoryRoot, "packages/reference-package/teamarness.package.json"),
    contractPath: join(repositoryRoot, "packages/reference-package/ui-contract.json"),
    backendPath,
    uiEntrypointPath: join(repositoryRoot, "packages/reference-package/ui/dist/index.js"),
    uiAssets: [{
      path: "ui/probe.svg",
      sourcePath: join(repositoryRoot, "packages/reference-package/ui/dist/probe.svg"),
    }],
    outputDirectory: canonicalDirectory,
    version: options.version,
  });
  await writeFile(
    join(repositoryRoot, "packages/application-packages.v1.json"),
    `${JSON.stringify({ schemaVersion: 1, packages: [{ directory: "packages/dev.teamarness.reference" }] }, null, 2)}\n`,
  );
  const localCopy = options.copyTo ?? canonicalLocalCopy;
  await copyPackageDirectory(canonicalDirectory, localCopy);
  await compareDeclaredPackageBytes(canonicalDirectory, localCopy);
  console.log("Reference Package staged with verified manifest-declared bytes.");
}

function parseArguments(arguments_) {
  const options = { backend: undefined, copyTo: undefined, version: undefined, skipUiBuild: false };
  for (let index = 0; index < arguments_.length; index += 1) {
    const argument = arguments_[index];
    if (argument === "--skip-ui-build") {
      options.skipUiBuild = true;
      continue;
    }
    if (!["--backend", "--copy-to", "--version"].includes(argument)) {
      throw new Error(`Unknown reference Package build option: ${argument}`);
    }
    const value = arguments_[index += 1];
    if (!value) throw new Error(`Missing value for ${argument}`);
    if (argument === "--backend") options.backend = resolve(value);
    if (argument === "--copy-to") options.copyTo = resolve(value);
    if (argument === "--version") options.version = value;
  }
  return options;
}

function run(command, args) {
  const result = spawnSync(command, args, { cwd: repositoryRoot, stdio: "inherit", shell: false });
  if (result.status !== 0) throw new Error(`${command} failed with status ${result.status ?? "unknown"}`);
}

function digest(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  buildReferencePackage(process.argv.slice(2)).catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
