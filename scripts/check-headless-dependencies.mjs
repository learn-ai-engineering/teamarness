import { execFile } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const scriptPath = fileURLToPath(import.meta.url);
const repositoryRoot = resolve(dirname(scriptPath), "..");
const execFileAsync = promisify(execFile);

export function forbiddenHeadlessDependencies(tree) {
  const names = tree.split("\n").map((line) => line.trim().split(/\s+/)[0]);
  return [...new Set(names.filter((name) => /^(?:tauri(?:-|$)|wry$|webkit|javascriptcore|gtk|gdk|appkit|objc2-app-kit$|cocoa$)/i.test(name)))];
}

export async function checkHeadlessDependencies(root = repositoryRoot) {
  const { stdout } = await execFileAsync("cargo", ["tree", "-p", "teamarness-headless", "--edges", "normal,build", "--locked", "--prefix", "none"], { cwd: root, maxBuffer: 8 * 1024 * 1024 });
  return forbiddenHeadlessDependencies(stdout);
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  checkHeadlessDependencies().then((forbidden) => {
    if (forbidden.length) {
      console.error(`Headless production dependencies include GUI frameworks: ${forbidden.join(", ")}`);
      process.exitCode = 1;
    } else {
      console.log("Headless normal/build dependency closure contains no Tauri, WebKit, AppKit, or GTK.");
    }
  }).catch((error) => { console.error(error.message); process.exitCode = 1; });
}
