import { chmod, copyFile, mkdir, readFile, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repository = resolve(here, "../../..");
const profile = process.argv[2];
if (!profile || !/^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/.test(profile)) {
  throw new Error("Provide one safe development profile name");
}
const data = join(repository, ".teamarness-dev", profile);
const packageDirectory = join(data, "phase-5-webkit-package");
const backendSource = join(repository, "contracts/package-process/v1/fixtures/node-package-process.mjs");
const backend = await readFile(backendSource);
const entrypoint = await readFile(join(here, "dist/index.js"));
const asset = await readFile(join(here, "dist/probe.svg"));
const contract = Buffer.from(JSON.stringify({ schemaVersion: 1, $defs: {}, requests: {}, subscriptions: {} }));

await mkdir(join(packageDirectory, "backend"), { recursive: true, mode: 0o700 });
await mkdir(join(packageDirectory, "ui"), { recursive: true, mode: 0o700 });
await mkdir(join(data, "packages"), { recursive: true, mode: 0o700 });
await copyFile(backendSource, join(packageDirectory, "backend/package.mjs"));
await chmod(join(packageDirectory, "backend/package.mjs"), 0o700);
await writeFile(join(packageDirectory, "ui/index.js"), entrypoint, { mode: 0o600 });
await writeFile(join(packageDirectory, "ui/probe.svg"), asset, { mode: 0o600 });
await writeFile(join(packageDirectory, "ui/contract.json"), contract, { mode: 0o600 });
await writeFile(join(packageDirectory, "teamarness.package.json"), JSON.stringify({
  schemaVersion: 1,
  identity: "com.example.webkit-probe",
  version: "1.0.0",
  presentation: { displayName: "WebKit protocol probe" },
  protocol: { major: 1, minMinor: 0, maxMinor: 0 },
  backends: [{
    target: "aarch64-apple-darwin",
    path: "backend/package.mjs",
    size: backend.length,
    sha256: digest(backend),
  }],
  ui: {
    entrypoint: { path: "ui/index.js", size: entrypoint.length, sha256: digest(entrypoint) },
    assets: [{ path: "ui/probe.svg", mediaType: "image/svg+xml", size: asset.length, sha256: digest(asset) }],
    contract: { path: "ui/contract.json", sha256: digest(contract) },
  },
  capabilities: [{ identity: "teamarness.workspace-context", major: 1, minMinor: 0, maxMinor: 0 }],
}), { mode: 0o600 });
const registration = join(data, "packages/registrations.v1.json");
await writeFile(registration, JSON.stringify({
  schemaVersion: 1,
  registrations: [{
    directory: packageDirectory,
    expectedIdentity: "com.example.webkit-probe",
    trust: { kind: "trusted_native_code_v1", acceptedRisks: ["filesystem", "network", "process"] },
    overrideApplication: false,
  }],
}), { mode: 0o600 });
await chmod(data, 0o700);
await chmod(join(data, "packages"), 0o700);
await chmod(registration, 0o600);
console.log("Phase 5 WebKit probe prepared without printing private paths.");

function digest(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}
