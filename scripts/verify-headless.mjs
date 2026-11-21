import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { repositoryRoot } from "./bundled-packages.mjs";
import { cargoFixtureEnvironment } from "./cargo-fixture.mjs";

export const targets = {
  headless_mesh: ["sync_only_two_headless_and_desktop_backends_cover_two_workspaces_offline_restart_and_late_join"],
  headless_personal_flow: ["canonical_runbook_fixture_settings_lifecycle_and_retention", "independent_personal_install_remove_preserves_team_generations_and_document_bodies"],
};
export const fixtureNames = ["headless-cli-under-test", "headless-personal-fixture", "person-port-consumer-under-test", "documents-backend-under-test", "backlog-backend-under-test", "conversations-backend-under-test"];
export function qualificationCommands() {
  // Use the integration-test feature graph for both fixture preparation and execution.
  // A plain `cargo build --bins` uses a different dev-dependency graph and replaces these paths.
  return [["cargo", ["test", "-p", "teamarness-system-tests", "--test", "headless_mesh", "--test", "headless_personal_flow", "--no-run", "--locked", "--message-format=json"]],
    ...Object.keys(targets).map(target => ["cargo", ["test", "-p", "teamarness-system-tests", "--test", target, "--locked", "--", "--nocapture"]])];
}
export function validateNative(platform, arch, host) {
  const expected = platform === "darwin" && arch === "arm64" ? "aarch64-apple-darwin" : platform === "linux" && arch === "x64" ? "x86_64-unknown-linux-gnu" : null;
  if (!expected || host !== expected) throw new Error(`unsupported native headless qualification: ${platform}/${arch}/${host}`);
  return expected;
}
export function requireHeadlessTests(output, expected) {
  const summaries = [...output.matchAll(/test result: [^\n]+/g)];
  const summary = summaries.length === 1 && /^test result: ok\. ([1-9][0-9]*) passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;/.exec(summaries[0][0]);
  if (!summary) throw new Error("qualification requires executed passing cases, without ignored or filtered tests");
  if (Number(summary[1]) !== expected.length || expected.some(name => !output.split("\n").includes(`test ${name} ... ok`))) throw new Error("qualification must execute every named headless case");
  return Number(summary[1]);
}
export function shellBlocks(markdown) { return [...markdown.matchAll(/```sh\n([\s\S]*?)```/g)].map(match => match[1]); }
const hash = bytes => createHash("sha256").update(bytes).digest("hex");
function run(command, args, { printOutput = true } = {}) {
  console.log(`> ${command} ${args.join(" ")}`);
  const result = spawnSync(command, args, { cwd: repositoryRoot, encoding: "utf8", maxBuffer: 128 * 1024 * 1024, env: { ...cargoFixtureEnvironment(), NO_COLOR: "1" } });
  if (printOutput || result.status !== 0) process.stdout.write(result.stdout ?? "");
  process.stderr.write(result.stderr ?? "");
  console.log(`command exit: ${result.status ?? "not-started"}`);
  if (result.error || result.status !== 0) throw new Error(`required headless command failed: ${command} (${result.status ?? result.error?.message})`);
  return result.stdout;
}
async function sourceManifest() {
  const files = run("git", ["ls-files", "--cached", "--others", "--exclude-standard", "-z"], { printOutput: false }).split("\0").filter(Boolean).sort();
  return Object.fromEntries(await Promise.all(files.map(async file => [file, hash(await readFile(join(repositoryRoot, file)))])));
}
export async function verifyHeadless() {
  for (const key of Object.keys(process.env)) if (/^TEAMARNESS_(?:AGENT_|EXTERNAL_GATEWAY_)/.test(key)) throw new Error(`external Package configuration is forbidden: ${key}`);
  const host = /^host: (.+)$/m.exec(run("rustc", ["-vV"]))?.[1];
  const target = validateNative(process.platform, process.arch, host);
  const revision = run("git", ["rev-parse", "HEAD"]).trim();
  const source = await sourceManifest();
  const fixtures = {};
  const commands = qualificationCommands();
  const built = run(...commands[0]);
  for (const line of built.split("\n")) {
    let artifact; try { artifact = JSON.parse(line); } catch { continue; }
    if (artifact.reason === "compiler-artifact" && artifact.executable && fixtureNames.includes(artifact.target.name)) fixtures[artifact.target.name] = { path: artifact.executable, sha256: hash(await readFile(artifact.executable)) };
  }
  if (fixtureNames.some(name => !fixtures[name])) throw new Error("Cargo did not report every source-current fixture executable");
  console.log(`source-current fixture provenance: ${JSON.stringify(fixtures)}`);
  const executed = {};
  for (const command of commands.slice(1)) {
    const target = command[1][command[1].indexOf("--test") + 1];
    executed[target] = requireHeadlessTests(run(...command), targets[target]);
  }
  if (JSON.stringify(source) !== JSON.stringify(await sourceManifest())) throw new Error("source changed during headless qualification");
  for (const [name, fixture] of Object.entries(fixtures)) if (hash(await readFile(fixture.path)) !== fixture.sha256) throw new Error(`fixture executable changed during qualification: ${name}`);
  const evidence = { revision, target, node: process.version, sourceManifestSha256: hash(JSON.stringify(source)), locks: { cargo: source["Cargo.lock"], npm: source["package-lock.json"] }, fixtures, executed, testNames: targets, qualification: "native loopback automation only; production Iroh, Ubuntu 26.04 and human M1–M5 are separate observations" };
  console.log(`headless qualification passed: ${JSON.stringify(evidence)}`);
  return evidence;
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    if (process.argv.length !== 2) throw new Error("usage: node scripts/verify-headless.mjs (no skips, prebuilt paths, or target overrides)");
    await verifyHeadless();
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
