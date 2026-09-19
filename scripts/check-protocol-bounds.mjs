import { execFile } from "node:child_process";
import { readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const scriptPath = fileURLToPath(import.meta.url);
export const repositoryRoot = resolve(dirname(scriptPath), "..");

// Every queue, buffer and window that carries contract-governed traffic is
// declared as an expression over the contract constant it serves, with a
// comment naming the worst case it must admit. A bare integer in a channel or
// semaphore constructor is a bound chosen without checking the constant, and
// that is how a lane sized for 16 came to carry the 32 answers the contract
// permits: the overflow was then reported as the peer having exited.
//
// A named `const` declared over a protocol constant satisfies the rule, so the
// derivation and its worst case are written down once and read at the
// constructor.
//
// The rule covers production source only. A test builds its own channels to
// shape one scenario, and those depths say nothing about the contract.
export const BOUNDED_CRATES = [
  "crates/teamarness-package-protocol/src/",
  "crates/teamarness-package-host/src/",
  "crates/teamarness-package-sdk/src/",
];

const CONSTRUCTORS = /\b(mpsc::channel|Semaphore::new)\s*(::<[^>]*>\s*)?\(\s*([^)]*)\)/g;

/** Whether the argument is a bare number rather than a named bound. */
function isBareLiteral(argument) {
  return /^[0-9][0-9_]*(\s*(\*|\+|-|\/)\s*[0-9][0-9_]*)*$/.test(argument.trim());
}

export function findBareBounds(file, text) {
  const problems = [];
  const lines = text.split("\n");
  for (const [index, line] of lines.entries()) {
    CONSTRUCTORS.lastIndex = 0;
    for (const match of line.matchAll(CONSTRUCTORS)) {
      if (!isBareLiteral(match[3])) continue;
      problems.push(
        `${file}:${index + 1} ${match[1]}(${match[3].trim()}) is a bare bound; declare a named const over the contract constant it serves, with a comment naming the worst case it must admit`,
      );
    }
  }
  return problems;
}

/** Reads one `pub const NAME: usize = <digits>;` out of a Rust source file. */
function rustConstant(text, name) {
  const match = new RegExp(`const\\s+${name}\\s*:\\s*usize\\s*=\\s*([0-9_]+)\\s*;`).exec(text);
  return match ? Number(match[1].replaceAll("_", "")) : null;
}

/** Reads one `export const NAME = <digits>;` out of a TypeScript source file. */
function typeScriptConstant(text, name) {
  const match = new RegExp(`export const ${name}\\s*=\\s*([0-9_]+)\\s*;`).exec(text);
  return match ? Number(match[1].replaceAll("_", "")) : null;
}

/**
 * The Shell asks for subscriptions the Package Host serves, so one bound is
 * published in two languages and neither may move alone.
 */
export async function checkSharedSubscriptionBound(root = repositoryRoot) {
  const hostPath = "crates/teamarness-package-host/src/ui.rs";
  const shellPath = "apps/teamarness-desktop/src/package-frame/protocol.ts";
  const host = rustConstant(
    await readFile(join(root, hostPath), "utf8"),
    "MAX_UI_SUBSCRIPTIONS_PER_MOUNT",
  );
  const shell = typeScriptConstant(
    await readFile(join(root, shellPath), "utf8"),
    "MAX_OPEN_SUBSCRIPTIONS",
  );
  if (host === null) return [`${hostPath} no longer declares MAX_UI_SUBSCRIPTIONS_PER_MOUNT`];
  if (shell === null) return [`${shellPath} no longer declares MAX_OPEN_SUBSCRIPTIONS`];
  if (host !== shell) {
    return [
      `${shellPath} admits ${shell} open subscriptions per mount while ${hostPath} serves ${host}; the extra subscriptions are accepted by the Shell and refused by the Host as \`overloaded\``,
    ];
  }
  return [];
}

/**
 * The Shell decides how often it acknowledges; the Package Host sizes its
 * control lane against that decision, so one behaviour is published in two
 * languages and neither may move alone.
 *
 * A bound derived from the protocol's vocabulary instead — "one `credit` frame
 * per subscription" — was wrong by a factor of 32 and every floor test agreed
 * with it, because the granularity that makes it wrong lived only in
 * TypeScript and the Rust side had no name for it.
 */
export async function checkSharedAcknowledgementGranularity(root = repositoryRoot) {
  const hostPath = "crates/teamarness-package-host/src/protocol.rs";
  const shellPath = "apps/teamarness-desktop/src/package-frame/protocol.ts";
  const host = rustConstant(
    await readFile(join(root, hostPath), "utf8"),
    "EVENTS_PER_ACKNOWLEDGEMENT",
  );
  const shell = typeScriptConstant(
    await readFile(join(root, shellPath), "utf8"),
    "EVENTS_PER_ACKNOWLEDGEMENT",
  );
  if (host === null) return [`${hostPath} no longer declares EVENTS_PER_ACKNOWLEDGEMENT`];
  if (shell === null) return [`${shellPath} no longer declares EVENTS_PER_ACKNOWLEDGEMENT`];
  if (host !== shell) {
    return [
      `${shellPath} acknowledges every ${shell} event while ${hostPath} sizes the control lane for every ${host}; the Host's \`CONTROL_WORST_CASE\` counts \`credit\` frames the Shell no longer sends at that rate`,
    ];
  }
  return [];
}

async function trackedFiles(root) {
  const { stdout } = await execFileAsync("git", ["ls-files", "*.rs"], { cwd: root });
  return stdout
    .split("\n")
    .filter((entry) => entry !== "" && BOUNDED_CRATES.some((crate) => entry.startsWith(crate)));
}

export async function checkProtocolBounds(root = repositoryRoot) {
  const problems = [];
  for (const file of await trackedFiles(root)) {
    problems.push(...findBareBounds(file, await readFile(join(root, file), "utf8")));
  }
  problems.push(...(await checkSharedSubscriptionBound(root)));
  problems.push(...(await checkSharedAcknowledgementGranularity(root)));
  return problems;
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  checkProtocolBounds()
    .then((problems) => {
      if (problems.length > 0) {
        console.error(problems.join("\n"));
        process.exitCode = 1;
      } else {
        console.log("Every protocol-facing bound is derived from the constant it serves.");
      }
    })
    .catch((error) => {
      console.error(error.message);
      process.exitCode = 1;
    });
}
