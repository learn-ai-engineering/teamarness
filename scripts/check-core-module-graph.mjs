import { execFile } from "node:child_process";
import { readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const scriptPath = fileURLToPath(import.meta.url);
export const repositoryRoot = resolve(dirname(scriptPath), "..");

export const CORE_SOURCE = "crates/teamarness-core/src";

// `network` owns the Endpoint and Router and nothing else: every protocol
// reaches it as an opaque registration from the runtime composition point.
// `onboarding` and `synchronization` are peers over it, neither above the
// other. Register C-12 puts the membership protocol in the crate-root module
// `membership_sync/` for the same reason, so `network` never names it either.
export const MODULE_RULES = [
  { directory: "network", forbidden: ["crate::onboarding", "crate::synchronization", "crate::membership", "crate::membership_sync"] },
  { directory: "synchronization", forbidden: ["crate::onboarding"] },
  { directory: "onboarding", forbidden: ["crate::synchronization"] },
];
export const TEST_HOOKS_ATTRIBUTE = '#[cfg(feature = "test-hooks")]';

// Production escape hatches that plan phase 8 removed from the supported API.
// The `test-hooks` feature exists so Host integration tests can observe Core,
// not so a demoted method can come back through a side door.
export const DEMOTED_METHODS = [
  "insert_remote_record",
  "replicate_records_to",
  "export_protection_grant",
  "import_protection_grant",
  "authenticate_membership_operation",
];

function stripComment(line) {
  let quote = null;
  for (let at = 0; at < line.length; at += 1) {
    const character = line[at];
    if (quote) {
      if (character === quote) quote = null;
    } else if (character === '"' || character === "'") {
      quote = character;
    } else if (character === "#" && line[at + 1] !== "[" && line[at + 1] !== "!") {
      return line.slice(0, at);
    }
  }
  return line;
}

// Which Cargo table a `teamarness-core` entry that enables `test-hooks` sits in.
// Only another crate's `[dev-dependencies]` may enable it: a normal dependency
// would ship the hooks in the application.
export function findTestHooksMisuse(file, text) {
  const problems = [];
  let table = null;
  for (const [index, raw] of text.split("\n").entries()) {
    const line = stripComment(raw).trim();
    if (line === "") continue;
    const header = /^\[\[?([^\]]+)\]\]?$/.exec(line);
    if (header) {
      table = header[1].trim();
      continue;
    }
    if (!line.includes("test-hooks")) continue;
    // The feature's own declaration in `[features]` is what the rule protects.
    if (table === "features") continue;
    if (table !== null && /(^|\.)dev-dependencies$/.test(table)) continue;
    problems.push(
      `${file}:${index + 1} the \`test-hooks\` feature is enabled from [${table ?? "?"}]; only a [dev-dependencies] entry may enable it`,
    );
  }
  return problems;
}

// The item that follows a `#[cfg(feature = "test-hooks")]` attribute, up to the
// end of its body, so a demoted method hidden inside a gated block is found too.
function gatedItems(text) {
  const items = [];
  let at = text.indexOf(TEST_HOOKS_ATTRIBUTE);
  while (at !== -1) {
    let cursor = at + TEST_HOOKS_ATTRIBUTE.length;
    let depth = 0;
    let seenBody = false;
    while (cursor < text.length) {
      const character = text[cursor];
      if (character === "{") {
        depth += 1;
        seenBody = true;
      } else if (character === "}") {
        depth -= 1;
        if (depth === 0) {
          cursor += 1;
          break;
        }
      } else if (character === ";" && depth === 0 && seenBody === false) {
        cursor += 1;
        break;
      }
      cursor += 1;
    }
    items.push({ start: at, text: text.slice(at, cursor) });
    at = text.indexOf(TEST_HOOKS_ATTRIBUTE, cursor);
  }
  return items;
}

export function findDemotedBehindTestHooks(file, text) {
  const problems = [];
  for (const item of gatedItems(text)) {
    const line = text.slice(0, item.start).split("\n").length;
    for (const method of DEMOTED_METHODS) {
      if (new RegExp(`\\b${method}\\b`).test(item.text)) {
        problems.push(
          `${file}:${line} \`${method}\` is demoted production API and must not be reachable through the \`test-hooks\` feature`,
        );
      }
    }
  }
  return problems;
}

// Everything from the first `#[cfg(test)]`-gated module to the end of the file
// is test code, which the module rules do not police.
function productionLines(text) {
  const lines = text.split("\n");
  const end = lines.findIndex(
    (line, index) =>
      /^\s*mod tests \{/.test(line) && /^\s*#\[cfg\(test\)\]/.test(lines[index - 1] ?? ""),
  );
  return end === -1 ? lines : lines.slice(0, end - 1);
}

export function findModuleGraphBreaks(directory, file, text, forbidden) {
  const problems = [];
  for (const [index, line] of productionLines(text).entries()) {
    for (const name of forbidden) {
      if (line.includes(name)) {
        problems.push(
          `${file}:${index + 1} \`${directory}\` must not name \`${name}\`; it is a leaf or a peer of that module`,
        );
      }
    }
  }
  return problems;
}

async function trackedFiles(root, pattern) {
  const { stdout } = await execFileAsync("git", ["ls-files", pattern], { cwd: root });
  return stdout.split("\n").filter((entry) => entry !== "");
}

export async function checkCoreModuleGraph(root = repositoryRoot) {
  const problems = [];
  for (const file of await trackedFiles(root, "*Cargo.toml")) {
    problems.push(...findTestHooksMisuse(file, await readFile(join(root, file), "utf8")));
  }
  for (const file of await trackedFiles(root, `${CORE_SOURCE}/*.rs`)) {
    problems.push(...findDemotedBehindTestHooks(file, await readFile(join(root, file), "utf8")));
  }
  for (const { directory, forbidden } of MODULE_RULES) {
    for (const file of await trackedFiles(root, `${CORE_SOURCE}/${directory}/*.rs`)) {
      if (/(^|\/)tests\//.test(file)) continue;
      problems.push(
        ...findModuleGraphBreaks(directory, file, await readFile(join(root, file), "utf8"), forbidden),
      );
    }
  }
  return problems;
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  checkCoreModuleGraph()
    .then((problems) => {
      if (problems.length > 0) {
        console.error(problems.join("\n"));
        process.exitCode = 1;
      } else {
        console.log("The core module graph and test-hooks rules hold.");
      }
    })
    .catch((error) => {
      console.error(error.message);
      process.exitCode = 1;
    });
}
