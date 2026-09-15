import { mkdir, rename, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { buildConversationsPackage } from "./build-conversations-package.mjs";
import { buildReferencePackage } from "./build-reference-package.mjs";
import { buildTutorialPackage } from "./build-tutorial-package.mjs";

const scriptPath = fileURLToPath(import.meta.url);
export const repositoryRoot = resolve(dirname(scriptPath), "..");
export const APPLICATION_PACKAGE_DECLARATIONS = Object.freeze([
  { identity: "dev.teamarness.reference", directory: "packages/dev.teamarness.reference" },
  { identity: "dev.teamarness.tutorial", directory: "packages/dev.teamarness.tutorial" },
  { identity: "dev.teamarness.conversations", directory: "packages/dev.teamarness.conversations" },
]);

export async function buildApplicationPackages({
  repositoryRoot: root = repositoryRoot,
  skipBuilds = false,
  skipUiBuild = false,
  referenceBackendPath,
  tutorialBackendPath,
  conversationsBackendPath,
} = {}) {
  const outputRoot = join(root, "target/teamarness-packages");
  await rm(outputRoot, { recursive: true, force: true });
  await mkdir(outputRoot, { recursive: true, mode: 0o700 });
  const referenceDirectory = join(outputRoot, "dev.teamarness.reference");
  const tutorialDirectory = join(outputRoot, "dev.teamarness.tutorial");
  const conversationsDirectory = join(outputRoot, "dev.teamarness.conversations");
  await buildReferencePackage({
    repositoryRoot: root,
    backendPath: referenceBackendPath,
    outputDirectory: referenceDirectory,
    skipBuilds,
    skipUiBuild,
  });
  await buildTutorialPackage({
    repositoryRoot: root,
    backendPath: tutorialBackendPath,
    outputDirectory: tutorialDirectory,
    skipBuilds,
    skipUiBuild,
  });
  await buildConversationsPackage({
    repositoryRoot: root,
    backendPath: conversationsBackendPath,
    outputDirectory: conversationsDirectory,
    skipBuilds,
    skipUiBuild,
  });
  const indexPath = join(root, "packages/application-packages.v1.json");
  const temporaryPath = `${indexPath}.tmp-${process.pid}`;
  const document = {
    schemaVersion: 1,
    packages: APPLICATION_PACKAGE_DECLARATIONS.map(({ directory }) => ({ directory })),
  };
  await writeFile(temporaryPath, `${JSON.stringify(document, null, 2)}\n`, { mode: 0o600 });
  await rename(temporaryPath, indexPath);
  return { indexPath, directories: [referenceDirectory, tutorialDirectory, conversationsDirectory] };
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  buildApplicationPackages().then(({ directories }) => {
    console.log(`Application Packages staged and verified: ${directories.join(", ")}`);
  }).catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
