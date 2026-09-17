import { mkdir, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  APPLICATION_INDEX_PATH,
  STAGING_ROOT,
  applicationIndexDocument,
  readBundledPackages,
  replaceFile,
  serializeJson,
} from "./bundled-packages.mjs";
import { buildPackage } from "./build-package.mjs";
import { compareDeclaredPackageBytes, copyPackageDirectory } from "./package-directory.mjs";

const scriptPath = fileURLToPath(import.meta.url);
export const repositoryRoot = resolve(dirname(scriptPath), "..");

export async function buildApplicationPackages({
  repositoryRoot: root = repositoryRoot,
  skipBuilds = false,
  skipUiBuild = false,
  backendPaths = {},
} = {}) {
  const packages = await readBundledPackages(root);
  const outputRoot = join(root, STAGING_ROOT);
  await rm(outputRoot, { recursive: true, force: true });
  await mkdir(outputRoot, { recursive: true, mode: 0o700 });
  const directories = [];
  for (const { sourceDirectory, cargoPackage, identity } of packages) {
    directories.push(await buildPackage({
      repositoryRoot: root,
      sourceDirectory,
      cargoPackage,
      expectedIdentity: identity,
      backendPath: backendPaths[identity],
      outputDirectory: join(outputRoot, identity),
      skipBuilds,
      skipUiBuild,
    }));
  }
  const indexPath = join(root, APPLICATION_INDEX_PATH);
  const index = serializeJson(applicationIndexDocument(packages));
  await replaceFile(indexPath, index);

  const debugRoot = join(root, "target/debug/packages");
  await rm(debugRoot, { recursive: true, force: true });
  await mkdir(debugRoot, { recursive: true, mode: 0o700 });
  for (const directory of directories) {
    const debugDirectory = join(debugRoot, directory.split("/").at(-1));
    await copyPackageDirectory(directory, debugDirectory);
    await compareDeclaredPackageBytes(directory, debugDirectory);
  }
  await writeFile(join(debugRoot, "application-packages.v1.json"), index, { mode: 0o600 });
  return { indexPath, directories };
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  buildApplicationPackages().then(({ directories }) => {
    console.log(`Application Packages staged and verified: ${directories.join(", ")}`);
  }).catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
