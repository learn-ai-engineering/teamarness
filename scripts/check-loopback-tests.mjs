import { execFile } from "node:child_process";
import { readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const scriptPath = fileURLToPath(import.meta.url);
export const repositoryRoot = resolve(dirname(scriptPath), "..");

// `cargo test --workspace` is a required check, so nothing it runs may reach
// the public internet. `CoreRuntime::open` binds the production way in a test
// build too — the loopback branch is gated on the readiness hook, not on
// `cfg(test)` — so every test that called it published its identity and
// addresses to n0's pkarr relay over HTTPS and dialled four public relays.
// `CoreRuntime::open_loopback_for_tests` is the same runtime with loopback
// addressing and no address-lookup service: real storage, real Endpoint and
// Router, real onboarding and synchronization, nothing off this machine.
//
// Two exceptions, each of which must say so on the line:
//  - `// production-addressing:` on the line, or in the two lines above it,
//    names the one seam that deliberately opens the production way, for a
//    scenario that is `#[ignore]`d and runs outside the required checks;
//  - a compile-fail fixture under `tests/ui/`, which is never executed and
//    whose text is the public API surface being asserted.
export const PRODUCTION_MARKER = "// production-addressing:";
export const PRODUCTION_OPEN = "CoreRuntime::open(";

/** A compile-fail fixture is source for `trybuild`, not a test that runs. */
export function isCompileFailFixture(file) {
  return file.split("/").includes("ui") && file.includes("/tests/");
}

// A Rust file under a `tests/` or `*_tests/` directory is test code.
function isTestFile(file) {
  return file
    .split("/")
    .slice(0, -1)
    .some((segment) => segment === "tests" || segment.endsWith("_tests"));
}

/** Whether this line of a production source file sits in a `#[cfg(test)]` block. */
function inTestModule(lines, at) {
  for (let index = at; index >= 0; index -= 1) {
    if (lines[index].trim() === "#[cfg(test)]") return true;
  }
  return false;
}

/** Whether the function enclosing `at` carries `#[ignore]`. */
function enclosingFunctionIsIgnored(lines, at) {
  for (let index = at; index >= 0; index -= 1) {
    if (!/\bfn\s+[A-Za-z_][A-Za-z0-9_]*/.test(lines[index])) continue;
    for (let above = index - 1; above >= 0; above -= 1) {
      const line = lines[above].trim();
      if (line.startsWith("#[ignore")) return true;
      if (line.startsWith("#[") || line.startsWith("///") || line.startsWith("//")) continue;
      return false;
    }
    return false;
  }
  return false;
}

export function findProductionBindings(file, text) {
  if (isCompileFailFixture(file)) return [];
  const problems = [];
  const lines = text.split("\n");
  const testFile = isTestFile(file);
  for (const [index, line] of lines.entries()) {
    if (!line.includes(PRODUCTION_OPEN)) continue;
    const nearby = lines.slice(Math.max(0, index - 2), index + 1).join("\n");
    if (nearby.includes(PRODUCTION_MARKER)) continue;
    if (!testFile && !inTestModule(lines, index)) continue;
    if (enclosingFunctionIsIgnored(lines, index)) continue;
    problems.push(
      `${file}:${index + 1} a test Installation opens with \`CoreRuntime::open\`, which binds public relays and publishes to n0's pkarr relay; open it with \`CoreRuntime::open_loopback_for_tests\`, or mark the line \`${PRODUCTION_MARKER}\` and \`#[ignore]\` the scenario that uses it`,
    );
  }
  return problems;
}

async function trackedFiles(root) {
  const { stdout } = await execFileAsync("git", ["ls-files", "*.rs"], { cwd: root });
  return stdout.split("\n").filter((entry) => entry !== "");
}

export async function checkLoopbackTests(root = repositoryRoot) {
  const problems = [];
  for (const file of await trackedFiles(root)) {
    problems.push(...findProductionBindings(file, await readFile(join(root, file), "utf8")));
  }
  return problems;
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  checkLoopbackTests()
    .then((problems) => {
      if (problems.length > 0) {
        console.error(problems.join("\n"));
        process.exitCode = 1;
      } else {
        console.log("Every test Installation binds loopback addressing.");
      }
    })
    .catch((error) => {
      console.error(error.message);
      process.exitCode = 1;
    });
}
