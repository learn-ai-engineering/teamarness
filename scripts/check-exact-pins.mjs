import { execFile } from "node:child_process";
import { readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const scriptPath = fileURLToPath(import.meta.url);
export const repositoryRoot = resolve(dirname(scriptPath), "..");

// Every Cargo dependency is pinned to one exact version, so a `cargo update` can
// never change what a build resolves without a reviewed manifest edit. Path and
// workspace dependencies carry no version of their own and are exempt.
const DEPENDENCY_TABLE =
  /^(?:target\.(?:'[^']*'|"[^"]*")\.)?(?:workspace\.)?(?:dependencies|dev-dependencies|build-dependencies)(?:\.(.+))?$/;

function stripComment(line) {
  let quote = null;
  for (let at = 0; at < line.length; at += 1) {
    const character = line[at];
    if (quote) {
      if (character === quote) quote = null;
    } else if (character === '"' || character === "'") {
      quote = character;
    } else if (character === "#") {
      return line.slice(0, at);
    }
  }
  return line;
}

function field(text, name) {
  const match = new RegExp(`(^|[{,\\s])${name}\\s*=\\s*("([^"]*)"|true|false)`).exec(text);
  if (!match) return undefined;
  return match[3] ?? match[2];
}

// A minimal table scan rather than a TOML parser: the repository's manifests use
// one-line entries and `[dependencies.name]` blocks, and a parser would be a new
// npm dependency for a check that runs on every `npm test`.
export function findLoosePinsInManifest(file, text) {
  const problems = [];
  let table = null;
  let blockName = null;
  for (const [index, raw] of text.split("\n").entries()) {
    const line = stripComment(raw).trim();
    if (line === "") continue;
    const header = /^\[\[?([^\]]+)\]\]?$/.exec(line);
    if (header) {
      const name = header[1].trim();
      const match = DEPENDENCY_TABLE.exec(name);
      table = match ? "dependencies" : null;
      blockName = match ? (match[1] ?? null) : null;
      continue;
    }
    if (table !== "dependencies") continue;

    const entry = /^([A-Za-z0-9_.-]+|"[^"]*")\s*=\s*(.*)$/.exec(line);
    if (!entry) continue;
    const key = entry[1].replace(/^"|"$/g, "");
    const value = entry[2].trim();

    if (blockName) {
      // Inside `[dependencies.name]`, the keys are that one dependency's fields.
      if (key !== "version") continue;
      const version = /^"([^"]*)"/.exec(value)?.[1];
      if (version !== undefined && !version.startsWith("=")) {
        problems.push(`${file}:${index + 1} ${blockName} ${version}`);
      }
      continue;
    }

    // `name.workspace = true` and `name.path = ".."` inherit or point at a path.
    if (key.includes(".")) continue;

    if (value.startsWith('"')) {
      const version = /^"([^"]*)"/.exec(value)?.[1] ?? "";
      if (!version.startsWith("=")) problems.push(`${file}:${index + 1} ${key} ${version}`);
      continue;
    }
    if (!value.startsWith("{")) continue;
    if (field(value, "path") !== undefined) continue;
    if (field(value, "workspace") === "true") continue;
    const version = field(value, "version");
    if (version === undefined) continue;
    if (!version.startsWith("=")) problems.push(`${file}:${index + 1} ${key} ${version}`);
  }
  return problems;
}

export async function findLoosePins(root = repositoryRoot) {
  const { stdout } = await execFileAsync("git", ["ls-files", "*Cargo.toml"], { cwd: root });
  const files = stdout.split("\n").filter((entry) => entry !== "");
  const problems = [];
  for (const file of files) {
    problems.push(...findLoosePinsInManifest(file, await readFile(join(root, file), "utf8")));
  }
  return problems;
}

// The Documents body is one Loro document that the Rust Backend (`loro`) and the UI frame
// (`loro-crdt`) both edit, so the two must be the same release. The lockfiles are what a build
// resolves, so they are what is compared.
const LORO_PARITY = { cargo: "loro", npm: "loro-crdt" };

/** The version `Cargo.lock` resolves for `name`, or `null`. */
export function cargoLockVersion(text, name) {
  const lines = text.split("\n");
  for (let index = 0; index < lines.length - 1; index += 1) {
    if (lines[index].trim() === `name = "${name}"`) {
      return /^version = "([^"]*)"$/.exec(lines[index + 1].trim())?.[1] ?? null;
    }
  }
  return null;
}

/** The version `package-lock.json` resolves for the hoisted package `name`, or `null`. */
export function packageLockVersion(text, name) {
  return JSON.parse(text).packages?.[`node_modules/${name}`]?.version ?? null;
}

/** A problem when the two lockfiles resolve different Loro releases, else `null`. */
export function findLoroParityProblem(cargoLock, packageLock) {
  const cargo = cargoLockVersion(cargoLock, LORO_PARITY.cargo);
  const npm = packageLockVersion(packageLock, LORO_PARITY.npm);
  if (cargo !== null && cargo === npm) return null;
  return `Cargo.lock resolves ${LORO_PARITY.cargo} ${cargo ?? "(none)"} but package-lock.json resolves ${LORO_PARITY.npm} ${npm ?? "(none)"}; pin both to one version.`;
}

export async function findLoroParity(root = repositoryRoot) {
  return findLoroParityProblem(
    await readFile(join(root, "Cargo.lock"), "utf8"),
    await readFile(join(root, "package-lock.json"), "utf8"),
  );
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  Promise.all([findLoosePins(), findLoroParity()])
    .then(([problems, parity]) => {
      if (problems.length > 0) {
        console.error(
          `${problems.join("\n")}\nEvery Cargo dependency version must start with \`=\`. Pin the exact version the lockfile resolves.`,
        );
        process.exitCode = 1;
      } else {
        console.log("Every Cargo dependency is pinned to an exact version.");
      }
      if (parity !== null) {
        console.error(parity);
        process.exitCode = 1;
      } else {
        console.log("Rust loro and npm loro-crdt resolve the same version.");
      }
    })
    .catch((error) => {
      console.error(error.message);
      process.exitCode = 1;
    });
}
