import { spawnSync } from "node:child_process";
import { stat } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { stagePackageDirectory, verifyPackageDirectory } from "./package-directory.mjs";

const scriptPath = fileURLToPath(import.meta.url);
export const repositoryRoot = resolve(dirname(scriptPath), "..");

export async function buildConversationsPackage({
  repositoryRoot: root = repositoryRoot,
  backendPath,
  outputDirectory,
  skipBuilds = false,
  skipUiBuild = false,
} = {}) {
  if (!outputDirectory) throw new Error("Conversations Package outputDirectory is required");
  let builtBackend = backendPath;
  if (!skipBuilds) {
    run(root, "cargo", ["build", "-p", "teamarness-conversations-package", "--release", "--locked"]);
    builtBackend = join(root, "target/release/teamarness-conversations-package");
  }
  if (!skipUiBuild) run(root, "npm", ["--workspace", "@teamarness/conversations-package-ui", "run", "build"]);
  if (!builtBackend || !(await stat(builtBackend)).isFile()) throw new Error("Conversations Package Backend was not built");
  const manifest = await stagePackageDirectory({
    manifestPath: join(root, "packages/conversations-package/teamarness.package.json"),
    contractPath: join(root, "packages/conversations-package/ui-contract.json"),
    backendPath: builtBackend,
    uiEntrypointPath: join(root, "packages/conversations-package/ui/dist/index.js"),
    uiAssets: [{ path: "ui/style.css", sourcePath: join(root, "packages/conversations-package/ui/dist/style.css") }],
    outputDirectory,
  });
  await verifyPackageDirectory(outputDirectory);
  if (manifest.identity !== "dev.teamarness.conversations") {
    throw new Error(`Unexpected Conversations Package Identity: ${manifest.identity}`);
  }
  return outputDirectory;
}

function run(root, command, args) {
  const result = spawnSync(command, args, { cwd: root, stdio: "inherit", shell: false });
  if (result.status !== 0) throw new Error(`${command} failed with status ${result.status ?? "unknown"}`);
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  buildConversationsPackage({ outputDirectory: join(repositoryRoot, "target/teamarness-packages/dev.teamarness.conversations") })
    .catch((error) => { console.error(error.message); process.exitCode = 1; });
}
