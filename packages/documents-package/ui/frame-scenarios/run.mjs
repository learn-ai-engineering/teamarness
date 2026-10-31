// `npm run verify:documents-editor`: the rendered-editor frame scenarios.
//
// Builds the Documents UI and the Rust Markdown authority, serves the UI in sandboxed
// `allow-scripts` frames under the Shell's frame policy, and runs every scenario in WebKit (the
// shipped engine) and Chromium. `node run.mjs webkit` runs one browser; `--only <name>` one
// scenario. Not part of `npm test`: it needs the Playwright browsers, which it installs.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { chromium, webkit } from "playwright";
import { MarkdownAuthority } from "./backend.mjs";
import { Harness } from "./harness.mjs";
import { SCENARIOS } from "./scenarios.mjs";
import { startServer } from "./server.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const repository = path.resolve(here, "../../../..");
const args = process.argv.slice(2);
const only = args.includes("--only") ? args[args.indexOf("--only") + 1] : null;
const engines = [["webkit", webkit], ["chromium", chromium]].filter(([name]) => {
  const named = args.filter((arg) => arg === "webkit" || arg === "chromium");
  return named.length === 0 || named.includes(name);
});

function step(label, command, commandArgs) {
  console.log(`> ${label}`);
  execFileSync(command, commandArgs, { cwd: repository, stdio: ["ignore", "inherit", "inherit"] });
}

if (process.env.FRAME_SCENARIOS_SKIP_BUILD !== "1") {
  step("build the Documents UI", "npm", ["--workspace", "@teamarness/documents-package-ui", "run", "build"]);
  step("build the Rust Markdown authority", "cargo", ["build", "--quiet", "--locked", "-p", "teamarness-documents-package", "--example", "frame_scenarios"]);
  step("install the Playwright browsers", "npx", ["playwright", "install", ...engines.map(([name]) => name)]);
}
const binary = path.join(repository, "target/debug/examples/frame_scenarios");
if (!fs.existsSync(binary)) throw new Error(`missing ${binary}`);
const entrypoint = fs.statSync(path.join(here, "../dist/index.js")).size;

const authority = new MarkdownAuthority(binary);
const server = await startServer();
const origin = `http://127.0.0.1:${server.address().port}`;
const results = [];

for (const [engine, type] of engines) {
  const browser = await type.launch();
  for (const scenario of SCENARIOS) {
    if (only !== null && scenario.name !== only) continue;
    const started = Date.now();
    const harness = await Harness.open(browser, origin, authority);
    let failure = null;
    let notes = "";
    try {
      notes = (await scenario.run(harness, { engine })) ?? "";
      const problems = residue(harness, engine, scenario);
      if (problems.length > 0) throw new Error(`console or policy errors: ${JSON.stringify(problems).slice(0, 1200)}`);
    } catch (error) {
      failure = error;
      if (process.env.FRAME_SCENARIOS_DEBUG) {
        for (const view of harness.views.keys()) {
          const state = await harness.rawFrame(view)?.evaluate(() => {
            const root = document.getElementById("teamarness-package-host").shadowRoot;
            return { notice: root.querySelector(".editor-notice")?.textContent, body: root.querySelector(".ProseMirror")?.innerHTML.slice(0, 600), active: root.activeElement?.className, composer: root.querySelector(".comment-input")?.value, error: root.querySelector(".documents-page [role=alert]")?.textContent };
          }).catch((reason) => String(reason));
          console.log(`  ${view}: ${JSON.stringify(state)}`);
        }
        console.log(`  comments: ${JSON.stringify(harness.workspace.comments).slice(0, 400)}`);
      }
    } finally {
      await harness.close();
    }
    const result = { engine, name: scenario.name, ok: failure === null, ms: Date.now() - started, notes: failure ? String(failure.stack ?? failure) : notes };
    results.push(result);
    console.log(`[${engine}] ${result.ok ? "PASS" : "FAIL"} ${scenario.name} (${result.ms} ms)${result.notes ? ` — ${result.notes.slice(0, 2000)}` : ""}`);
  }
  await browser.close();
}
server.close();
authority.dispose();

/**
 * Everything a scenario left in the consoles, the policy reports and the frames' error handlers.
 * The one accepted residue is Chromium's `style-src-attr` report when pasted HTML carries `style=`
 * (the paste still works; the attribute is dropped).
 */
function residue(harness, engine, scenario) {
  const accepted = (entry) => engine === "chromium" && scenario.pastesStyledHtml === true
    && /style-src-attr|inline style|'style-src'/.test(`${entry.directive ?? ""} ${entry.text ?? ""}`);
  return [
    ...harness.errors,
    ...harness.csp.filter((entry) => !accepted(entry)),
    ...harness.console.filter((entry) => !accepted(entry)),
  ];
}

console.log(`\nDocuments UI entrypoint: ${entrypoint} bytes`);
const width = Math.max(...results.map((result) => result.name.length)) + 2;
console.log(`${"scenario".padEnd(width)}${engines.map(([name]) => name.padEnd(10)).join("")}`);
for (const name of [...new Set(results.map((result) => result.name))]) {
  console.log(`${name.padEnd(width)}${engines.map(([engine]) => {
    const result = results.find((entry) => entry.engine === engine && entry.name === name);
    return (result ? (result.ok ? "PASS" : "FAIL") : "-").padEnd(10);
  }).join("")}`);
}
const failed = results.filter((result) => !result.ok);
if (failed.length > 0 || results.length === 0) {
  console.error(`\n${failed.length} of ${results.length} scenario runs failed.`);
  process.exit(1);
}
console.log(`\nAll ${results.length} scenario runs passed.`);
