import { spawnSync } from "node:child_process";
import { stat } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { stagePackageDirectory } from "./package-directory.mjs";

const scriptPath = fileURLToPath(import.meta.url);
export const repositoryRoot = resolve(dirname(scriptPath), "..");

export async function buildEmojartPackage({
  repositoryRoot: root = repositoryRoot,
  backendPath,
  skipBuilds = false,
} = {}) {
  let builtBackend = backendPath;
  if (!skipBuilds) {
    run(root, "cargo", ["build", "-p", "teamarness-emojart-package", "--release", "--locked"]);
    run(root, "npm", ["--workspace", "@teamarness/emojart-package-ui", "run", "build"]);
    builtBackend = join(root, "target/release/teamarness-emojart-package");
  }
  if (!builtBackend) throw new Error("Emojart Package Backend path is required when builds are skipped");
  const backendDetails = await stat(builtBackend);
  if (!backendDetails.isFile()) throw new Error("Emojart Package Backend was not built");

  const outputDirectory = join(root, "target/teamarness-packages-local/dev.teamarness.emojart");
  const manifest = await stagePackageDirectory({
    manifestPath: join(root, "packages/emojart/teamarness.package.json"),
    contractPath: join(root, "packages/emojart/ui-contract.json"),
    backendPath: builtBackend,
    uiEntrypointPath: join(root, "packages/emojart/ui/dist/index.js"),
    outputDirectory,
  });
  if (manifest.identity !== "dev.teamarness.emojart") {
    throw new Error(`Unexpected Emojart Package Identity: ${manifest.identity}`);
  }
  console.log("Emojart Package staged for installation-local registration.");
  return outputDirectory;
}

function run(root, command, args) {
  const result = spawnSync(command, args, { cwd: root, stdio: "inherit", shell: false });
  if (result.status !== 0) throw new Error(`${command} failed with status ${result.status ?? "unknown"}`);
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  buildEmojartPackage().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
