import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { repositoryRoot } from "./bundled-packages.mjs";

const MAC = "aarch64-apple-darwin", LINUX = "x86_64-unknown-linux-gnu";

// Pure selection: no command execution, filesystem access, or default application/target.
export function describeBuild(args, nativeTarget) {
  const options = new Map();
  for (let i = 0; i < args.length; i += 2) {
    const key = args[i], value = args[i + 1];
    if (!["--app", "--target"].includes(key) || !value || options.has(key)) throw new Error("Usage: build:app -- --app desktop|headless --target <native-target>");
    options.set(key, value);
  }
  const app = options.get("--app"), target = options.get("--target");
  if (!((app === "desktop" && target === MAC) || (app === "headless" && [MAC, LINUX].includes(target)))) {
    throw new Error("Select desktop aarch64-apple-darwin, or headless aarch64-apple-darwin|x86_64-unknown-linux-gnu; both selectors are required");
  }
  if (target !== nativeTarget) throw new Error(`Application target ${target} is not native (${nativeTarget})`);
  return app === "desktop"
    ? { app, target, command: "npm", args: ["run", "tauri", "build", "--", "--bundles", "app,dmg"], outputDirectory: "target/release/bundle" }
    : { app, target, command: "node", args: ["scripts/build-headless.mjs", "--target", target], outputDirectory: `target/teamarness-applications/headless/${target}` };
}

export async function buildApplication(args, { root = repositoryRoot, run = spawnSync } = {}) {
  const host = run("rustc", ["-vV"], { cwd: root, encoding: "utf8", shell: false });
  const native = host.status === 0 && /^host: (.+)$/m.exec(host.stdout)?.[1];
  if (!native) throw new Error("rustc did not report its native host target");
  const selection = describeBuild(args, native);
  const result = run(selection.command, selection.args, { cwd: root, stdio: "inherit", shell: false });
  if (result.status !== 0) throw new Error(`${selection.app} build failed (${result.status ?? "could not start"})`);
  return selection;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  buildApplication(process.argv.slice(2)).catch((error) => { console.error(error.message); process.exitCode = 1; });
}
