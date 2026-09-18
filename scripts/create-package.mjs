import { execFileSync } from "node:child_process";
import { cp, lstat, mkdtemp, readdir, readFile, rename, rm, stat, writeFile } from "node:fs/promises";
import { basename, dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { VENDOR_TREES, vendorTreeDigest } from "./vendor-digest.mjs";

export const FOUNDATION_TEMPLATE_VERSION = 2;
export const PACKAGE_IDENTITY_PATTERN = /^[a-z](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\.[a-z](?:[a-z0-9-]{0,61}[a-z0-9])?)+$/;

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
export const repositoryRoot = dirname(scriptDirectory);
export const templateDirectory = join(repositoryRoot, "templates/package-foundation/v1");
/**
 * What a generated project vendors, and where each tree comes from.
 *
 * Tests are not vendored: they exercise the SDK, not the project, and a generated project's own
 * checks are about its own Package.
 */
export const canonicalSources = {
  protocol: { from: "crates/teamarness-package-protocol/src", to: "package-protocol/src" },
  backendSdk: { from: "crates/teamarness-package-sdk/src", to: "package-sdk/src" },
  packageContract: { from: "crates/teamarness-package-contract/src", to: "package-contract/src" },
  uiSdk: { from: "packages/teamarness-package-ui-sdk/src", to: "package-ui-sdk/src" },
  uiSdkBin: { from: "packages/teamarness-package-ui-sdk/bin", to: "package-ui-sdk/bin" },
};

/** The checkout paths whose state decides whether a recorded revision is dirty. */
const provenancePaths = Object.values(canonicalSources)
  .map(({ from }) => dirname(from))
  .filter((path, index, all) => all.indexOf(path) === index);

function isTestFile(path) {
  return path.endsWith(".test.ts") || path.endsWith(".test.mjs");
}
const textExtensions = new Set([".json", ".js", ".mjs", ".ts", ".tsx", ".rs", ".toml", ".md", ".html", ".css", ""]);

export function parseArgs(argv) {
  if (argv[0] === "--vendor-report") {
    const project = argv[1];
    if (argv.length !== 2 || !project || project.startsWith("--")) {
      throw new Error("Missing value for --vendor-report");
    }
    return { vendorReport: resolve(project) };
  }
  const values = {};
  const allowed = new Set(["--output", "--identity", "--display-name"]);
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    if (!allowed.has(flag)) throw new Error(`Unknown argument: ${flag}`);
    if (values[flag]) throw new Error(`Duplicate argument: ${flag}`);
    const value = argv[index + 1];
    if (!value || value.startsWith("--")) throw new Error(`Missing value for ${flag}`);
    values[flag] = value;
    index += 1;
  }
  for (const flag of allowed) if (!values[flag]) throw new Error(`Missing required argument: ${flag}`);
  return {
    output: resolve(values["--output"]),
    identity: values["--identity"],
    displayName: values["--display-name"],
  };
}

export function validateArguments({ output, identity, displayName }) {
  if (identity.length > 128 || !PACKAGE_IDENTITY_PATTERN.test(identity)) {
    throw new Error("identity must match the Package Identity grammar and contain at most 128 characters");
  }
  if (
    typeof displayName !== "string"
    || !displayName.isWellFormed()
    || displayName.length === 0
    || displayName.trim() !== displayName
    || /\p{Cc}/u.test(displayName)
    || Buffer.byteLength(displayName, "utf8") > 128
  ) {
    throw new Error("display-name must be trimmed valid Unicode without control characters and contain 1 to 128 UTF-8 bytes");
  }
  if (!output) throw new Error("output is required");
}

export async function inspectDestination(output) {
  try {
    const info = await lstat(output);
    if (info.isSymbolicLink()) throw new Error("output destination may not be a symlink");
    if (!info.isDirectory()) throw new Error("output destination must be absent or an empty directory");
    const entries = await readdir(output);
    if (entries.length !== 0) throw new Error("output destination must be absent or an empty directory");
    return "empty";
  } catch (error) {
    if (error?.code === "ENOENT") return "absent";
    throw error;
  }
}

export async function createPackageProject(options) {
  const args = {
    output: resolve(options.output),
    identity: options.identity,
    displayName: options.displayName,
  };
  validateArguments(args);
  const destinationState = await inspectDestination(args.output);
  const parent = dirname(args.output);
  const temporary = await mkdtemp(join(parent, ".teamarness-package-"));
  try {
    await copyTemplate(temporary, args);
    await copyCanonicalSdkSources(temporary);
    await recordProvenance(temporary);
    if (destinationState === "empty") {
      const currentEntries = await readdir(args.output);
      if (currentEntries.length !== 0) throw new Error("output destination changed while generating");
      await rm(args.output, { recursive: true, force: true });
    }
    await rename(temporary, args.output);
    return args.output;
  } catch (error) {
    await rm(temporary, { recursive: true, force: true });
    throw error;
  }
}

async function copyTemplate(destination, args) {
  await cp(templateDirectory, destination, { recursive: true, force: true });
  const namespace = args.identity.split(".").at(-1);
  const serializedDisplayName = JSON.stringify(args.displayName);
  await replaceTokens(destination, {
    __PACKAGE_IDENTITY__: args.identity,
    __PACKAGE_DISPLAY_NAME_MARKDOWN__: args.displayName,
    __PACKAGE_DISPLAY_NAME_JSON__: serializedDisplayName,
    __PACKAGE_DISPLAY_NAME_RUST__: serializedDisplayName,
    __REQUEST_NAMESPACE__: namespace,
  });
}

async function copyCanonicalSdkSources(destination) {
  const vendorRoot = join(destination, "vendor/teamarness");
  for (const { from, to } of Object.values(canonicalSources)) {
    const target = join(vendorRoot, to);
    await rm(target, { recursive: true, force: true });
    await cp(join(repositoryRoot, from), target, {
      recursive: true,
      filter: (source) => !isTestFile(source),
    });
  }
  await cp(join(scriptDirectory, "vendor-digest.mjs"), join(destination, "scripts/vendor-digest.mjs"));

  const generatedGuidePath = join(vendorRoot, "package-ui-sdk/README.md");
  const canonicalGuidePath = join(repositoryRoot, "packages/teamarness-package-ui-sdk/README.md");
  const generatedNotice = await readFile(generatedGuidePath, "utf8");
  const canonicalGuide = await readFile(canonicalGuidePath, "utf8");
  const heading = canonicalGuide.slice(0, canonicalGuide.indexOf("\n"));
  const noticeBody = generatedNotice.slice(generatedNotice.indexOf("\n") + 1).trim();
  const canonicalBody = canonicalGuide.slice(canonicalGuide.indexOf("\n") + 1).trimStart();
  await writeFile(generatedGuidePath, `${heading}\n\n${noticeBody}\n\n${canonicalBody}`);
}

/**
 * Records which checkout the vendored trees came from, and what they contained.
 *
 * The revision names the source; the digests are what `npm run check:vendor` compares a project
 * against, so an edited vendored file is reported rather than silently carried.
 */
async function recordProvenance(destination) {
  const path = join(destination, "teamarness-authoring.json");
  const provenance = JSON.parse(await readFile(path, "utf8"));
  const { revision, dirty } = checkoutRevision();
  provenance.templateVersion = FOUNDATION_TEMPLATE_VERSION;
  provenance.sdkRevision = revision;
  provenance.sdkRevisionDirty = dirty;
  provenance.vendorDigests = await vendorDigests(destination);
  await writeFile(path, `${JSON.stringify(provenance, null, 2)}\n`);
}

async function vendorDigests(project) {
  const digests = {};
  for (const tree of VENDOR_TREES) {
    digests[tree] = await vendorTreeDigest(join(project, "vendor/teamarness", tree));
  }
  return digests;
}

function checkoutRevision() {
  const git = (...args) => execFileSync("git", ["-C", repositoryRoot, ...args], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  });
  try {
    const revision = git("rev-parse", "HEAD").trim();
    if (!/^[0-9a-f]{40}$/.test(revision)) return { revision: "unknown", dirty: true };
    return { revision, dirty: git("status", "--porcelain", "--", ...provenancePaths).trim().length > 0 };
  } catch {
    return { revision: "unknown", dirty: true };
  }
}

/**
 * Compares a project's vendored trees with what this checkout would generate today.
 *
 * It is read-only, on purpose: ADR 0010 leaves a generated project without an SDK updater, so this
 * reports a difference and leaves the decision to the author.
 */
export async function vendorReport(project) {
  const projectRoot = resolve(project);
  const provenance = JSON.parse(await readFile(join(projectRoot, "teamarness-authoring.json"), "utf8"));
  const manifest = JSON.parse(await readFile(join(projectRoot, "teamarness.package.json"), "utf8"));
  const parent = await mkdtemp(join(dirname(projectRoot), ".teamarness-vendor-report-"));
  try {
    const comparison = join(parent, basename(projectRoot));
    await createPackageProject({
      output: comparison,
      identity: manifest.identity,
      displayName: manifest.presentation.displayName,
    });
    const current = await vendorDigests(comparison);
    const trees = {};
    for (const tree of VENDOR_TREES) {
      const held = await vendorTreeDigest(join(projectRoot, "vendor/teamarness", tree));
      trees[tree] = held === current[tree] ? "same" : "differs";
    }
    const report = {
      sdkRevision: provenance.sdkRevision,
      sdkRevisionDirty: provenance.sdkRevisionDirty,
      trees,
    };
    console.log(`sdkRevision: ${report.sdkRevision}${report.sdkRevisionDirty ? " (dirty)" : ""}`);
    for (const tree of VENDOR_TREES) console.log(`${tree}: ${trees[tree]}`);
    return report;
  } finally {
    await rm(parent, { recursive: true, force: true });
  }
}

async function replaceTokens(path, tokens) {
  const info = await lstat(path);
  if (info.isDirectory()) {
    for (const entry of await readdir(path)) await replaceTokens(join(path, entry), tokens);
    return;
  }
  if (!info.isFile() || !textExtensions.has(extname(path))) return;
  const original = await readFile(path, "utf8");
  const tokenPattern = new RegExp(
    Object.keys(tokens).map((token) => token.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|"),
    "g",
  );
  const replaced = original.replace(tokenPattern, (token) => tokens[token]);
  if (replaced !== original) await writeFile(path, replaced);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    const args = parseArgs(process.argv.slice(2));
    if (args.vendorReport) await vendorReport(args.vendorReport);
    else await createPackageProject(args);
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  }
}
