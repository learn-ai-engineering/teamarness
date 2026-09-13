import { chmod, mkdir, stat } from "node:fs/promises";
import { spawn } from "node:child_process";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_PATH = fileURLToPath(import.meta.url);
export const REPOSITORY_ROOT = resolve(dirname(SCRIPT_PATH), "..");
const PROFILE_PATTERN = /^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/;

export function resolveDevelopmentDataDir(value, repositoryRoot = REPOSITORY_ROOT) {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error("Provide one profile name, for example: npm run dev -- alice");
  }

  if (isAbsolute(value)) {
    return resolve(value);
  }

  if (!PROFILE_PATTERN.test(value)) {
    throw new Error(
      "Profile names must be 1-64 letters, numbers, underscores, or hyphens, starting with a letter or number. Use an absolute path for an expert override.",
    );
  }

  return join(repositoryRoot, ".teamarness-dev", value);
}

export function buildDevelopmentCommand(dataDir, platform = process.platform) {
  return {
    command: platform === "win32" ? "npm.cmd" : "npm",
    args: [
      "--workspace",
      "teamarness-desktop",
      "run",
      "tauri",
      "--",
      "dev",
      "--",
      "--",
      "--data-dir",
      dataDir,
    ],
  };
}

export function buildInstanceCommand(
  dataDir,
  repositoryRoot = REPOSITORY_ROOT,
  platform = process.platform,
) {
  const executable = join(
    repositoryRoot,
    "target",
    "debug",
    platform === "win32" ? "teamarness-desktop.exe" : "teamarness-desktop",
  );
  return {
    command: executable,
    args: ["--data-dir", dataDir],
  };
}

export async function ensurePrivateDirectory(dataDir, platform = process.platform) {
  await mkdir(dataDir, { recursive: true, mode: 0o700 });
  if (platform !== "win32") {
    await chmod(dataDir, 0o700);
  }
}

export async function requireDebugExecutable(command) {
  try {
    const details = await stat(command);
    if (details.isFile()) {
      return;
    }
  } catch (error) {
    if (error?.code !== "ENOENT") {
      throw error;
    }
  }

  throw new Error(
    `Debug executable not found at ${command}. Run "npm run dev -- <profile>" once to build it.`,
  );
}

function runChild({ command, args }) {
  const child = spawn(command, args, {
    cwd: REPOSITORY_ROOT,
    stdio: "inherit",
    shell: false,
  });
  const signals = ["SIGINT", "SIGTERM", "SIGHUP"];
  const handlers = new Map();

  const removeHandlers = () => {
    for (const [signal, handler] of handlers) {
      process.off(signal, handler);
    }
  };

  for (const signal of signals) {
    const handler = () => {
      if (!child.killed) {
        child.kill(signal);
      }
    };
    handlers.set(signal, handler);
    process.on(signal, handler);
  }

  child.once("error", (error) => {
    removeHandlers();
    console.error(`Could not start development process: ${error.message}`);
    process.exitCode = 1;
  });

  child.once("exit", (code, signal) => {
    removeHandlers();
    if (code !== null) {
      process.exitCode = code;
      return;
    }
    if (signal) {
      process.kill(process.pid, signal);
    }
  });
}

export function parseLauncherArguments(argv) {
  const [mode, value, ...extra] = argv;
  if ((mode !== "dev" && mode !== "instance") || extra.length > 0) {
    throw new Error("Usage: npm run dev -- <profile> | npm run dev:instance -- <profile>");
  }
  return { mode, value };
}

export async function main(argv = process.argv.slice(2)) {
  const { mode, value } = parseLauncherArguments(argv);
  const dataDir = resolveDevelopmentDataDir(value);
  await ensurePrivateDirectory(dataDir);

  const childCommand =
    mode === "dev"
      ? buildDevelopmentCommand(dataDir)
      : buildInstanceCommand(dataDir);
  if (mode === "instance") {
    await requireDebugExecutable(childCommand.command);
  }
  runChild(childCommand);
}

if (process.argv[1] && resolve(process.argv[1]) === SCRIPT_PATH) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
