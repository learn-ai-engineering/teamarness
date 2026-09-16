import { cp, lstat, mkdtemp, readdir, readFile, rename, rm, stat, writeFile } from "node:fs/promises";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const FOUNDATION_TEMPLATE_VERSION = 1;
export const PACKAGE_IDENTITY_PATTERN = /^[a-z](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\.[a-z](?:[a-z0-9-]{0,61}[a-z0-9])?)+$/;

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
export const repositoryRoot = dirname(scriptDirectory);
export const templateDirectory = join(repositoryRoot, "templates/package-foundation/v1");
const canonicalSources = {
  protocol: join(repositoryRoot, "crates/teamarness-package-protocol/src"),
  backendSdk: join(repositoryRoot, "crates/teamarness-package-sdk/src"),
  uiSdk: join(repositoryRoot, "packages/teamarness-package-ui-sdk/src"),
};
const textExtensions = new Set([".json", ".js", ".mjs", ".ts", ".tsx", ".rs", ".toml", ".md", ".html", ".css", ""]);

export function parseArgs(argv) {
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
  for (const [name, source] of Object.entries(canonicalSources)) {
    const targetName = name === "protocol" ? "package-protocol" : name === "backendSdk" ? "package-sdk" : "package-ui-sdk";
    const target = join(vendorRoot, targetName, "src");
    await rm(target, { recursive: true, force: true });
    await cp(source, target, { recursive: true });
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
    await createPackageProject(parseArgs(process.argv.slice(2)));
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  }
}
