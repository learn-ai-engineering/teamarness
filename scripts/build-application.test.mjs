import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { describeBuild, buildApplication } from "./build-application.mjs";

const mac = "aarch64-apple-darwin", linux = "x86_64-unknown-linux-gnu";
const select = (app, target) => ["--app", app, "--target", target];

// Verifies: TEAM.DEL-R07
test("exact native matrix describes one application before execution", () => {
  assert.deepEqual(describeBuild(select("desktop", mac), mac), {
    app: "desktop", target: mac, command: "npm", args: ["run", "tauri", "build", "--", "--bundles", "app,dmg"],
    outputDirectory: "target/release/bundle",
  });
  for (const target of [mac, linux]) {
    assert.deepEqual(describeBuild(select("headless", target), target), {
      app: "headless", target, command: "node", args: ["scripts/build-headless.mjs", "--target", target],
      outputDirectory: `target/teamarness-applications/headless/${target}`,
    });
  }
  for (const args of [[], ["--app", "headless"], ["--target", mac], select("desktop", linux), select("both", mac), select("headless", "aarch64-unknown-linux-gnu"), [...select("headless", mac), "--target", mac], [...select("headless", mac), "--all"]]) {
    assert.throws(() => describeBuild(args, mac));
  }
  assert.throws(() => describeBuild(select("headless", linux), mac), /not native/);
});

test("native preflight is the only command before a mismatch, with no output mutation", async () => {
  const commands = [];
  const run = (command, args) => {
    commands.push([command, args]);
    return { status: 0, stdout: `rustc test\nhost: ${mac}\n`, stderr: "" };
  };
  await assert.rejects(buildApplication(select("headless", linux), { run }), /not native/);
  assert.deepEqual(commands, [["rustc", ["-vV"]]]);
  commands.length = 0;
  await buildApplication(select("desktop", mac), { run });
  assert.deepEqual(commands, [["rustc", ["-vV"]], ["npm", ["run", "tauri", "build", "--", "--bundles", "app,dmg"]]]);
  commands.length = 0;
  await buildApplication(select("headless", mac), { run });
  assert.deepEqual(commands, [["rustc", ["-vV"]], ["node", ["scripts/build-headless.mjs", "--target", mac]]]);
  await assert.rejects(buildApplication(select("headless", mac), { run: () => ({ status: 1, stdout: "" }) }), /rustc/);
});

test("manual user-service example preserves shutdown and private-path policy", async () => {
  const service = await readFile(new URL("../docs/headless/teamarness-headless.service.example", import.meta.url), "utf8");
  for (const line of ["ExecStart=%h/.local/lib/teamarness-headless/teamarness-headless serve --data-dir %h/.local/share/teamarness-headless", "UMask=0077", "Restart=on-failure", "TimeoutStopSec=150s", "KillMode=mixed", "WantedBy=default.target"]) assert.ok(service.split("\n").includes(line), line);
  assert.doesNotMatch(service, /User=|DISPLAY=|EnvironmentFile=|ExecStartPre=|sudo/);
  const guide = await readFile(new URL("../docs/headless/README.md", import.meta.url), "utf8");
  for (const instruction of ["loginctl enable-linger", "systemd-analyze --user verify", "journalctl --user", "120 seconds", "150s", "outcomeUncertain", "retryable", "details", "pending native qualification", "No launchd"]) assert.ok(guide.includes(instruction), instruction);
});

test("legacy desktop and release command ownership remain unchanged", async () => {
  const { scripts } = JSON.parse(await readFile(new URL("../package.json", import.meta.url)));
  assert.equal(scripts["build:app"], "node scripts/build-application.mjs");
  assert.equal(scripts.build, "npm --workspace @teamarness/package-ui-sdk run build && npm run build:application-packages && npm --workspace teamarness-desktop run build");
  const release = await readFile(new URL("./release/build.mjs", import.meta.url), "utf8");
  assert.match(release, /await run\(\["run", "tauri", "build"/);
  assert.doesNotMatch(release, /build:app|build-headless/);
});
