import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmod, mkdtemp, readFile, realpath, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { vendorReport } from "./create-package.mjs";
import { stagePackageDirectory, verifyPackageDirectory } from "./package-directory.mjs";
import { VENDOR_TREES, vendorTreeDigest } from "./vendor-digest.mjs";

const script = fileURLToPath(import.meta.url);
const host = resolve(dirname(script), "..");
const identity = "dev.teamarness.external-gateway";
export function parseArgs(args) {
  if (args.length !== 2 || args[0] !== "--project" || !args[1] || args[1].startsWith("--")) {
    throw new Error("usage: node scripts/verify-external-gateway.mjs --project <standalone source root>");
  }
  return resolve(args[1]);
}
export function requireExecutedTests(output, expected) {
  const summaries = [...output.matchAll(/test result: [^\n]+/g)];
  const match = summaries.length === 1 && /^test result: ok\. ([1-9][0-9]*) passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;/.exec(summaries[0][0]);
  if (!match) throw new Error("qualification requires executed, passing external tests (no ignored or filtered cases)");
  const count = Number(match[1]);
  if (expected && (count !== expected.length || expected.some(name => !output.split("\n").includes(`test ${name} ... ok`)))) {
    throw new Error("qualification must execute every discovered external case");
  }
  return count;
}
export function validateProvenance(provenance, report) {
  if (provenance.sdkRevisionDirty !== false) throw new Error("SDK snapshot must be clean");
  if (!/^[a-f0-9]{40}$/.test(provenance.sdkRevision)) throw new Error("invalid SDK revision");
  for (const tree of VENDOR_TREES) {
    if (!/^sha256:[a-f0-9]{64}$/.test(provenance.vendorDigests?.[tree]) || report.trees?.[tree] !== "same") {
      throw new Error(`canonical vendor mismatch: ${tree}`);
    }
  }
}
function run(cwd, command, args, { capture = false, env = process.env } = {}) {
  console.log(`qualification: ${command} ${args.join(" ")}`);
  const result = spawnSync(command, args, { cwd, env, encoding: "utf8", stdio: capture ? "pipe" : "inherit", maxBuffer: 64 * 1024 * 1024 });
  if (result.error || result.status !== 0) {
    if (capture) process.stderr.write(`${result.stdout ?? ""}${result.stderr ?? ""}`);
    throw new Error(`required command failed: ${command} (exit ${result.status ?? "unavailable"})`);
  }
  return result.stdout ?? "";
}
function git(cwd, ...args) { return run(cwd, "git", args, { capture: true }).trim(); }
const hash = bytes => createHash("sha256").update(bytes).digest("hex");
async function artifactHashes(directory, files) {
  return Object.fromEntries(await Promise.all(files.map(async file => [file, hash(await readFile(join(directory, file)))])));
}
export async function qualify(project) {
  if (git(project, "status", "--porcelain") !== "") throw new Error("gateway source must be clean before build");
  const gatewayRevision = git(project, "rev-parse", "HEAD");
  const hostRevision = git(host, "rev-parse", "HEAD");
  const provenance = JSON.parse(await readFile(join(project, "teamarness-authoring.json"), "utf8"));
  const report = await vendorReport(project);
  validateProvenance(provenance, report);
  git(host, "merge-base", "--is-ancestor", provenance.sdkRevision, "HEAD");
  for (const tree of VENDOR_TREES) {
    if (await vendorTreeDigest(join(project, "vendor/teamarness", tree)) !== provenance.vendorDigests[tree]) throw new Error(`recorded vendor digest mismatch: ${tree}`);
  }
  // No prebuilt dist can substitute for these complete locked source checks.
  run(project, "npm", ["ci"]);
  for (const name of ["validate", "check:vendor", "typecheck", "lint"]) run(project, "npm", ["run", name]);
  run(project, "npm", ["test"]);
  run(project, "npm", ["run", "build"]);
  git(project, "diff", "--check");
  if (git(project, "status", "--porcelain") !== "" || git(project, "rev-parse", "HEAD") !== gatewayRevision) throw new Error("gateway source changed during qualification");
  const directory = join(project, "dist", identity);
  const validated = await verifyPackageDirectory(directory);
  if (validated.identity !== identity) throw new Error("wrong external Package identity");
  const files = ["teamarness.package.json", ...validated.files];
  const artifacts = await artifactHashes(directory, files);
  const locks = await artifactHashes(project, ["Cargo.lock", "package-lock.json"]);
  // Production activation refreshes OS-home output. NEVER activate these bytes here.
  // Offline modes are final-artifact evidence; injected-home live evidence is separate.
  const productionManifest = JSON.parse(await readFile(join(directory, "teamarness.package.json"), "utf8"));
  const productionExecutable = join(directory, productionManifest.backends[0].path);
  for (const option of ["--help", "--version", "--licenses"]) {
    const result = run(host, productionExecutable, ["--tmx-cli", option], { capture: true });
    if (!result.trim()) throw new Error(`empty production offline output: ${option}`);
  }
  run(project, "cargo", ["build", "--locked", "--release", "-p", "teamarness-generated-package-backend", "--example", "backend-fixture", "--example", "cli-fixture"]);
  const scratch = await realpath(await mkdtemp(join(tmpdir(), "teamarness-gateway-qualification-")));
  await chmod(scratch, 0o700);
  try {
    const fixtureDirectory = join(scratch, "package");
    await stagePackageDirectory({
      manifestPath: join(directory, "teamarness.package.json"),
      contractPath: join(directory, productionManifest.ui.contract.path),
      uiEntrypointPath: join(directory, productionManifest.ui.entrypoint.path),
      backendPath: join(project, "target/release/examples/backend-fixture"),
      outputDirectory: fixtureDirectory,
    });
    const fixture = await verifyPackageDirectory(fixtureDirectory);
    const fixtureArtifacts = await artifactHashes(fixtureDirectory, ["teamarness.package.json", ...fixture.files]);
    const cliFixture = join(project, "target/release/examples/cli-fixture");
    const drivers = await artifactHashes(project, ["target/release/examples/backend-fixture", "target/release/examples/cli-fixture"]);
    const sourceHashes = async root => artifactHashes(root, git(root, "ls-files").split("\n").filter(Boolean));
    const sources = { host: hash(JSON.stringify(await sourceHashes(host))), gateway: hash(JSON.stringify(await sourceHashes(project))) };
    const evidence = {
      hostRevision, hostDirty: git(host, "status", "--porcelain") !== "", hostTrackedDirty: git(host, "status", "--porcelain", "--untracked-files=no") !== "",
      gatewayRevision, gatewayClean: true, locks, hostLocks: await artifactHashes(host, ["Cargo.lock", "package-lock.json"]),
      sourceDigests: sources, packageVersion: productionManifest.version,
      tools: { node: process.version, rustc: run(host, "rustc", ["--version"], { capture: true }).trim(), cargo: run(host, "cargo", ["--version"], { capture: true }).trim(), npm: run(host, "npm", ["--version"], { capture: true }).trim() },
      sdkRevision: provenance.sdkRevision, sdkRevisionDirty: false, vendorDigests: provenance.vendorDigests,
      productionArtifacts: artifacts, fixtureArtifacts, drivers,
      liveRuntime: "injected-home Backend and CLI; production-home installed/native M1-M5 pending",
    };
    console.log(`gateway build provenance: ${JSON.stringify(evidence)}`);
    const args = ["test", "-p", "teamarness-system-tests", "--test", "external_gateway_flow", "--locked"];
    const listing = run(host, "cargo", [...args, "--", "--list"], { capture: true });
    const expected = listing.split("\n").filter(line => line.endsWith(": test")).map(line => line.slice(0, -6));
    if (!expected.length) throw new Error("no external tests discovered");
    const output = run(host, "cargo", [...args, "--", "--include-ignored"], {
      capture: true, env: { ...process.env, TEAMARNESS_EXTERNAL_GATEWAY_PACKAGE_DIR: fixtureDirectory,
        TEAMARNESS_EXTERNAL_GATEWAY_PRODUCTION_DIR: directory, TEAMARNESS_EXTERNAL_GATEWAY_CLI_FIXTURE: cliFixture,
        TEAMARNESS_FIXTURE_ROOT: scratch, TEAMARNESS_EXTERNAL_GATEWAY_BACKEND_FIXTURE_HASH: drivers["target/release/examples/backend-fixture"] },
    });
    process.stdout.write(output);
    evidence.executedTests = requireExecutedTests(output, expected);
    evidence.testNames = expected;
    if (JSON.stringify(await artifactHashes(directory, files)) !== JSON.stringify(artifacts)) throw new Error("production artifacts changed during live qualification");
    if (JSON.stringify(await artifactHashes(fixtureDirectory, ["teamarness.package.json", ...fixture.files])) !== JSON.stringify(fixtureArtifacts)) throw new Error("fixture artifacts changed during live qualification");
    if (JSON.stringify(await artifactHashes(project, Object.keys(drivers))) !== JSON.stringify(drivers)) throw new Error("fixture drivers changed during live qualification");
    if (git(project, "status", "--porcelain") !== "" || git(project, "rev-parse", "HEAD") !== gatewayRevision) throw new Error("gateway source changed during real-Host tests");
    if (git(host, "rev-parse", "HEAD") !== hostRevision || hash(JSON.stringify(await sourceHashes(host))) !== sources.host) throw new Error("host source changed during real-Host tests");
    console.log(`gateway qualification passed: ${JSON.stringify(evidence)}`);
    return evidence;
  } finally { await rm(scratch, { recursive: true, force: true }); }
}
if (process.argv[1] && resolve(process.argv[1]) === script) {
  try { await qualify(parseArgs(process.argv.slice(2))); }
  catch (error) { console.error(error.message); process.exitCode = 1; }
}
