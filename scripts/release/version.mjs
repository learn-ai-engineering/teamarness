import { readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { repositoryRoot } from "../bundled-packages.mjs";

export const VERSION_FILES = [
  "apps/teamarness-desktop/src-tauri/tauri.conf.json",
  "apps/teamarness-desktop/src-tauri/Cargo.toml",
  "apps/teamarness-desktop/package.json",
  "Cargo.lock",
  "package-lock.json",
];

export function stableVersion(value) {
  if (typeof value !== "string" || !/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(value)
    || value.split(".").some((part) => !Number.isSafeInteger(Number(part)))) {
    throw new Error("Expected canonical stable MAJOR.MINOR.PATCH version");
  }
  return value;
}

export function tagVersion(tag) {
  if (typeof tag !== "string" || !tag.startsWith("v")) throw new Error("Expected vMAJOR.MINOR.PATCH tag");
  return stableVersion(tag.slice(1));
}

// Scan only JSON property locations after JSON.parse validates syntax. Unlike JSON.parse,
// this retains duplicate keys and exact byte spans, so a setter cannot leave hidden drift.
function jsonVersionEntry(file, text, path) {
  const tokens = [...text.matchAll(/"(?:\\.|[^"\\])*"|[{}\[\]:,]|[^\s{}\[\]:,]+/g)];
  const stack = [];
  const matches = [];
  for (let at = 0; at < tokens.length; at++) {
    const token = tokens[at];
    const parent = stack.at(-1);
    if (token[0] === "{" || token[0] === "[") {
      stack.push({ path: parent ? [...parent.path, parent.key ?? "*"] : [] });
      if (parent) parent.key = undefined;
    } else if (token[0] === "}" || token[0] === "]") stack.pop();
    else if (token[0] === ",") parent.key = undefined;
    else if (tokens[at + 1]?.[0] === ":") {
      parent.key = JSON.parse(token[0]);
      at++;
    } else if (parent?.key !== undefined) {
      const location = [...parent.path, parent.key];
      if (location.length === path.length && location.every((part, index) => part === path[index])) matches.push(token);
      parent.key = undefined;
    }
  }
  if (matches.length !== 1) throw new Error(`${file}: expected exactly one desktop version`);
  const [match] = matches;
  const value = stableVersion(JSON.parse(match[0]));
  if (match[0] !== JSON.stringify(value)) throw new Error(`${file}: expected literal canonical version`);
  return { value, start: match.index + 1, end: match.index + match[0].length - 1 };
}

// Match a single desktop-owned table/object, never a global version replacement.
function versionEntry(file, text) {
  let block;
  if (file.endsWith("Cargo.toml")) {
    if ([...text.matchAll(/^\[package\]$/gm)].length !== 1) throw new Error(`${file}: expected one desktop version table`);
    block = /^\[package\]\r?\n([\s\S]*?)(?=^\[|$(?![\s\S]))/m.exec(text);
  }
  else if (file === "Cargo.lock") {
    const blocks = [...text.matchAll(/^\[\[package\]\]\r?\nname = "teamarness-desktop"\r?\n[\s\S]*?(?=^\[\[package\]\]|$(?![\s\S]))/gm)];
    if (blocks.length !== 1) throw new Error(`${file}: expected one desktop version table`);
    [block] = blocks;
  } else if (file === "package-lock.json") {
    // npm v3's workspace entry is authoritative; its node_modules entry is a link.
    const document = JSON.parse(text);
    if (document.lockfileVersion !== 3 || document.packages?.["node_modules/teamarness-desktop"]?.link !== true
      || document.packages["node_modules/teamarness-desktop"].resolved !== "apps/teamarness-desktop") throw new Error("Unsupported npm lockfile desktop version layout");
    return jsonVersionEntry(file, text, ["packages", "apps/teamarness-desktop", "version"]);
  } else {
    const document = JSON.parse(text);
    if (file.endsWith("package.json") && document.name !== "teamarness-desktop") throw new Error(`${file}: unexpected desktop identity`);
    return jsonVersionEntry(file, text, ["version"]);
  }
  if (!block) throw new Error(`${file}: missing desktop version block`);
  const pattern = file.endsWith(".toml") || file === "Cargo.lock" ? /^version = "([^"]+)"/gm : /"version"\s*:\s*"([^"]+)"/g;
  const matches = [...block[0].matchAll(pattern)];
  if (matches.length !== 1) throw new Error(`${file}: expected exactly one desktop version`);
  const [match] = matches;
  const value = stableVersion(match[1]);
  const start = block.index + match.index + match[0].lastIndexOf(value);
  return { value, start, end: start + value.length };
}

async function entries(root) {
  return Promise.all(VERSION_FILES.map(async (file) => {
    const text = await readFile(join(root, file), "utf8");
    return { file, text, ...versionEntry(file, text) };
  }));
}

export async function checkVersion({ root = repositoryRoot, tag } = {}) {
  const values = await entries(root);
  const expected = tag === undefined ? values[0].value : tagVersion(tag);
  for (const { file, value } of values) if (value !== expected) throw new Error(`${file}: desktop version ${value} differs from ${expected}`);
  return expected;
}

export async function setVersion(version, { root = repositoryRoot } = {}) {
  stableVersion(version);
  const values = await entries(root); // Validate every occurrence before changing anything.
  for (const { file, text, start, end } of values) await writeFile(join(root, file), text.slice(0, start) + version + text.slice(end));
  await checkVersion({ root, tag: `v${version}` });
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [command, ...args] = process.argv.slice(2);
  const operation = command === "check" && args.length === 2 && args[0] === "--tag"
    ? checkVersion({ tag: args[1] }) : command === "set" && args.length === 1 ? setVersion(args[0]) : undefined;
  if (!operation) { console.error("Usage: node scripts/release/version.mjs check --tag vX.Y.Z | set X.Y.Z"); process.exitCode = 2; }
  else operation.then(() => console.log("Desktop versions agree.")).catch((error) => { console.error(error.message); process.exitCode = 1; });
}
