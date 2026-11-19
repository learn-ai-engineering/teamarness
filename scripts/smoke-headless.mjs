import assert from "node:assert/strict";
import { execFile, spawn } from "node:child_process";
import { once } from "node:events";
import { chmod, cp, lstat, mkdir, mkdtemp, readdir, rm, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { readBundledPackages } from "./bundled-packages.mjs";
import { nativeBackendTarget } from "./package-directory.mjs";
import { verifyHeadlessDistribution } from "./headless-distribution.mjs";
const exec = promisify(execFile);

// Only used for this smoke's mkdtemp tree. Host admission deliberately makes UI caches read-only.
export async function removeSmokeDirectory(directory) {
  const makeDirectoriesWritable = async (path) => {
    const details = await lstat(path);
    if (!details.isDirectory() || details.isSymbolicLink()) return;
    await chmod(path, 0o700);
    for (const entry of await readdir(path, { withFileTypes: true })) {
      if (entry.isDirectory() && !entry.isSymbolicLink()) await makeDirectoriesWritable(join(path, entry.name));
    }
  };
  await makeDirectoriesWritable(directory);
  await rm(directory, { recursive: true, force: true });
}

export async function smokeHeadless(directory) {
  const target = nativeBackendTarget(), packages = await readBundledPackages();
  const temporary = await mkdtemp(join(tmpdir(), "tmh-"));
  try {
    const installed = join(temporary, "installed"), cwd = join(temporary, "outside"), data = join(temporary, "data");
    await cp(resolve(directory), installed, { recursive: true });
    await mkdir(cwd, { mode: 0o700 });
    const artifact = await verifyHeadlessDistribution({ directory: installed, target, packages });
    // A symlink also proves that default resources follow the canonical executable, not argv[0].
    const executable = join(cwd, "teamarness-headless");
    await symlink(join(installed, "teamarness-headless"), executable);
    const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !/^(DISPLAY|WAYLAND_DISPLAY|MIR_SOCKET|XAUTHORITY|DBUS_SESSION_BUS_ADDRESS|XDG_SESSION_TYPE|XDG_CURRENT_DESKTOP|DESKTOP_SESSION|QT_.*|QML_.*|GTK_.*|GDK_.*|TEAMARNESS_.*)$/.test(key)));
    const run = (args) => exec(executable, args, { cwd, env, timeout: 100_000, maxBuffer: 2 * 1024 * 1024 });
    assert.match((await run(["--help"])).stdout, /Usage:/);
    assert.match((await run(["--version"])).stdout, /^teamarness-headless \d+\.\d+\.\d+/);
    await assert.rejects(lstat(data), /ENOENT/);
    let identity;
    for (const signal of ["SIGTERM", "SIGINT"]) {
      const child = spawn(executable, ["serve", "--data-dir", data], { cwd, env, stdio: ["ignore", "ignore", "pipe"] });
      let diagnostics = "";
      child.stderr.on("data", (bytes) => { diagnostics = (diagnostics + bytes).slice(-16_384); });
      const exited = once(child, "exit");
      try {
        const deadline = Date.now() + 30_000;
        while (!(await lstat(join(data, "control.sock")).catch((error) => { if (error.code !== "ENOENT") throw error; }))?.isSocket()) {
          if (child.exitCode !== null || child.signalCode !== null || Date.now() >= deadline) throw new Error(`Disposable serve did not bind: ${diagnostics}`);
          // poll-interval: wait for the owner-bound endpoint, never replay an operator request.
          await delay(20);
        }
        const status = JSON.parse((await run(["--data-dir", data, "status"])).stdout);
        assert.equal(status.readiness, "ready");
        assert.deepEqual(status.host.diagnostics, []);
        assert.deepEqual(status.host.registrations, []);
        assert.deepEqual(status.host.packages.map(({ identity }) => identity).sort(), artifact.identities.toSorted());
        assert.deepEqual(status.host.instances, []); // No Workspace or optional registration is manufactured.
        if (identity) assert.equal(status.installation, identity);
        identity = status.installation;
        child.kill(signal);
        const result = await Promise.race([exited, delay(150_000, null, { ref: false })]);
        assert.deepEqual(result, [0, null], `Shutdown ${signal}: ${diagnostics}`);
        await assert.rejects(lstat(join(data, "control.sock")), /ENOENT/);
      } finally {
        if (child.exitCode === null && child.signalCode === null) { child.kill("SIGKILL"); await exited; }
      }
    }
    console.log(JSON.stringify({ ...artifact, smoke: "offline help/version; relocated canonical resources; GUI environment absent; fresh status; SIGTERM/SIGINT; persistent identity" }));
    return artifact;
  } finally { await removeSmokeDirectory(temporary); }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  if (args.length !== 1) { console.error("Usage: smoke-headless.mjs <distribution-directory>"); process.exitCode = 2; }
  else smokeHeadless(args[0]).catch((error) => { console.error(error); process.exitCode = 1; });
}
