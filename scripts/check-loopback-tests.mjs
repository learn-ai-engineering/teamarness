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
// Two checks, because a spelling is not a property:
//
//  - **Reachability.** No test that the required checks run may reach
//    production addressing. `Addressing::ProductionRelay` is the system
//    harness's name for it, and a test reaches it directly or through helpers
//    in any tracked file. This is the guard that matters: a substring scan over
//    `CoreRuntime::open(` was evaded eleven of twelve ways, including by a
//    `#[tokio::test]` with no `#[ignore]` that bound production relays through
//    the one helper the marker exempts.
//  - **Spelling.** `CoreRuntime::open(` outside the reachability seam, as a
//    cheap second net. It tolerates whitespace inside the path, ignores what
//    is inside a `//` comment, and reads `#[cfg(all(test, ...))]` as a test
//    module.
//
// Two exceptions to the spelling check, each of which must say so on the line:
//  - `// production-addressing:` on the line, or in the two lines above it,
//    names the one seam that deliberately opens the production way. The seam
//    is an ordinary helper, so its `#[ignore]` discipline lives in its callers
//    — which is exactly what the reachability check reads;
//  - a compile-fail fixture under `tests/ui/`, which is never executed and
//    whose text is the public API surface being asserted.
//
// One evasion is left open on purpose: a type alias (`type Runtime =
// CoreRuntime; Runtime::open(..)`) defeats any spelling, which is the whole
// argument for reading reachability instead. Such an alias still has to choose
// `Addressing::ProductionRelay` to reach a relay, and the reachability check
// reads that.
export const PRODUCTION_MARKER = "// production-addressing:";
export const PRODUCTION_ADDRESSING = "Addressing::ProductionRelay";
/**
 * Choosing production addressing, rather than dispatching on it.
 *
 * The seam helper carries `Addressing::ProductionRelay =>` as a match arm and
 * serves loopback callers from the same body, so the arm is not a choice. What
 * taints is naming the variant as a value.
 */
const PRODUCTION_CHOICE = /Addressing::ProductionRelay(?!\s*=>)/;

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

/** Blanks every `//` comment while keeping each byte at its own offset. */
function withoutComments(text) {
  return text.replace(/\/\/[^\n]*/g, (match) => " ".repeat(match.length));
}

/** The 1-based line holding `offset`. */
function lineAt(text, offset) {
  let line = 1;
  for (let index = 0; index < offset; index += 1) if (text[index] === "\n") line += 1;
  return line;
}

/** Whether an attribute line is a `cfg` that selects test builds. */
function isTestCfg(line) {
  const match = /^#\[cfg\((.*)\)\]$/.exec(line.replace(/\/\/.*$/, "").trim());
  return match !== null && /\btest\b/.test(match[1]);
}

/** Whether this line of a production source file sits in a test-only module. */
function inTestModule(lines, at) {
  for (let index = at; index >= 0; index -= 1) {
    if (isTestCfg(lines[index])) return true;
  }
  return false;
}

/**
 * Every function in one Rust file, with the attributes above it and the span
 * of its body.
 *
 * Brace counting is enough here: the guard reads which function a line belongs
 * to and which functions a function names, neither of which needs a parser.
 */
export function functionsOf(text) {
  const scannable = withoutComments(text);
  const lines = scannable.split("\n");
  const functions = [];
  const signature = /^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+"[^"]*"\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)/;
  for (const [index, line] of lines.entries()) {
    const match = signature.exec(line);
    if (match === null) continue;
    const attributes = [];
    for (let above = index - 1; above >= 0; above -= 1) {
      const trimmed = lines[above].trim();
      if (trimmed === "" || trimmed.startsWith("///") || trimmed.startsWith("//")) continue;
      if (!trimmed.startsWith("#[")) break;
      attributes.push(trimmed);
    }
    let depth = 0;
    let started = false;
    let end = index;
    for (let below = index; below < lines.length; below += 1) {
      for (const character of lines[below]) {
        if (character === "{") { depth += 1; started = true; }
        else if (character === "}") depth -= 1;
      }
      end = below;
      if (started && depth <= 0) break;
      // A signature that ends in `;` is a trait method with no body.
      if (!started && /;\s*$/.test(lines[below])) break;
    }
    // The marker is read from the original source: `lines` has had its
    // comments blanked, and the marker is a comment.
    const raw = text.split("\n").slice(index, end + 1).join("\n");
    functions.push({
      name: match[1],
      line: index + 1,
      endLine: end + 1,
      attributes,
      body: lines.slice(index, end + 1).join("\n"),
      marked: raw.includes(PRODUCTION_MARKER),
    });
  }
  return functions;
}

const TEST_ATTRIBUTE = /^#\[(?:tokio::)?test\b/;

/**
 * Tests the required checks run that can reach production addressing.
 *
 * A function is tainted when its body names `Addressing::ProductionRelay` or
 * carries the `production-addressing` seam marker, or when it calls a tainted
 * function declared anywhere in the corpus, taken to a fixed point. A tainted
 * `#[test]` without `#[ignore]` reaches the public internet from
 * `cargo test --workspace`.
 *
 * `sources` is `[file, text]` pairs, because the seam and the tests that reach
 * it live in different files — which is exactly why reading the seam's own
 * `#[ignore]` attribute said nothing. The seeds only ever grow where the
 * production choice is spelled, but the caller walk runs over every tracked
 * `.rs` file: a test reaches the seam through helpers of its own, and the file
 * holding such a test names no addressing at all — that gap was an executed
 * evasion. A wider walk can collide two unrelated functions of the same name,
 * which over-taints and reports a caller the walk need not have reached; that
 * is the safe direction for a guard whose failures otherwise end in a test
 * run that leaves this machine.
 */
export function findProductionReachability(sources) {
  const functions = [];
  for (const [file, text] of sources) {
    if (isCompileFailFixture(file)) continue;
    for (const entry of functionsOf(text)) functions.push({ ...entry, file });
  }
  // The seam helper itself is not a seed: it dispatches on its argument, so
  // tainting it would report every loopback caller too. What taints is naming
  // the production variant, which is where the choice is actually made.
  const tainted = new Set(
    functions
      .filter((entry) => PRODUCTION_CHOICE.test(entry.body))
      .map((entry) => entry.name),
  );
  for (let pass = 0; pass < functions.length; pass += 1) {
    const before = tainted.size;
    const callers = [...tainted].map(
      (name) => new RegExp(`\\b${name}\\s*(::<[^>]*>\\s*)?\\(`),
    );
    for (const entry of functions) {
      if (tainted.has(entry.name)) continue;
      if (callers.some((pattern) => pattern.test(entry.body))) tainted.add(entry.name);
    }
    if (tainted.size === before) break;
  }
  return functions
    .filter(
      (entry) =>
        tainted.has(entry.name) &&
        entry.attributes.some((attribute) => TEST_ATTRIBUTE.test(attribute)) &&
        !entry.attributes.some((attribute) => attribute.startsWith("#[ignore")),
    )
    .map(
      (entry) =>
        `${entry.file}:${entry.line} \`${entry.name}\` runs in the required checks and reaches production addressing, which binds public relays and publishes to n0's pkarr relay; use \`Addressing::Loopback\`, or \`#[ignore]\` the scenario so it runs outside them`,
    );
}

export function findProductionBindings(file, text) {
  if (isCompileFailFixture(file)) return [];
  const problems = [];
  const lines = text.split("\n");
  const scannable = withoutComments(text);
  const testFile = isTestFile(file);
  // `Self::open(` is the same call inside the runtime's own impl block. It is
  // read only where such a block exists, so unrelated `Self::open(` methods
  // elsewhere are not false positives.
  const selfCounts = /impl\s+CoreRuntime\b/.test(text);
  const pattern = /\b(CoreRuntime|Self)\s*::\s*open\s*\(/g;
  const functions = functionsOf(text);
  for (const match of scannable.matchAll(pattern)) {
    if (match[1] === "Self" && !selfCounts) continue;
    const index = lineAt(scannable, match.index) - 1;
    // The marker is read from the original text: blanking comments removed it.
    const nearby = lines.slice(Math.max(0, index - 2), index + 1).join("\n");
    // The marker excuses the seam, not any line that carries it. The seam is
    // the function that dispatches on `Addressing::ProductionRelay`, because
    // that is the one the reachability check can follow to its callers. A
    // marker on a helper that dispatches on nothing was a free pass with no
    // `#[ignore]` anywhere, which is how the exemption was evaded twice.
    const enclosing = functions.find(
      (entry) => entry.line <= index + 1 && index + 1 <= entry.endLine,
    );
    const isSeam = enclosing !== undefined && enclosing.body.includes(PRODUCTION_ADDRESSING);
    if (nearby.includes(PRODUCTION_MARKER) && isSeam) continue;
    if (!testFile && !inTestModule(lines, index)) continue;
    problems.push(
      `${file}:${index + 1} a test Installation opens with \`CoreRuntime::open\`, which binds public relays and publishes to n0's pkarr relay; open it with \`CoreRuntime::open_loopback_for_tests\`, or route it through the \`${PRODUCTION_MARKER}\` seam, whose callers the reachability check reads`,
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
  const sources = [];
  for (const file of await trackedFiles(root)) {
    const text = await readFile(join(root, file), "utf8");
    // Every tracked file feeds the reachability walk, not only the files
    // that spell addressing: a test that reaches the seam through a helper
    // can live in a file that never names `Addressing`, and only the seed
    // — the production choice itself — needs the spelling.
    sources.push([file, text]);
    problems.push(...findProductionBindings(file, text));
  }
  problems.push(...findProductionReachability(sources));
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
