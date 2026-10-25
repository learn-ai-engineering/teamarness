import { spawn } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { repositoryRoot } from "../bundled-packages.mjs";
import { qualifyRelease } from "../qualify-release.mjs";
import { assertEnvironment } from "./cli.mjs";
import { checkVersion } from "./version.mjs";

function run(args, { env = process.env, privateOutput = false } = {}) {
  return new Promise((resolveRun, reject) => {
    const child = spawn("npm", args, { cwd: repositoryRoot, env, stdio: privateOutput ? "ignore" : "inherit", shell: false });
    child.on("error", () => reject(new Error("Release build tool could not start")));
    child.on("exit", (code) => code === 0 ? resolveRun() : reject(new Error(`Release build tool failed (${code}); signing values are never logged by this wrapper`)));
  });
}
export async function buildRelease(args) {
  const disposable = args[0] === "--disposable";
  if ((!disposable && (args.length !== 1 || args[0] !== "--production")) || (disposable && (args.length !== 3 || args[1] !== "--public-key-out"))) throw new Error("Usage: node scripts/release/build.mjs --production | --disposable --public-key-out FILE");
  assertEnvironment();
  const version = await checkVersion({ root: repositoryRoot });
  const temporary = await mkdtemp(join(tmpdir(), "teamarness-private-signing-"));
  try {
    let publicKey = process.env.TEAMARNESS_UPDATER_PUBLIC_KEY;
    let signingKey = process.env.TAURI_SIGNING_PRIVATE_KEY;
    let password = process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD;
    if (disposable) {
      signingKey = join(temporary, "key"); password = "";
      await run(["run", "tauri", "signer", "generate", "--", "--ci", "-p", "", "-w", signingKey], { privateOutput: true });
      publicKey = (await readFile(`${signingKey}.pub`, "utf8")).trim();
    }
    if (!publicKey || !signingKey || password === undefined) throw new Error("Missing reviewed public key or protected signing inputs");
    const overlay = join(temporary, "public-overlay.json"), keyFile = join(temporary, "public-key.txt");
    await writeFile(overlay, JSON.stringify({ plugins: { updater: { pubkey: publicKey } } }), { mode: 0o600 });
    await writeFile(keyFile, publicKey, { mode: 0o600 });
    await run(["run", "tauri", "build", "--", "--config", "src-tauri/tauri.release.conf.json", "--config", overlay, "--bundles", "app,dmg", "--", "--locked"], { env: { ...process.env, TAURI_SIGNING_PRIVATE_KEY: signingKey, TAURI_SIGNING_PRIVATE_KEY_PASSWORD: password } });
    // The wrapper owns only temporary private material; never remove maintainer keys.
    if (disposable) {
      await writeFile(resolve(args[2]), publicKey, { mode: 0o600 });
      const privateKey = process.env.TAURI_SIGNING_PRIVATE_KEY, privatePassword = process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD;
      delete process.env.TAURI_SIGNING_PRIVATE_KEY; delete process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD;
      try { await qualifyRelease({ directory: join(repositoryRoot, "target/release/distribution"), tag: `v${version}`, publicKey: keyFile, collect: true }); }
      finally {
        if (privateKey !== undefined) process.env.TAURI_SIGNING_PRIVATE_KEY = privateKey;
        if (privatePassword !== undefined) process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD = privatePassword;
      }
    }
  } finally { await rm(temporary, { recursive: true, force: true }); }
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) buildRelease(process.argv.slice(2)).catch((error) => { console.error(error.message); process.exitCode = 1; });
