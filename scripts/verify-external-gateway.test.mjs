import assert from "node:assert/strict";
import test from "node:test";
import { spawnSync } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parseArgs, qualify, requireExecutedTests, validateProvenance } from "./verify-external-gateway.mjs";

// Verifies: TEAM.SYS.PKG-R09, TEAM.SYS.PKG-R13 — explicit artifacts, never a silent skip.
test("qualification requires exactly one explicit project", () => {
  assert.throws(() => parseArgs([]), /--project/);
  assert.throws(() => parseArgs(["--project", "/tmp/g", "--skip-build"]), /--project/);
  assert.throws(() => parseArgs(["--project"]), /--project/);
  assert.equal(parseArgs(["--project", "/tmp/g"]), "/tmp/g");
});
test("zero, ignored, failed, and missing test results cannot qualify", () => {
  for (const text of ["", "test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out;", "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out;"]) {
    assert.throws(() => requireExecutedTests(text), /executed/);
  }
  assert.equal(requireExecutedTests("test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;"), 3);
});
test("SDK provenance must be clean and all four exact vendor trees must agree", () => {
  const provenance = { sdkRevision: "a".repeat(40), sdkRevisionDirty: false, vendorDigests: Object.fromEntries(["package-contract", "package-protocol", "package-sdk", "package-ui-sdk"].map(t => [t, `sha256:${"b".repeat(64)}`])) };
  const report = { trees: Object.fromEntries(Object.keys(provenance.vendorDigests).map(t => [t, "same"])) };
  validateProvenance(provenance, report);
  assert.throws(() => validateProvenance({ ...provenance, sdkRevisionDirty: true }, report), /clean/);
  assert.throws(() => validateProvenance({ ...provenance, sdkRevision: "HEAD" }, report), /revision/);
  assert.throws(() => validateProvenance(provenance, { trees: {} }), /vendor/);
  assert.throws(() => validateProvenance({ ...provenance, vendorDigests: {} }, report), /vendor/);
});
test("qualification counts every discovered test and rejects mixed or incomplete reports", () => {
  const summary = "test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;";
  assert.throws(() => requireExecutedTests(summary, ["first", "second"]), /every/);
  assert.throws(() => requireExecutedTests(`test first ... ok\n${summary}`, ["first", "second"]), /every/);
  assert.equal(requireExecutedTests(`test first ... ok\ntest second ... ok\n${summary}`, ["first", "second"]), 2);
  assert.throws(() => requireExecutedTests(`${summary}\ntest result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out;`), /executed/);
  assert.throws(() => requireExecutedTests("test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out;"), /executed/);
});

test("dirty gateway source is refused before any build or activation", async () => {
  const project = await mkdtemp(join(tmpdir(), "gateway-runner-guard-"));
  try {
    assert.equal(spawnSync("git", ["init", "--quiet", project]).status, 0);
    await writeFile(join(project, "untracked-source"), "dirty");
    await assert.rejects(qualify(project), /source must be clean before build/);
  } finally { await rm(project, { recursive: true, force: true }); }
});
