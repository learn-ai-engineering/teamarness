import { spawnSync } from "node:child_process";
import { stat, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  compareDeclaredPackageBytes,
  copyPackageDirectory,
  stagePackageDirectory,
} from "./package-directory.mjs";

export {
  compareDeclaredPackageBytes,
  copyPackageDirectory,
  stagePackageDirectory,
};

const scriptPath = fileURLToPath(import.meta.url);
export const repositoryRoot = resolve(dirname(scriptPath), "..");
const canonicalDirectory = join(repositoryRoot, "target/teamarness-packages/dev.teamarness.reference");
const canonicalLocalCopy = join(repositoryRoot, "target/teamarness-packages-local/dev.teamarness.reference");

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

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  buildReferencePackage(process.argv.slice(2)).catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
