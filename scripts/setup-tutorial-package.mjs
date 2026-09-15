import { chmod, lstat, mkdir, readFile, realpath, rename, writeFile } from "node:fs/promises";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { buildTutorialPackage } from "./build-tutorial-package.mjs";

const SCRIPT_PATH = fileURLToPath(import.meta.url);
export const REPOSITORY_ROOT = resolve(dirname(SCRIPT_PATH), "..");
const PROFILE_PATTERN = /^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/;
const IDENTITY_PATTERN = /^[a-z][a-z0-9-]{0,62}(?:\.[a-z][a-z0-9-]{0,62})+$/;
const TUTORIAL_IDENTITY = "dev.teamarness.tutorial";
const TUTORIAL_TRUST = {
  kind: "trusted_native_code_v1",
  acceptedRisks: ["filesystem", "network", "process"],
};

export async function setupTutorialPackage({
  profileName,
  repositoryRoot = REPOSITORY_ROOT,
  buildPackage = buildTutorialPackage,
} = {}) {
  if (typeof profileName !== "string" || !PROFILE_PATTERN.test(profileName)) {
    throw new Error("Profile names must be 1-64 letters, numbers, underscores, or hyphens, starting with a letter or number.");
  }

  const root = await realpath(repositoryRoot);
  const builtDirectory = await buildPackage({ repositoryRoot: root });
  if (typeof builtDirectory !== "string" || !isAbsolute(builtDirectory)) {
    throw new Error("Tutorial Package build returned a non-absolute directory");
  }
  const packageDirectory = await realpath(builtDirectory);

  const developmentDirectory = join(root, ".teamarness-dev");
  const profileDirectory = join(developmentDirectory, profileName);
  const packagesDirectory = join(profileDirectory, "packages");
  await ensurePrivateDirectory(developmentDirectory);
  await ensurePrivateDirectory(profileDirectory);
  await ensurePrivateDirectory(packagesDirectory);

  const registrationPath = join(packagesDirectory, "registrations.v1.json");
  const document = await readRegistrations(registrationPath);
  const tutorialRegistration = {
    directory: packageDirectory,
    expectedIdentity: TUTORIAL_IDENTITY,
    trust: TUTORIAL_TRUST,
  };
  const existingTutorials = document.registrations.filter(
    (registration) => registration.expectedIdentity === TUTORIAL_IDENTITY,
  );
  if (existingTutorials.length > 1) {
    throw new Error("registrations.v1.json contains duplicate Tutorial registrations");
  }
  const registrations = document.registrations.filter(
    (registration) => registration.expectedIdentity !== TUTORIAL_IDENTITY,
  );
  registrations.push(tutorialRegistration);

  await writePrivateJson(registrationPath, { schemaVersion: 1, registrations });
  return registrationPath;
}

async function ensurePrivateDirectory(path) {
  try {
    const details = await lstat(path);
    if (!details.isDirectory() || details.isSymbolicLink()) {
      throw new Error(`${path} must be a directory, not a symbolic link`);
    }
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
    await mkdir(path, { recursive: true, mode: 0o700 });
  }
  if (process.platform !== "win32") await chmod(path, 0o700);
}

async function readRegistrations(path) {
  try {
    const details = await lstat(path);
    if (!details.isFile() || details.isSymbolicLink()) {
      throw new Error("registrations.v1.json must be a regular file");
    }
    const document = JSON.parse(await readFile(path, "utf8"));
    if (!isRegistrationDocument(document)) {
      throw new Error("registrations.v1.json is not a valid registrations document");
    }
    return document;
  } catch (error) {
    if (error?.code === "ENOENT") return { schemaVersion: 1, registrations: [] };
    if (error instanceof SyntaxError) {
      throw new Error("registrations.v1.json is not a valid registrations document");
    }
    throw error;
  }
}

function isRegistrationDocument(document) {
  return document
    && typeof document === "object"
    && document.schemaVersion === 1
    && Array.isArray(document.registrations)
    && document.registrations.every(isRegistration);
}

function isRegistration(registration) {
  if (!registration || typeof registration !== "object" || Array.isArray(registration)) return false;
  const keys = Object.keys(registration).sort();
  const expectedKeys = registration.overrideApplication === undefined
    ? ["directory", "expectedIdentity", "trust"]
    : ["directory", "expectedIdentity", "overrideApplication", "trust"];
  return keys.length === expectedKeys.length
    && keys.every((key, index) => key === expectedKeys[index])
    && typeof registration.directory === "string"
    && isAbsolute(registration.directory)
    && typeof registration.expectedIdentity === "string"
    && IDENTITY_PATTERN.test(registration.expectedIdentity)
    && registration.trust
    && typeof registration.trust === "object"
    && Object.keys(registration.trust).length === 2
    && Object.keys(registration.trust).every((key) => ["kind", "acceptedRisks"].includes(key))
    && registration.trust.kind === "trusted_native_code_v1"
    && Array.isArray(registration.trust.acceptedRisks)
    && registration.trust.acceptedRisks.length === 3
    && ["filesystem", "network", "process"].every((risk) => registration.trust.acceptedRisks.includes(risk))
    && (registration.overrideApplication === undefined || typeof registration.overrideApplication === "boolean");
}

async function writePrivateJson(path, document) {
  const temporaryPath = `${path}.tmp-${process.pid}`;
  await writeFile(temporaryPath, `${JSON.stringify(document, null, 2)}\n`, { mode: 0o600 });
  if (process.platform !== "win32") await chmod(temporaryPath, 0o600);
  await rename(temporaryPath, path);
  if (process.platform !== "win32") await chmod(path, 0o600);
}

if (process.argv[1] && resolve(process.argv[1]) === SCRIPT_PATH) {
  const [profileName, ...extra] = process.argv.slice(2);
  if (extra.length > 0) {
    console.error("Usage: npm run setup:tutorial -- <fresh-profile-name>");
    process.exitCode = 1;
  } else {
    setupTutorialPackage({ profileName }).then(() => {
      console.log("Tutorial Package registered for the named development profile. Restart Teamarness to admit it.");
    }).catch((error) => {
      console.error(error.message);
      process.exitCode = 1;
    });
  }
}
