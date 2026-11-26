import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmod, mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { repositoryRoot } from "./bundled-packages.mjs";

const setup = join(repositoryRoot, "scripts/headless/teamarness-headless-setup");

// Fake Host binary: answers operator commands from a JSON state file and logs argv.
const fakeHost = String.raw`#!/usr/bin/env python3
import json, os, sys
state_path = os.environ["FAKE_STATE"]
state = json.load(open(state_path))
args = sys.argv[1:]
if args == ["--version"]:
    print("teamarness-headless 0.1.0"); sys.exit(0)
args = args[2:]  # drop --data-dir DATA
state.setdefault("calls", []).append(args)
def done(body, code=0):
    json.dump(state, open(state_path, "w")); print(json.dumps(body)); sys.exit(code)
if args == ["status"]:
    if not state.get("active"): done({"code": "unavailable", "outcomeUncertain": False}, 1)
    done({"readiness": "ready"})
if args == ["identity", "show"]:
    done({"displayName": state.get("name", ""), "installation": "ab" * 32})
if args[:2] == ["identity", "set-name"]:
    state["name"] = args[3]; done({"changed": True})
if args == ["workspace", "list"]:
    done(state.get("workspaces", []))
if args[:2] == ["workspace", "join"]:
    if state.get("join") == "uncertain": done({"code": "host_failure", "outcomeUncertain": True}, 1)
    workspace = {"id": "cd" * 16, "name": "team"}
    state.setdefault("workspaces", []).append(workspace)
    if state.get("join") == "unreconciled":
        done({"code": "reconciliation_failed", "retryable": False, "outcomeUncertain": False,
              "details": {"kind": "joined", "membershipCommitted": True, "workspace": workspace, "reconciliation": {}}}, 1)
    done({"membershipCommitted": True, "workspace": workspace})
done({"code": "invalid_input"}, 2)
`;

const fakeSystemctl = String.raw`#!/usr/bin/env python3
import json, os, sys
state_path = os.environ["FAKE_STATE"]
state = json.load(open(state_path))
args = sys.argv[2:]  # drop --user
state.setdefault("systemctl", []).append(args)
out = ""
if args[0] == "is-active": out = "active" if state.get("active") else "inactive"
elif args[0] == "is-enabled": out = "enabled" if state.get("enabled") else "disabled"
elif args[:2] == ["enable", "--now"]: state["enabled"] = state["active"] = True
elif args[0] in ("start", "restart"): state["active"] = True
elif args[0] == "stop": state["active"] = False
json.dump(state, open(state_path, "w"))
print(out)
`;

const fakeLoginctl = String.raw`#!/usr/bin/env python3
import json, os, sys
state_path = os.environ["FAKE_STATE"]
state = json.load(open(state_path))
args = sys.argv[1:]
if args[0] == "show-user": print("yes" if state.get("linger") else "no"); sys.exit(0)
if args[0] == "enable-linger": state["linger"] = True
json.dump(state, open(state_path, "w"))
`;

async function fixture(initialState = {}) {
  const root = await mkdtemp(join(tmpdir(), "teamarness-setup-"));
  const home = join(root, "home"), bin = join(root, "bin"), dist = join(root, "dist");
  await mkdir(home, { mode: 0o700 });
  await mkdir(bin);
  await mkdir(join(dist, "packages"), { recursive: true });
  await writeFile(join(dist, "packages/application-packages.v1.json"), "{}");
  for (const [path, body] of [[join(dist, "teamarness-headless"), fakeHost], [join(bin, "systemctl"), fakeSystemctl], [join(bin, "loginctl"), fakeLoginctl]]) {
    await writeFile(path, body, { mode: 0o755 });
  }
  const state = join(root, "state.json");
  await writeFile(state, JSON.stringify(initialState));
  const answers = join(root, "host.toml");
  const writeAnswers = (extra = "") => writeFile(answers, `distribution_dir = "${dist}"\ninstallation_name = "box"\n${extra}`);
  await writeAnswers();
  const run = (...args) => {
    const result = spawnSync("python3", ["-I", setup, ...args], {
      encoding: "utf8",
      env: { HOME: home, PATH: `${bin}:${process.env.PATH}`, FAKE_STATE: state, USER: "tester", LOGNAME: "tester" },
    });
    return { code: result.status, out: result.stdout, err: result.stderr };
  };
  const readState = async () => JSON.parse(await readFile(state, "utf8"));
  return { root, home, dist, answers, writeAnswers, run, readState, cleanup: () => rm(root, { recursive: true, force: true }) };
}

const lines = (out, prefix) => out.split("\n").filter((line) => line.startsWith(prefix));

test("print-template writes a private file and refuses to overwrite it", async () => {
  const f = await fixture();
  try {
    const path = join(f.root, "template.toml");
    assert.equal(f.run("--print-template", path).code, 0);
    assert.equal((await stat(path)).mode & 0o777, 0o600);
    assert.match(await readFile(path, "utf8"), /distribution_dir = ""/);
    assert.equal(f.run("--print-template", path).code, 2);
    assert.match(f.run("--print-template").out, /\[join\]/);
  } finally { await f.cleanup(); }
});

test("invalid answers exit 2 before any change", async () => {
  const f = await fixture();
  try {
    await f.writeAnswers('surprise = true\n');
    const result = f.run("--answers", f.answers);
    assert.equal(result.code, 2);
    assert.match(result.out, /unknown key `surprise`/);
    await writeFile(f.answers, 'installation_name = "box"\n');
    assert.equal(f.run("--answers", f.answers).code, 2);
    assert.deepEqual(await f.readState(), {});
  } finally { await f.cleanup(); }
});

test("a fresh run installs everything and a rerun changes nothing", async () => {
  const f = await fixture();
  try {
    const first = f.run("--answers", f.answers);
    assert.equal(first.code, 0, first.out + first.err);
    for (const item of ["install", "unit", "service enabled and started", "linger enabled", "installation name box", "helpers"]) {
      assert.ok(lines(first.out, "changed:").some((line) => line.includes(item)), `${item} changed\n${first.out}`);
    }
    const unit = await readFile(join(f.home, ".config/systemd/user/teamarness-headless.service"), "utf8");
    // Default paths render exactly as the documented example unit.
    assert.equal(unit, await readFile(join(repositoryRoot, "docs/headless/teamarness-headless.service.example"), "utf8"));
    assert.equal((await stat(join(f.home, ".config/teamarness-headless/helpers.sh"))).mode & 0o777, 0o600);

    const second = f.run("--answers", f.answers);
    assert.equal(second.code, 0, second.out);
    assert.deepEqual(lines(second.out, "changed:"), []);
    assert.match(second.out, /summary: 0 changed/);
    const state = await f.readState();
    assert.equal(state.calls.filter((call) => call[1] === "set-name").length, 1);
  } finally { await f.cleanup(); }
});

test("a changed distribution is applied as an update around a service stop", async () => {
  const f = await fixture();
  try {
    assert.equal(f.run("--answers", f.answers).code, 0);
    await writeFile(join(f.dist, "packages/application-packages.v1.json"), '{"v":2}');
    const update = f.run("--answers", f.answers);
    assert.equal(update.code, 0, update.out);
    assert.match(update.out, /changed: install .* \(updated\)/);
    const commands = (await f.readState()).systemctl.map((call) => call[0]);
    assert.ok(commands.lastIndexOf("stop") < commands.lastIndexOf("start"));
    assert.equal(await readFile(join(f.home, ".local/lib/teamarness-headless/packages/application-packages.v1.json"), "utf8"), '{"v":2}');
  } finally { await f.cleanup(); }
});

test("owned group-writable ancestors are tightened", async () => {
  const f = await fixture();
  try {
    await mkdir(join(f.home, ".local/lib"), { recursive: true });
    await chmod(join(f.home, ".local/lib"), 0o775);
    const result = f.run("--answers", f.answers);
    assert.equal(result.code, 0, result.out);
    assert.match(result.out, /changed: permissions .*\.local\/lib no longer group\/other-writable/);
    assert.equal((await stat(join(f.home, ".local/lib"))).mode & 0o777, 0o755);
  } finally { await f.cleanup(); }
});

test("a complete invitation joins once and its file is deleted", async () => {
  const f = await fixture();
  try {
    const invitation = join(f.root, "invite.txt");
    const token = "00".repeat(4), ticket = Buffer.from("ticket").toString("hex");
    const payload = "02" + "0004" + token + "11".repeat(32) + "0006" + ticket;
    await writeFile(invitation, `teamarness-workspace-v1.${payload}\n`, { mode: 0o600 });
    await f.writeAnswers(`[join]\ninvitation_file = "${invitation}"\n`);
    const first = f.run("--answers", f.answers);
    assert.equal(first.code, 0, first.out);
    assert.match(first.out, /changed: joined Workspace team/);
    await assert.rejects(stat(invitation), /ENOENT/);
    const second = f.run("--answers", f.answers);
    assert.equal(second.code, 0, second.out);
    assert.match(second.out, /unchanged: join \(invitation file consumed/);
  } finally { await f.cleanup(); }
});

test("a truncated invitation stops before joining", async () => {
  const f = await fixture();
  try {
    const invitation = join(f.root, "invite.txt");
    await writeFile(invitation, `teamarness-workspace-v1.02${"ab".repeat(115)}`, { mode: 0o600 });
    await f.writeAnswers(`[join]\ninvitation_file = "${invitation}"\n`);
    const result = f.run("--answers", f.answers);
    assert.equal(result.code, 2);
    assert.match(result.out, /incomplete Invitation/);
    const state = await f.readState();
    assert.deepEqual(state, {}, "nothing ran or changed before the Invitation was refused");
    await stat(invitation);
  } finally { await f.cleanup(); }
});

test("a committed join whose Packages fail to start deletes the spent file and says so", async () => {
  const f = await fixture({ join: "unreconciled" });
  try {
    const invitation = join(f.root, "invite.txt");
    const payload = "02" + "0004" + "00".repeat(4) + "11".repeat(32) + "0002" + "6869";
    await writeFile(invitation, `teamarness-workspace-v1.${payload}`, { mode: 0o600 });
    await f.writeAnswers(`[join]\ninvitation_file = "${invitation}"\n`);
    const first = f.run("--answers", f.answers);
    assert.equal(first.code, 1);
    assert.match(first.out, /stopped: joined Workspace team \(c+d+.*\) and deleted the invitation file, but its Packages did not start \(reconciliation_failed\)/);
    await assert.rejects(stat(invitation));
    const second = f.run("--answers", f.answers);
    assert.match(second.out, /unchanged: join \(invitation file consumed/);
    assert.equal((await f.readState()).calls.filter((call) => call[1] === "join").length, 1);
  } finally { await f.cleanup(); }
});

test("an uncertain join stops without retry and keeps the file", async () => {
  const f = await fixture({ join: "uncertain" });
  try {
    const invitation = join(f.root, "invite.txt");
    const payload = "02" + "0004" + "00".repeat(4) + "11".repeat(32) + "0002" + "6869";
    await writeFile(invitation, `teamarness-workspace-v1.${payload}`, { mode: 0o600 });
    await f.writeAnswers(`[join]\ninvitation_file = "${invitation}"\n`);
    const result = f.run("--answers", f.answers);
    assert.equal(result.code, 1);
    assert.match(result.out, /stopped: join outcome unknown — inspect with: .* workspace list, then rerun setup/);
    assert.equal((await f.readState()).calls.filter((call) => call[1] === "join").length, 1);
    await stat(invitation);
  } finally { await f.cleanup(); }
});
