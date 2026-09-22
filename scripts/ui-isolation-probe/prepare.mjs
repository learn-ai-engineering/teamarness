import { chmod, copyFile, mkdir, readFile, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repository = resolve(here, "../..");

export const UI_ISOLATION_PROBE_IDENTITY = "dev.teamarness.ui-isolation-probe";
export const UI_ISOLATION_PROBE_DISPLAY_NAME = "UI isolation probe";

/**
 * Writes a local Package Directory whose UI is the isolation probe, and
 * registers it as trusted native code in the supplied data directory.
 */
export async function prepareUiIsolationProbe(dataDirectory) {
  const packageDirectory = join(dataDirectory, "ui-isolation-probe-package");
  const backendSource = join(repository, "contracts/package-process/v1/fixtures/node-package-process.mjs");
  const backend = await readFile(backendSource);
  const entrypoint = await readFile(join(here, "ui/index.js"));
  const contract = Buffer.from(JSON.stringify({ schemaVersion: 1, $defs: {}, requests: {}, subscriptions: {} }));

  await mkdir(join(packageDirectory, "backend"), { recursive: true, mode: 0o700 });
  await mkdir(join(packageDirectory, "ui"), { recursive: true, mode: 0o700 });
  await mkdir(join(dataDirectory, "packages"), { recursive: true, mode: 0o700 });
  await copyFile(backendSource, join(packageDirectory, "backend/package.mjs"));
  await chmod(join(packageDirectory, "backend/package.mjs"), 0o700);
  await writeFile(join(packageDirectory, "ui/index.js"), entrypoint, { mode: 0o600 });
  await writeFile(join(packageDirectory, "ui/contract.json"), contract, { mode: 0o600 });
  await writeFile(join(packageDirectory, "teamarness.package.json"), JSON.stringify({
    schemaVersion: 1,
    identity: UI_ISOLATION_PROBE_IDENTITY,
    version: "1.0.0",
    presentation: { displayName: UI_ISOLATION_PROBE_DISPLAY_NAME },
    protocol: { major: 1, minMinor: 0, maxMinor: 0 },
    backends: [{
      target: "aarch64-apple-darwin",
      path: "backend/package.mjs",
      size: backend.length,
      sha256: digest(backend),
    }],
    ui: {
      entrypoint: { path: "ui/index.js", size: entrypoint.length, sha256: digest(entrypoint) },
      assets: [],
      contract: { path: "ui/contract.json", sha256: digest(contract) },
    },
    capabilities: [{ identity: "teamarness.workspace-context", major: 1, minMinor: 0, maxMinor: 0 }],
  }), { mode: 0o600 });

  const registration = join(dataDirectory, "packages/registrations.v1.json");
  await writeFile(registration, JSON.stringify({
    schemaVersion: 1,
    registrations: [{
      directory: packageDirectory,
      expectedIdentity: UI_ISOLATION_PROBE_IDENTITY,
      trust: { kind: "trusted_native_code_v1", acceptedRisks: ["filesystem", "network", "process"] },
      overrideApplication: false,
    }],
  }), { mode: 0o600 });
  await chmod(dataDirectory, 0o700);
  await chmod(join(dataDirectory, "packages"), 0o700);
  await chmod(registration, 0o600);

  return { identity: UI_ISOLATION_PROBE_IDENTITY, displayName: UI_ISOLATION_PROBE_DISPLAY_NAME };
}

function digest(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}
