import { readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptPath = fileURLToPath(import.meta.url);
export const repositoryRoot = resolve(dirname(scriptPath), "..");
export const BASELINE_PATH = "scripts/error-context-baseline.json";

// A closure that takes `|_|` throws the cause away. At a storage, network or
// startup boundary the cause is the only thing that says what actually failed,
// so those files run it through `fault!` instead, which writes one redacted
// line before returning the same public code.
//
// This is a ratchet, not a ban: parser and validation code deliberately redacts
// untrusted input, and `membership/projection.rs` is out of scope by design.
// The rule is only that the scoped files never erase more causes than they do
// today.
const ERASURE = /\.map_err\(\|_\|/g;

export function countErasures(text) {
  return (text.match(ERASURE) ?? []).length;
}

/**
 * Compares each scoped file's erasure count with the recorded baseline.
 *
 * A count above its baseline fails. A count below it also fails, with the
 * instruction to lower the baseline in the same commit, so the ratchet only
 * ever tightens. A scoped file that no longer exists fails until its entry is
 * removed.
 */
export function compareCounts(baseline, counts) {
  const problems = [];
  for (const [file, allowed] of Object.entries(baseline)) {
    const actual = counts[file];
    if (actual === undefined) {
      problems.push(
        `${file} is in ${BASELINE_PATH} but is not in the repository; remove its entry in the commit that removes the file`,
      );
      continue;
    }
    if (actual > allowed) {
      problems.push(
        `${file} erases ${actual} causes with \`map_err(|_|\`, above its baseline of ${allowed}; use \`fault!\` at storage, network and startup boundaries`,
      );
    } else if (actual < allowed) {
      problems.push(
        `${file} erases ${actual} causes, below its baseline of ${allowed}; lower the baseline in ${BASELINE_PATH} in the same commit`,
      );
    }
  }
  return problems;
}

export async function checkErrorContext(root = repositoryRoot) {
  const baseline = JSON.parse(await readFile(join(root, BASELINE_PATH), "utf8"));
  const counts = {};
  for (const file of Object.keys(baseline)) {
    let text;
    try {
      text = await readFile(join(root, file), "utf8");
    } catch {
      continue;
    }
    counts[file] = countErasures(text);
  }
  return compareCounts(baseline, counts);
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  checkErrorContext()
    .then((problems) => {
      if (problems.length > 0) {
        console.error(problems.join("\n"));
        process.exitCode = 1;
      } else {
        console.log("Error context at the scoped boundaries holds its baseline.");
      }
    })
    .catch((error) => {
      console.error(error.message);
      process.exitCode = 1;
    });
}
