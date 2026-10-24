import { createHash } from "node:crypto";
import { chmod, cp, lstat, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";

export async function stagePackageDirectory({
  manifestPath,
  contractPath,
  portContractPath,
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
  const portContract = manifest.portContract && await readFile(portContractPath ?? join(dirname(manifestPath), manifest.portContract.path));
  if (portContract) manifest.portContract.sha256 = digest(portContract);
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
  if (portContract) {
    const destination = join(outputDirectory, manifest.portContract.path);
    await mkdir(dirname(destination), { recursive: true, mode: 0o700 });
    await writeFile(destination, portContract, { mode: 0o600 });
  }
  await writeFile(
    join(outputDirectory, "teamarness.package.json"),
    `${JSON.stringify(manifest, null, 2)}\n`,
    { mode: 0o600 },
  );
  return manifest;
}

export async function verifyPackageDirectory(directory) {
  const manifestPath = join(directory, "teamarness.package.json");
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  const declarations = [
    ...manifest.backends.map(({ path, size, sha256 }) => ({ path, size, sha256, mode: 0o700 })),
    { ...manifest.ui.entrypoint, mode: 0o600 },
    ...manifest.ui.assets.map(({ path, size, sha256 }) => ({ path, size, sha256, mode: 0o600 })),
    { ...manifest.ui.contract, size: undefined, mode: 0o600 },
    ...(manifest.portContract ? [{ ...manifest.portContract, size: undefined, mode: 0o600 }] : []),
  ];
  for (const declaration of declarations) {
    const path = join(directory, declaration.path);
    const details = await lstat(path);
    if (!details.isFile() || details.isSymbolicLink()) throw new Error(`declared path is not a regular file: ${declaration.path}`);
    const bytes = await readFile(path);
    if (declaration.size !== undefined && declaration.size !== bytes.length) throw new Error(`declared size mismatch: ${declaration.path}`);
    if (declaration.sha256 !== digest(bytes) || declaration.sha256 !== declaration.sha256.toLowerCase()) throw new Error(`declared digest mismatch: ${declaration.path}`);
    if (process.platform !== "win32" && (details.mode & 0o777) !== declaration.mode) throw new Error(`declared mode mismatch: ${declaration.path}`);
  }
  const manifestDetails = await stat(manifestPath);
  if (process.platform !== "win32" && (manifestDetails.mode & 0o777) !== 0o600) throw new Error("manifest mode mismatch");
  return { identity: manifest.identity, files: declarations.map(({ path }) => path) };
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
    ...(manifest.portContract ? [{ ...manifest.portContract, size: undefined }] : []),
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

function digest(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}
