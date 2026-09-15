import { spawnSync } from "node:child_process";
import { stat } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { stagePackageDirectory } from "./package-directory.mjs";

const scriptPath = fileURLToPath(import.meta.url);
export const repositoryRoot = resolve(dirname(scriptPath), "..");

export async function buildTutorialPackage({
  repositoryRoot: root = repositoryRoot,
  backendPath,
  skipBuilds = false,
} = {}) {
  let builtBackend = backendPath;
  if (!skipBuilds) {
    run(root, "cargo", ["build", "-p", "teamarness-tutorial-package", "--release", "--locked"]);
    run(root, "npm", ["--workspace", "@teamarness/tutorial-package-ui", "run", "build"]);
    builtBackend = join(root, "target/release/teamarness-tutorial-package");
  }
  if (!builtBackend) throw new Error("Tutorial Package Backend path is required when builds are skipped");
  const backendDetails = await stat(builtBackend);
  if (!backendDetails.isFile()) throw new Error("Tutorial Package Backend was not built");

  const outputDirectory = join(root, "target/teamarness-packages-local/dev.teamarness.tutorial");
  const manifest = await stagePackageDirectory({
    manifestPath: join(root, "packages/tutorial/teamarness.package.json"),
    contractPath: join(root, "packages/tutorial/ui-contract.json"),
    backendPath: builtBackend,
    uiEntrypointPath: join(root, "packages/tutorial/ui/dist/index.js"),
    outputDirectory,
  });
  if (manifest.identity !== "dev.teamarness.tutorial") {
    throw new Error(`Unexpected Tutorial Package Identity: ${manifest.identity}`);
  }
  console.log("Tutorial Package staged for installation-local registration.");
  return outputDirectory;
}

function run(root, command, args) {
  const result = spawnSync(command, args, { cwd: root, stdio: "inherit", shell: false });
  if (result.status !== 0) throw new Error(`${command} failed with status ${result.status ?? "unknown"}`);
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  buildTutorialPackage().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
