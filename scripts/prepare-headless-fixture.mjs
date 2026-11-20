// Test-only Personal Package for the canonical manual runbook. Never application resources.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { repositoryRoot } from "./bundled-packages.mjs";
import { nativeBackendTarget } from "./package-directory.mjs";
import { fileURLToPath } from "node:url";
import { qualificationCommands } from "./verify-headless.mjs";
import { cargoFixtureEnvironment } from "./cargo-fixture.mjs";

export async function prepareFixture(directory) {
  // A new explicit directory only. Never overwrite an existing Package or profile.
  await mkdir(directory, { mode: 0o700 });
  const [command, args] = qualificationCommands()[0];
  const env = cargoFixtureEnvironment();
  const build = spawnSync(command, args, { cwd: repositoryRoot, env, encoding: "utf8", maxBuffer: 16 * 1024 * 1024 });
  if (build.error || build.status !== 0) throw new Error(`source fixture build failed (${build.status ?? build.error}):\nstdout:\n${build.stdout ?? ""}\nstderr:\n${build.stderr ?? ""}`);
  const binary = build.stdout.split("\n").map(line => { try { return JSON.parse(line); } catch { return null; } }).find(row => row?.target?.name === "headless-personal-fixture" && row.executable)?.executable;
  if (!binary) throw new Error("Cargo did not report the source-current fixture");
  return writeFixture(directory, await readFile(binary));
}

// The integration harness supplies only its compile-time CARGO_BIN_EXE fixture. Rebuilding
// here can temporarily unlink a different caller-owned executable while another test runs.
// This embedding entry is not a manual CLI option; ordinary preparation still source-builds.
export async function prepareCargoFixture(directory, executable) {
  await mkdir(directory, { mode: 0o700 });
  return writeFixture(directory, await readFile(executable));
}

async function writeFixture(directory, bytes) {
  const contract = await readFile(join(repositoryRoot, "tests/system/fixtures/headless-personal-ui-contract.json"));
  const ui = Buffer.from("throw new Error('UI_MUST_NOT_EXECUTE');");
  const hash = bytes => createHash("sha256").update(bytes).digest("hex");
  await mkdir(join(directory, "backend"), { mode: 0o700 });
  await mkdir(join(directory, "ui"), { mode: 0o700 });
  await writeFile(join(directory, "backend/fixture"), bytes, { mode: 0o700 });
  await writeFile(join(directory, "ui/index.js"), ui, { mode: 0o600 });
  await writeFile(join(directory, "ui/contract.json"), contract, { mode: 0o600 });
  const manifest = { schemaVersion: 1, identity: "org.unfamiliar.headless-proof", version: "1.0.0", presentation: { displayName: "Independent proof" }, protocol: { major: 1, minMinor: 1, maxMinor: 1 }, activation: "eager",
    backends: [{ target: nativeBackendTarget(), path: "backend/fixture", size: bytes.length, sha256: hash(bytes) }],
    ui: { entrypoint: { path: "ui/index.js", size: ui.length, sha256: hash(ui) }, assets: [], contract: { path: "ui/contract.json", sha256: hash(contract) } },
    capabilities: [{ identity: "teamarness.agent-binding", major: 1, minMinor: 0, maxMinor: 0 }] };
  await writeFile(join(directory, "teamarness.package.json"), `${JSON.stringify(manifest, null, 2)}\n`, { mode: 0o600 });
  console.log(`Prepared test fixture only (not product qualification): ${JSON.stringify({ directory, target: nativeBackendTarget(), backendSha256: hash(bytes), contractSha256: hash(contract) })}`);
  return manifest;
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    if (process.argv.length !== 3 || !process.argv[2] || process.argv[2].startsWith("--")) throw new Error("usage: node scripts/prepare-headless-fixture.mjs <new-directory>");
    await prepareFixture(resolve(process.argv[2]));
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
