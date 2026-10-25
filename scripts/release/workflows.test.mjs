import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";

const pins = {
  checkout: "08c6903cd8c0fde910a37f88322edcfb5dd907a8",
  "setup-node": "a0853c24544627f65ddf259abe73b1d18a591444",
  "upload-artifact": "ea165f8d65b6e75b540449e92b4886f43607fa02",
  "download-artifact": "d3f86a106a0bac45b974a628896c90dbdf5c8093",
};
// JSON-subset YAML makes the actual complete workflow structure testable without
// a second parser dependency or regex-only security qualification.
const load = async (name) => JSON.parse(await readFile(new URL(`../../.github/workflows/${name}.yml`, import.meta.url), "utf8"));
function nativeJob(job) {
  assert.equal(job["runs-on"], "macos-15");
  assert.equal(job.env.CI, "true");
  for (const step of job.steps) {
    if (!step.uses) continue;
    const [action, sha] = step.uses.split("@");
    assert.equal(sha, pins[action.slice("actions/".length)]); assert.equal(sha.length, 40);
    if (action === "actions/checkout") assert.deepEqual(step.with, { ref: "${{ github.sha }}", "persist-credentials": false });
    if (action === "actions/setup-node") assert.deepEqual(step.with, { "node-version": "24.20.0" });
  }
  assert.ok(job.steps.some((step) => step.run?.includes("rustup toolchain install 1.99.1 --profile minimal --component rustfmt --component clippy")));
  assert.ok(job.steps.some((step) => step.run === "node scripts/release/ci.mjs"));
  assert.ok(!JSON.stringify(job).includes("--target"));
  assert.ok(!JSON.stringify(job).includes("actions/cache"));
}
function checksJob(job) {
  assert.deepEqual(job.permissions, { contents: "read" });
  assert.ok(!JSON.stringify(job).includes("secrets.")); assert.equal(job.environment, undefined);
  for (const command of ["npm run check", "cargo build --workspace --locked", "npm run tauri build -- --bundles app,dmg -- --locked"]) assert.ok(job.steps.some((step) => step.run === command));
  assert.ok(job.steps.some((step) => step.run?.includes("build.mjs --disposable")));
}
// Verifies: TEAM.DEL-R04
 test("checks cover PR and main with no signing secrets, write token or unsafe PR execution", async () => {
  const workflow = await load("checks");
  assert.deepEqual(workflow.on, { pull_request: {}, push: { branches: ["main"] } });
  assert.deepEqual(workflow.permissions, { contents: "read" });
  assert.deepEqual(Object.keys(workflow.jobs), ["checks"]);
  nativeJob(workflow.jobs.checks); checksJob(workflow.jobs.checks);
});
// Verifies: TEAM.DEL-R04, TEAM.DEL-R05
 test("stable jobs use reviewed pins, exact guards/dependencies, protected environment and secret isolation", async () => {
  const workflow = await load("release");
  assert.deepEqual(workflow.on, { push: { tags: ["v*"] } }); assert.deepEqual(workflow.permissions, { contents: "read" });
  assert.deepEqual(Object.keys(workflow.jobs), ["checks", "signed-build", "publish"]);
  const guard = "github.event_name == 'push' && github.repository == 'learn-ai-engineering/teamarness' && startsWith(github.ref, 'refs/tags/v') && github.event.deleted == false";
  for (const [name, job] of Object.entries(workflow.jobs)) {
    nativeJob(job); assert.equal(job.if, guard);
    const preflight = job.steps.findIndex((s) => s.run === 'node scripts/release/cli.mjs preflight --tag "$TAG"');
    assert.ok(preflight > 0);
    for (let at = 0; at < job.steps.length; at++) {
      const step = job.steps[at];
      if (JSON.stringify(step).includes("secrets.")) {
        assert.equal(name, "signed-build"); assert.ok(at > preflight);
        assert.equal(step.run, "node scripts/release/build.mjs --production");
        assert.deepEqual(Object.keys(step.env).sort(), ["TAURI_SIGNING_PRIVATE_KEY", "TAURI_SIGNING_PRIVATE_KEY_PASSWORD", "TEAMARNESS_UPDATER_PUBLIC_KEY"]);
      }
    }
    assert.ok(!JSON.stringify(job.env).includes("secrets."));
    if (name !== "checks") assert.equal(job.environment, "release");
  }
  checksJob(workflow.jobs.checks);
  assert.deepEqual(workflow.jobs["signed-build"].needs, ["checks"]);
  assert.deepEqual(workflow.jobs["signed-build"].permissions, { contents: "read" });
  assert.deepEqual(workflow.jobs.publish.needs, ["checks", "signed-build"]);
  assert.deepEqual(workflow.jobs.publish.permissions, { contents: "write" });
  assert.deepEqual(workflow.jobs.publish.concurrency, { group: "teamarness-stable-publication", "cancel-in-progress": false });
  assert.equal(JSON.stringify(workflow).match(/secrets\./g).length, 2);
});
// Verifies: TEAM.DEL-R05
 test("only same-run allowlisted distribution is transferred; validators rebuild before publication", async () => {
  const workflow = await load("release");
  const upload = workflow.jobs["signed-build"].steps.find((s) => s.uses?.startsWith("actions/upload-artifact@"));
  assert.deepEqual(upload.with, { name: "qualified-stable-distribution", path: "${{ steps.inputs.outputs.assets }}", "if-no-files-found": "error", "retention-days": 3 });
  const steps = workflow.jobs.publish.steps;
  const download = steps.find((s) => s.uses?.startsWith("actions/download-artifact@"));
  assert.deepEqual(download.with, { name: "qualified-stable-distribution", path: "target/release/distribution" });
  assert.ok(steps.some((s) => s.run?.includes("cargo build --locked -p teamarness-desktop --example qualify-update")));
  assert.ok(steps.some((s) => s.run?.includes("cargo build --locked -p teamarness-package-contract --bin teamarness-package-validate")));
  assert.ok(steps.at(-1).run.startsWith("npm run release:publish --"));
  const ci = await readFile(new URL("./ci.mjs", import.meta.url), "utf8");
  assert.ok(ci.includes('[...assetNames(tag), "qualification.json"]'));
  assert.ok(!JSON.stringify(workflow).includes("continue-on-error"));
  assert.ok(!JSON.stringify(workflow).includes("pull_request_target"));
});
