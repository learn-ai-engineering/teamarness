import { isDeepStrictEqual } from "node:util";
import { readFile, rename, writeFile } from "node:fs/promises";
import { dirname, join, posix, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptPath = fileURLToPath(import.meta.url);
export const repositoryRoot = resolve(dirname(scriptPath), "..");

// The one list of bundled Application Packages. Everything else is derived from it.
export const BUNDLED_PACKAGES_PATH = "packages/bundled-packages.json";
export const APPLICATION_INDEX_PATH = "packages/application-packages.v1.json";
export const TAURI_CONFIG_PATH = "apps/teamarness-desktop/src-tauri/tauri.conf.json";
// Staging stays inside the repository because tauri.conf.json paths are relative to it.
export const STAGING_ROOT = "target/teamarness-packages";
const TAURI_TO_REPOSITORY = "../../..";

export async function readBundledPackages(root = repositoryRoot) {
  const document = JSON.parse(await readFile(join(root, BUNDLED_PACKAGES_PATH), "utf8"));
  if (document.schemaVersion !== 1 || !Array.isArray(document.packages) || document.packages.length === 0) {
    throw new Error(`${BUNDLED_PACKAGES_PATH} must have schemaVersion 1 and a non-empty packages list`);
  }
  const packages = [];
  const seen = new Set();
  for (const entry of document.packages) {
    const { sourceDirectory, cargoPackage } = entry ?? {};
    const keys = Object.keys(entry ?? {}).sort().join(",");
    if (keys !== "cargoPackage,sourceDirectory" || typeof sourceDirectory !== "string" || typeof cargoPackage !== "string") {
      throw new Error(`${BUNDLED_PACKAGES_PATH} entries need exactly string sourceDirectory and cargoPackage`);
    }
    if (posix.normalize(sourceDirectory) !== sourceDirectory || !sourceDirectory.startsWith("packages/")) {
      throw new Error(`Bundled Package sourceDirectory must be a normalized packages/ path: ${sourceDirectory}`);
    }
    const manifest = JSON.parse(await readFile(join(root, sourceDirectory, "teamarness.package.json"), "utf8"));
    const { identity } = manifest;
    for (const key of [sourceDirectory, cargoPackage, identity]) {
      if (seen.has(key)) throw new Error(`Duplicate bundled Package entry: ${key}`);
      seen.add(key);
    }
    const [backend] = manifest.backends;
    if (manifest.backends.length !== 1 || backend.path !== `backend/${cargoPackage}-${backend.target}`) {
      throw new Error(`Bundled Package ${identity} must declare one Backend at backend/${cargoPackage}-<target>`);
    }
    packages.push({ sourceDirectory, cargoPackage, identity, manifest });
  }
  return packages;
}

export function applicationIndexDocument(packages) {
  return {
    schemaVersion: 1,
    packages: packages.map(({ identity }) => ({ directory: `packages/${identity}` })),
  };
}

export function tauriBundleConfiguration(packages) {
  const resources = {
    [`${TAURI_TO_REPOSITORY}/${APPLICATION_INDEX_PATH}`]: APPLICATION_INDEX_PATH,
  };
  for (const { identity } of packages) {
    resources[`${TAURI_TO_REPOSITORY}/${STAGING_ROOT}/${identity}/`] = `packages/${identity}/`;
  }
  const externalBin = packages.map(({ identity, cargoPackage }) =>
    `${TAURI_TO_REPOSITORY}/${STAGING_ROOT}/${identity}/backend/${cargoPackage}`);
  return { resources, externalBin };
}

export function serializeJson(document) {
  return `${JSON.stringify(document, null, 2)}\n`;
}

export async function checkBundledPackages(root = repositoryRoot) {
  const packages = await readBundledPackages(root);
  const problems = [];
  const config = JSON.parse(await readFile(join(root, TAURI_CONFIG_PATH), "utf8"));
  const expected = tauriBundleConfiguration(packages);
  if (!isDeepStrictEqual(config.bundle?.resources, expected.resources)) {
    problems.push(`${TAURI_CONFIG_PATH} bundle.resources does not match ${BUNDLED_PACKAGES_PATH}`);
  }
  if (!isDeepStrictEqual(config.bundle?.externalBin, expected.externalBin)) {
    problems.push(`${TAURI_CONFIG_PATH} bundle.externalBin does not match ${BUNDLED_PACKAGES_PATH}`);
  }
  const index = await readFile(join(root, APPLICATION_INDEX_PATH), "utf8").catch(() => "");
  if (index !== serializeJson(applicationIndexDocument(packages))) {
    problems.push(`${APPLICATION_INDEX_PATH} does not match ${BUNDLED_PACKAGES_PATH}`);
  }
  return problems;
}

export async function writeBundledPackages(root = repositoryRoot) {
  const packages = await readBundledPackages(root);
  const configPath = join(root, TAURI_CONFIG_PATH);
  const text = await readFile(configPath, "utf8");
  const { resources, externalBin } = tauriBundleConfiguration(packages);
  const updated = replaceOnce(
    replaceOnce(text, /"resources": \{[^{}]*\}/, `"resources": ${indentBlock(resources)}`),
    /"externalBin": \[[^[\]]*\]/,
    `"externalBin": ${indentBlock(externalBin)}`,
  );
  if (updated !== text) await replaceFile(configPath, updated);
  // The application builder stays the only writer of the index; --check still compares it.
  return (await checkBundledPackages(root)).filter((problem) => !problem.startsWith(APPLICATION_INDEX_PATH));
}

export async function replaceFile(path, contents) {
  const temporaryPath = `${path}.tmp-${process.pid}`;
  await writeFile(temporaryPath, contents, { mode: 0o600 });
  await rename(temporaryPath, path);
}

function replaceOnce(text, pattern, replacement) {
  if (!pattern.test(text)) throw new Error(`${TAURI_CONFIG_PATH} has no ${pattern} block to regenerate`);
  return text.replace(pattern, () => replacement);
}

// tauri.conf.json nests bundle.resources and bundle.externalBin at a four-space indent.
function indentBlock(value) {
  return JSON.stringify(value, null, 2).split("\n").join("\n    ");
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  const mode = process.argv[2];
  const run = mode === "--write" ? writeBundledPackages : mode === "--check" ? checkBundledPackages : undefined;
  if (!run) {
    console.error("Usage: node scripts/bundled-packages.mjs --write|--check");
    process.exitCode = 2;
  } else {
    run().then((problems = []) => {
      if (problems.length > 0) {
        console.error(`${problems.join("\n")}\nRun \`node scripts/bundled-packages.mjs --write\` and \`npm run build:application-packages\` to regenerate.`);
        process.exitCode = 1;
      } else {
        console.log(mode === "--write"
          ? `Tauri bundle configuration regenerated from ${BUNDLED_PACKAGES_PATH}.`
          : `Bundled Package configuration matches ${BUNDLED_PACKAGES_PATH}.`);
      }
    }).catch((error) => {
      console.error(error.message);
      process.exitCode = 1;
    });
  }
}
