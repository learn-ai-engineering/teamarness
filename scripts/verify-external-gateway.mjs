import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { vendorReport } from "./create-package.mjs";
import { verifyPackageDirectory } from "./package-directory.mjs";
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
export function requireExecutedTests(output) {
  const match = /test result: ok\. ([1-9][0-9]*) passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;/.exec(output);
  if (!match) throw new Error("qualification requires executed, passing external tests (no ignored or filtered cases)");
  return Number(match[1]);
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
  const evidence = { hostRevision, hostDirty: git(host, "status", "--porcelain") !== "", gatewayRevision, gatewayClean: true, locks, sdkRevision: provenance.sdkRevision, sdkRevisionDirty: false, vendorDigests: provenance.vendorDigests, artifacts };
  console.log(`gateway build provenance: ${JSON.stringify(evidence)}`);
  const output = run(host, "cargo", ["test", "-p", "teamarness-system-tests", "--test", "external_gateway_flow", "--locked", "--", "--ignored"], {
    capture: true, env: { ...process.env, TEAMARNESS_EXTERNAL_GATEWAY_PACKAGE_DIR: directory },
  });
  process.stdout.write(output);
  evidence.executedTests = requireExecutedTests(output);
  if (JSON.stringify(await artifactHashes(directory, files)) !== JSON.stringify(artifacts)) throw new Error("gateway artifacts changed during live qualification");
  if (git(project, "status", "--porcelain") !== "" || git(project, "rev-parse", "HEAD") !== gatewayRevision) throw new Error("gateway source changed during real-Host tests");
  console.log(`gateway qualification passed: ${JSON.stringify(evidence)}`);
  return evidence;
}
if (process.argv[1] && resolve(process.argv[1]) === script) {
  try { await qualify(parseArgs(process.argv.slice(2))); }
  catch (error) { console.error(error.message); process.exitCode = 1; }
}
