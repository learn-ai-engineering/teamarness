import { lstat, readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { repositoryRoot } from "../bundled-packages.mjs";
import { qualifyRelease } from "../qualify-release.mjs";
import { checkAssetDirectory, native } from "./artifacts.mjs";
import { assetNames } from "./manifest.mjs";
import { checkVersion } from "./version.mjs";
import { preflight, publishRelease } from "./publish.mjs";
import { githubAdapter, fixtureGithubAdapter } from "./github.mjs";

export function assertEnvironment({ node = process.versions.node, platform = process.platform, arch = process.arch, rust = native("rustc", ["-Vv"]) } = {}) {
  if (node !== "24.20.0" || platform !== "darwin" || arch !== "arm64" || !rust.includes("release: 1.99.1\n") || !rust.includes("host: aarch64-apple-darwin")) throw new Error("Expected Node 24.20.0 / Rust 1.99.1 on native Apple Silicon macOS");
}
export function releaseEvent(environment, payload) {
  return { name: environment.GITHUB_EVENT_NAME, repository: environment.GITHUB_REPOSITORY, ref: environment.GITHUB_REF, sha: environment.GITHUB_SHA, deleted: payload.deleted === false ? false : true };
}

const HELP = `Protected same-repository release policy (native Apple Silicon only).
Usage: node scripts/release/cli.mjs preflight --tag vX.Y.Z
       npm run release:publish -- --tag vX.Y.Z --public-key FILE [--dry-run [--fixture FILE]]
preflight uses CI event state and a read-only GITHUB_TOKEN, checks clean source, versions, tools, public repository, protected main ancestry and resolved tag SHA.
publish reads only target/release/distribution: four assets + same-run qualification.json.
--dry-run makes no remote mutations; it still requires clean source and real native artifact/signature qualification.
--fixture is allowed ONLY with --dry-run, replacing GitHub reads with a local JSON fixture (event, repository, main, tagSha, ancestor, releases).
Without --fixture, only the canonical stable tag push event is accepted. No PAT, arbitrary destination or uploaded executable is used.
Publication requires a serialized protected release job; published releases are immutable.`;

export async function runReleaseCli(args) {
  if (args.length === 1 && args[0] === "--help") { console.log(HELP); return; }
  const command = args.shift(), flags = {};
  if (!["preflight", "publish"].includes(command)) throw new Error("Expected preflight or publish; use --help");
  for (let at = 0; at < args.length; at++) {
    const flag = args[at];
    if (flag === "--dry-run" && !flags[flag]) flags[flag] = true;
    else if (["--tag", "--public-key", "--fixture"].includes(flag) && args[at + 1] && flags[flag] === undefined) flags[flag] = args[++at];
    else throw new Error("Invalid/duplicate release arguments; use --help");
  }
  const tag = flags["--tag"], dryRun = flags["--dry-run"] === true;
  if (!tag || (flags["--fixture"] && (!dryRun || command !== "publish")) || (command === "preflight" && (dryRun || flags["--public-key"]))) throw new Error("Invalid release arguments; use --help");
  assertEnvironment();
  await checkVersion({ root: repositoryRoot, tag });
  const sourceSha = native("git", ["rev-parse", "HEAD"]);
  if (native("git", ["status", "--porcelain"]) !== "") throw new Error("Release requires clean committed source");
  let event, api;
  if (flags["--fixture"]) {
    const path = flags["--fixture"];
    if (!(await lstat(path)).isFile() || (await lstat(path)).size > 1024 * 1024) throw new Error("Expected bounded fixture file");
    const fixture = JSON.parse(await readFile(path, "utf8"));
    event = fixture.event; api = fixtureGithubAdapter(fixture);
  } else {
    const payload = JSON.parse(await readFile(process.env.GITHUB_EVENT_PATH, "utf8"));
    event = releaseEvent(process.env, payload); api = githubAdapter({ token: process.env.GITHUB_TOKEN });
  }
  if (command === "preflight") { await preflight({ api, event, tag, sourceSha }); console.log(`Preflight qualified ${tag} at ${sourceSha}`); return; }
  if (!flags["--public-key"]) throw new Error("Missing --public-key");
  const publicKey = resolve(flags["--public-key"]), directory = join(repositoryRoot, "target/release/distribution");
  await checkAssetDirectory({ directory, tag });
  const record = JSON.parse(await readFile(join(directory, "qualification.json"), "utf8"));
  const files = Object.fromEntries(await Promise.all(assetNames(tag).map(async (name) => [name, await readFile(join(directory, name))])));
  const qualify = async (downloaded) => {
    // Only the already digest-matched four allowlisted bytes are written. Native
    // validators are rebuilt from this checkout, never accepted as CI artifacts.
    for (const name of assetNames(tag)) await writeFile(join(directory, name), downloaded[name], { mode: 0o600 });
    await qualifyRelease({ directory, tag, publicKey, referenceRecord: record });
  };
  console.log(JSON.stringify(await publishRelease({ api, event, tag, sourceSha, record, files, qualify, dryRun })));
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) runReleaseCli(process.argv.slice(2)).catch((error) => { console.error(error.message); process.exitCode = 1; });
