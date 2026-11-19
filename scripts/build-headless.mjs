import { spawnSync } from "node:child_process";
import { chmod, copyFile, mkdir, rename, rm } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { readBundledPackages, repositoryRoot } from "./bundled-packages.mjs";
import { buildApplicationPackages } from "./build-application-packages.mjs";
import { cargoTargetDirectory } from "./build-package.mjs";
import { describeBuild } from "./build-application.mjs";
import { verifyHeadlessDistribution } from "./headless-distribution.mjs";
import { adhocSignBackend, nativeBackendTarget } from "./package-directory.mjs";

export async function buildHeadless({ root = repositoryRoot, target } = {}) {
  // Native preflight precedes every build and filesystem mutation, even for direct invocation.
  const selection = describeBuild(["--app", "headless", "--target", target], nativeBackendTarget());
  const output = join(root, selection.outputDirectory);
  const staging = `${output}.staging`;
  const packages = await readBundledPackages(root);
  const run = (command, args) => {
    const result = spawnSync(command, args, { cwd: root, stdio: "inherit", shell: false });
    if (result.status !== 0) throw new Error(`${command} failed (${result.status ?? "could not start"})`);
  };
  await rm(staging, { recursive: true, force: true });
  await mkdir(staging, { recursive: true, mode: 0o700 });
  try {
    run("cargo", ["build", "-p", "teamarness-headless", "--bin", "teamarness-headless", "--release", "--locked", "--target", target]);
    run("npm", ["--workspace", "@teamarness/package-ui-sdk", "run", "build"]);
    const executable = join(staging, "teamarness-headless");
    await copyFile(join(cargoTargetDirectory(root), target, "release/teamarness-headless"), executable);
    await chmod(executable, 0o700);
    if (target === "aarch64-apple-darwin") await adhocSignBackend(executable);
    await buildApplicationPackages({ repositoryRoot: root, target, outputRoot: join(staging, "packages"), release: true });
    const verified = await verifyHeadlessDistribution({ directory: staging, target, packages });
    await rm(output, { recursive: true, force: true });
    await rename(staging, output);
    console.log(`Headless distribution: ${output}\n${JSON.stringify(verified)}`);
    return { directory: output, ...verified };
  } finally { await rm(staging, { recursive: true, force: true }); }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  if (args.length !== 2 || args[0] !== "--target") {
    console.error("Usage: build-headless.mjs --target <native-target>"); process.exitCode = 2;
  } else buildHeadless({ target: args[1] }).catch((error) => { console.error(error.message); process.exitCode = 1; });
}
