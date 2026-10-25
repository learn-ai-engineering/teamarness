// Registers the test-only Exhibits fixture consumer as a local Package in a data directory.
//
// The consumer (`exhibits-port-consumer-under-test`, built from `tests/system`) uploads embedded
// Exhibit fixtures through the real `dev.teamarness.exhibits` Ports. Its UI has one button per
// fixture and a frame probe. The packaged smoke uses it, and a developer uses it for manual
// checks in a named dev profile:
//
//   node scripts/exhibits-fixture-consumer/prepare.mjs <profile | absolute data directory>
//   npm run dev -- <profile>
//
// It never ships: nothing in the application bundle refers to it.
import { spawnSync } from "node:child_process";
import { chmod, copyFile, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { ensurePrivateDirectory, resolveDevelopmentDataDir } from "../dev-profile.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const repository = resolve(here, "../..");
const source = join(repository, "tests/system/exhibits-consumer");

export const EXHIBITS_FIXTURE_CONSUMER_IDENTITY = "dev.teamarness.exhibits-fixture-consumer";
export const EXHIBITS_FIXTURE_CONSUMER_DISPLAY_NAME = "Exhibits fixture consumer";
export const EXHIBITS_FIXTURE_CONSUMER_BINARY = "exhibits-port-consumer-under-test";
export const DEFAULT_CONSUMER_BACKEND = join(repository, "target/debug", EXHIBITS_FIXTURE_CONSUMER_BINARY);
const PACKAGE_DIRECTORY = "exhibits-fixture-consumer-package";

/** Builds the consumer binary with Cargo and returns its path. */
export function buildExhibitsFixtureConsumer() {
  const built = spawnSync("cargo", [
    "build", "--locked", "-p", "teamarness-system-tests", "--bin", EXHIBITS_FIXTURE_CONSUMER_BINARY,
  ], { cwd: repository, stdio: "inherit" });
  if (built.status !== 0) throw new Error("Building the Exhibits fixture consumer failed");
  return DEFAULT_CONSUMER_BACKEND;
}

/**
 * Writes a local Package Directory for the consumer, from its manifest template, with every
 * declared size and digest filled in, and registers it as trusted native code. Other local
 * registrations in the data directory are kept; an earlier consumer registration is replaced.
 */
export async function prepareExhibitsFixtureConsumer(dataDirectory, { backend = DEFAULT_CONSUMER_BACKEND } = {}) {
  const packageDirectory = join(dataDirectory, PACKAGE_DIRECTORY);
  const manifest = JSON.parse(await readFile(join(source, "teamarness.package.json"), "utf8"));
  const backendBytes = await readFile(backend);
  const entrypoint = await readFile(join(source, "ui/index.js"));
  const contract = await readFile(join(source, "ui-contract.json"));

  await rm(packageDirectory, { recursive: true, force: true });
  await mkdir(join(packageDirectory, "backend"), { recursive: true, mode: 0o700 });
  await mkdir(join(packageDirectory, "ui"), { recursive: true, mode: 0o700 });
  await mkdir(join(dataDirectory, "packages"), { recursive: true, mode: 0o700 });
  const backendPath = join(packageDirectory, manifest.backends[0].path);
  await copyFile(backend, backendPath);
  await chmod(backendPath, 0o700);
  await writeFile(join(packageDirectory, manifest.ui.entrypoint.path), entrypoint, { mode: 0o600 });
  await writeFile(join(packageDirectory, manifest.ui.contract.path), contract, { mode: 0o600 });

  manifest.backends[0].size = backendBytes.length;
  manifest.backends[0].sha256 = digest(backendBytes);
  manifest.ui.entrypoint.size = entrypoint.length;
  manifest.ui.entrypoint.sha256 = digest(entrypoint);
  manifest.ui.contract.sha256 = digest(contract);
  await writeFile(join(packageDirectory, "teamarness.package.json"), JSON.stringify(manifest), { mode: 0o600 });

  const registration = join(dataDirectory, "packages/registrations.v1.json");
  let registrations = [];
  try {
    const existing = JSON.parse(await readFile(registration, "utf8"));
    registrations = (existing.registrations ?? []).filter((entry) =>
      entry.expectedIdentity !== EXHIBITS_FIXTURE_CONSUMER_IDENTITY);
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  registrations.push({
    directory: packageDirectory,
    expectedIdentity: EXHIBITS_FIXTURE_CONSUMER_IDENTITY,
    trust: { kind: "trusted_native_code_v1", acceptedRisks: ["filesystem", "network", "process"] },
    overrideApplication: false,
  });
  await writeFile(registration, JSON.stringify({ schemaVersion: 1, registrations }), { mode: 0o600 });
  await chmod(dataDirectory, 0o700);
  await chmod(join(dataDirectory, "packages"), 0o700);
  await chmod(registration, 0o600);

  return {
    identity: EXHIBITS_FIXTURE_CONSUMER_IDENTITY,
    displayName: EXHIBITS_FIXTURE_CONSUMER_DISPLAY_NAME,
    packageDirectory,
  };
}

function digest(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

async function main(argv) {
  if (argv.length !== 1) {
    throw new Error("Usage: node scripts/exhibits-fixture-consumer/prepare.mjs <profile | absolute data directory>");
  }
  const dataDirectory = resolveDevelopmentDataDir(argv[0]);
  await ensurePrivateDirectory(dataDirectory);
  const backend = buildExhibitsFixtureConsumer();
  const { displayName } = await prepareExhibitsFixtureConsumer(dataDirectory, { backend });
  console.log(`Registered "${displayName}" in ${dataDirectory}. It appears under Personal Packages.`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
