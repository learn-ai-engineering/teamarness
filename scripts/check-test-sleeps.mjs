import { execFile } from "node:child_process";
import { readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const scriptPath = fileURLToPath(import.meta.url);
export const repositoryRoot = resolve(dirname(scriptPath), "..");

// A fixed sleep before an assertion is a guess about how fast the machine is.
// It passes on a quiet laptop and fails in a loaded suite, and the failure
// looks like a product bug. A test waits for the event it is about instead.
//
// Four sleeps are legitimate and this rule names each of them:
//  - inside a waiting helper, which is how an event-anchored wait is built;
//  - `// poll-interval:` the interval of an inline polling loop that is bounded
//    by its own deadline;
//  - `// negative-wait:` a bounded window for proving something did NOT happen,
//    or a backoff between retries, neither of which has an event to wait on;
//  - `// settle:` letting work already in flight reach the point the test acts
//    on, where the seam under test exposes no event for it;
//  - `// simulated-delay:` a delay inside a fixture standing in for slow work,
//    which is the subject of the test rather than a guess about timing.
export const WAITING_HELPERS = /^(eventually|poll_\w*|wait_\w*|.*_until|.*_within)$/;
export const MARKERS = [
  "// poll-interval:",
  "// negative-wait:",
  "// settle:",
  "// simulated-delay:",
];

// A Rust file under a `tests/` or `*_tests/` directory is test code.
export function isTestFile(file) {
  if (!file.endsWith(".rs")) return false;
  return file
    .split("/")
    .slice(0, -1)
    .some((segment) => segment === "tests" || segment.endsWith("_tests"));
}

function enclosingFunction(lines, at) {
  for (let index = at; index >= 0; index -= 1) {
    const match = /\bfn\s+([A-Za-z_][A-Za-z0-9_]*)/.exec(lines[index]);
    if (match) return match[1];
  }
  return null;
}

export function findBareSleeps(file, text) {
  const problems = [];
  const lines = text.split("\n");
  for (const [index, line] of lines.entries()) {
    if (!/(^|[^A-Za-z0-9_])sleep\s*\(/.test(line)) continue;
    const helper = enclosingFunction(lines, index);
    if (helper !== null && WAITING_HELPERS.test(helper)) continue;
    const nearby = lines.slice(Math.max(0, index - 2), index + 3).join("\n");
    if (MARKERS.some((marker) => nearby.includes(marker))) continue;
    problems.push(
      `${file}:${index + 1} a fixed sleep in a test must be inside a waiting helper (eventually/poll_*/wait_*), or carry a \`// negative-wait:\` or \`// simulated-delay:\` comment saying why a fixed window is inherent`,
    );
  }
  return problems;
}

async function trackedFiles(root) {
  const { stdout } = await execFileAsync("git", ["ls-files", "*.rs"], { cwd: root });
  return stdout.split("\n").filter((entry) => entry !== "" && isTestFile(entry));
}

export async function checkTestSleeps(root = repositoryRoot) {
  const problems = [];
  for (const file of await trackedFiles(root)) {
    problems.push(...findBareSleeps(file, await readFile(join(root, file), "utf8")));
  }
  return problems;
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  checkTestSleeps()
    .then((problems) => {
      if (problems.length > 0) {
        console.error(problems.join("\n"));
        process.exitCode = 1;
      } else {
        console.log("Every sleep in a test is event-anchored or explained.");
      }
    })
    .catch((error) => {
      console.error(error.message);
      process.exitCode = 1;
    });
}
