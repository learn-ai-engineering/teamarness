// Runs the Package Host's admission rules over every bundled Package, through the same
// `teamarness-package-validate` binary a Package author runs.
import { spawn } from "node:child_process";
import { stat } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { STAGING_ROOT, readBundledPackages, repositoryRoot } from "./bundled-packages.mjs";

const scriptPath = fileURLToPath(import.meta.url);

export async function validateBundledPackages({
  repositoryRoot: root = repositoryRoot,
  stagedRoot = STAGING_ROOT,
  smoke = true,
  packages,
} = {}) {
  const entries = packages ?? (await readBundledPackages(root));
  const failures = [];
  const ran = [];
  for (const { identity, sourceDirectory } of entries) {
    const source = await runValidator(root, ["source", resolve(root, sourceDirectory)]);
    ran.push({ identity, mode: "source", notes: source.notes });
    report(identity, "source", source);
    if (!source.ok) failures.push(describe(identity, "source", source));

    const directory = resolve(root, stagedRoot, identity);
    if (!(await isDirectory(directory))) continue;
    const staged = await runValidator(root, [
      "directory",
      directory,
      ...(smoke ? ["--smoke"] : []),
    ]);
    ran.push({ identity, mode: "directory", notes: staged.notes });
    report(identity, "directory", staged);
    if (!staged.ok) failures.push(describe(identity, "directory", staged));
  }
  if (failures.length > 0) {
    throw new Error(`Package validation failed:\n${failures.join("\n")}`);
  }
  return ran;
}

function report(identity, mode, result) {
  for (const note of result.notes) console.log(`${identity} (${mode}): ${note}`);
}

function describe(identity, mode, result) {
  const lines = result.errors.map(
    ({ code, rule, file, pointer }) => `  ${file}${pointer ? `#${pointer}` : ""}: ${code}/${rule}`,
  );
  if (lines.length === 0) lines.push(`  ${result.stderr.trim() || "the validator exited non-zero"}`);
  return `${identity} (${mode}):\n${lines.join("\n")}`;
}

async function isDirectory(path) {
  return stat(path).then(
    (entry) => entry.isDirectory(),
    () => false,
  );
}

function runValidator(root, validatorArguments) {
  const cargo = [
    "run",
    "--quiet",
    "--locked",
    "-p",
    "teamarness-package-contract",
    "--bin",
    "teamarness-package-validate",
    "--",
    ...validatorArguments,
    "--json",
  ];
  return new Promise((resolveResult, rejectResult) => {
    const child = spawn("cargo", cargo, { cwd: root, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("error", rejectResult);
    child.on("close", (code) => {
      let report;
      try {
        report = JSON.parse(stdout);
      } catch {
        report = { ok: code === 0, errors: [], notes: [] };
      }
      resolveResult({ ok: code === 0 && report.ok !== false, errors: report.errors ?? [], notes: report.notes ?? [], stderr });
    });
  });
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  validateBundledPackages()
    .then((ran) => {
      console.log(`Validated ${ran.length} Package check(s) against the host's admission rules.`);
    })
    .catch((error) => {
      console.error(error.message);
      process.exitCode = 1;
    });
}
