import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createHash } from "node:crypto";
import { prepareFixture } from "./prepare-headless-fixture.mjs";
import test from "node:test";
import { qualificationCommands, requireHeadlessTests, validateNative, shellBlocks } from "./verify-headless.mjs";

test("native headless qualification always source-builds every fixture then executes both named targets", () => {
  const commands = qualificationCommands();
  assert.deepEqual(commands[0], ["cargo", ["test", "-p", "teamarness-system-tests", "--test", "headless_mesh", "--test", "headless_personal_flow", "--no-run", "--locked", "--message-format=json"]]);
  assert.deepEqual(commands.slice(1).map(([, args]) => args[args.indexOf("--test") + 1]), ["headless_mesh", "headless_personal_flow"]);
  for (const [, args] of commands.slice(1)) {
    assert.ok(args.includes("--locked"));
    assert.ok(!args.some(arg => /ignored|skip|filter/.test(arg)));
  }
  assert.equal(validateNative("darwin", "arm64", "aarch64-apple-darwin"), "aarch64-apple-darwin");
  assert.equal(validateNative("linux", "x64", "x86_64-unknown-linux-gnu"), "x86_64-unknown-linux-gnu");
  for (const args of [["linux", "x64", "aarch64-apple-darwin"], ["darwin", "x64", "x86_64-apple-darwin"], ["win32", "x64", "x86_64-unknown-linux-gnu"]]) assert.throws(() => validateNative(...args), /native/);
});
test("manual fixture preparation builds current native bytes, shares the tested contract, and never overwrites", async () => {
  const root = await mkdtemp(join(tmpdir(), "headless-manual-fixture-"));
  try {
    const directory = join(root, "new-package");
    const hash = bytes => createHash("sha256").update(bytes).digest("hex");
    const [command, args] = qualificationCommands()[0];
    const current = spawnSync(command, args, { encoding: "utf8", maxBuffer: 16 * 1024 * 1024 });
    assert.equal(current.status, 0, current.stderr);
    const paths = current.stdout.split("\n").map(line => { try { return JSON.parse(line); } catch { return null; } }).filter(row => row?.reason === "compiler-artifact" && row.executable).map(row => row.executable);
    assert.ok(paths.length >= 6, "source-current integration fixtures reported");
    const before = await Promise.all(paths.map(async path => hash(await readFile(path))));
    // Conflicting ambient rustup selection and injected Cargo-test metadata must not
    // select a different compiler or replace any live Cargo-owned fixture.
    const prepared = spawnSync(process.execPath, ["scripts/prepare-headless-fixture.mjs", directory], { encoding: "utf8", env: { ...process.env, RUSTUP_TOOLCHAIN: "stable", RUSTC: "rustc", CARGO_MANIFEST_DIR: join(process.cwd(), "tests/system"), CARGO_PKG_NAME: "teamarness-system-tests", CARGO_PKG_VERSION_MAJOR: "0", CARGO_PKG_VERSION_MINOR: "1", CARGO_PKG_VERSION_PATCH: "0", CARGO_PKG_VERSION_PRE: "" } });
    assert.equal(prepared.status, 0, prepared.stderr);
    const manifest = JSON.parse(await readFile(join(directory, "teamarness.package.json"), "utf8"));
    assert.deepEqual(await Promise.all(paths.map(async path => hash(await readFile(path)))), before, "manual preparation must not replace integration-test executables with a different Cargo feature graph or inherited Cargo-test metadata");
    assert.equal(manifest.identity, "org.unfamiliar.headless-proof");
    assert.equal(hash(await readFile(join(directory, "backend/fixture"))), manifest.backends[0].sha256);
    assert.deepEqual(await readFile(join(directory, "ui/contract.json")), await readFile(new URL("../tests/system/fixtures/headless-personal-ui-contract.json", import.meta.url)));
    await assert.rejects(prepareFixture(directory), /EEXIST/);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("Cargo-owned runbook fixture assembly preserves live caller executables without nested builds", async () => {
  const { prepareCargoFixture } = await import("./prepare-headless-fixture.mjs");
  assert.equal(typeof prepareCargoFixture, "function", "runbook must assemble its Cargo-owned input without rebuilding live binaries");
  const [command, args] = qualificationCommands()[0];
  const built = spawnSync(command, args, { encoding: "utf8", maxBuffer: 16 * 1024 * 1024 });
  assert.equal(built.status, 0, built.stderr);
  const artifacts = built.stdout.split("\n").map(line => { try { return JSON.parse(line); } catch { return null; } }).filter(row => row?.reason === "compiler-artifact" && row.executable);
  const fixture = artifacts.find(row => row.target.name === "headless-personal-fixture");
  assert.ok(fixture, "Cargo reports the source-current fixture input");
  const hash = bytes => createHash("sha256").update(bytes).digest("hex");
  const before = await Promise.all(artifacts.map(async row => hash(await readFile(row.executable))));
  const root = await mkdtemp(join(tmpdir(), "headless-cargo-fixture-"));
  try {
    const directory = join(root, "fixture");
    const manifest = await prepareCargoFixture(directory, fixture.executable);
    assert.equal(hash(await readFile(join(directory, "backend/fixture"))), hash(await readFile(fixture.executable)));
    assert.equal(manifest.backends[0].sha256, hash(await readFile(fixture.executable)));
    assert.deepEqual(await Promise.all(artifacts.map(async row => hash(await readFile(row.executable)))), before);
    await assert.rejects(prepareCargoFixture(directory, fixture.executable), /EEXIST/);
    // Normal manual CLI offers no arbitrary prebuilt-executable bypass.
    const refused = spawnSync(process.execPath, ["scripts/prepare-headless-fixture.mjs", join(root, "other"), fixture.executable], { encoding: "utf8" });
    assert.equal(refused.status, 1);
    assert.match(refused.stderr, /usage:/);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("qualification refuses zero, ignored, filtered, failed or missing named cases", () => {
  const name = "sync_only_two_headless_and_desktop_backends_cover_two_workspaces_offline_restart_and_late_join";
  const summary = "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;";
  assert.equal(requireHeadlessTests(`test ${name} ... ok\n${summary}`, [name]), 1);
  for (const output of ["", summary, `test ${name} ... ignored\n${summary}`, `test ${name} ... ok\n${summary.replace("0 ignored", "1 ignored")}`, `test ${name} ... ok\n${summary.replace("0 filtered", "1 filtered")}`]) assert.throws(() => requireHeadlessTests(output, [name]), /every|executed/);
});
test("all shipped headless shell snippets parse without executing installation or service commands", async () => {
  for (const file of ["README.md", "verification.md", "follow-ups/agent.md", "follow-ups/external-gateway.md"]) {
    const blocks = shellBlocks(await readFile(new URL(`../docs/headless/${file}`, import.meta.url), "utf8"));
    assert.ok(blocks.length, `${file} has executable shell examples`);
    for (const block of blocks) {
      const result = spawnSync("bash", ["-n"], { input: block, encoding: "utf8" });
      assert.equal(result.status, 0, `${file}: ${result.stderr}`);
    }
  }
});
