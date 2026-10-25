import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import {
  prepareUiIsolationProbe,
  UI_ISOLATION_PROBE_DISPLAY_NAME,
  UI_ISOLATION_PROBE_IDENTITY,
} from "./prepare.mjs";

const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");

async function withDataDirectory(body) {
  const directory = await mkdtemp(join(tmpdir(), "teamarness-ui-isolation-probe-"));
  try {
    return await body(directory);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

test("prepares a local probe Package whose manifest sizes and digests match its files", async () => {
  await withDataDirectory(async (dataDirectory) => {
    const prepared = await prepareUiIsolationProbe(dataDirectory);
    assert.equal(prepared.identity, UI_ISOLATION_PROBE_IDENTITY);
    assert.equal(prepared.displayName, UI_ISOLATION_PROBE_DISPLAY_NAME);

    const packageDirectory = join(dataDirectory, "ui-isolation-probe-package");
    const manifest = JSON.parse(await readFile(join(packageDirectory, "teamarness.package.json"), "utf8"));
    assert.equal(manifest.identity, UI_ISOLATION_PROBE_IDENTITY);
    assert.equal(manifest.presentation.displayName, UI_ISOLATION_PROBE_DISPLAY_NAME);
    assert.deepEqual(manifest.capabilities, [
      { identity: "teamarness.workspace-context", major: 1, minMinor: 0, maxMinor: 0 },
    ]);

    for (const declared of [manifest.backends[0], manifest.ui.entrypoint]) {
      const bytes = await readFile(join(packageDirectory, declared.path));
      assert.equal(declared.size, bytes.length, `${declared.path} size`);
      assert.equal(declared.sha256, digest(bytes), `${declared.path} digest`);
    }
    const contract = await readFile(join(packageDirectory, manifest.ui.contract.path));
    assert.equal(manifest.ui.contract.sha256, digest(contract));
    assert.deepEqual(JSON.parse(contract.toString("utf8")).requests, {});
    assert.deepEqual(manifest.ui.assets, []);

    const entrypoint = await readFile(join(packageDirectory, "ui/index.js"), "utf8");
    assert.match(entrypoint, /export async function mount\(context\)/);
    assert.match(entrypoint, /ui-isolation-breach/);

    assert.equal((await stat(join(packageDirectory, "backend/package.mjs"))).mode & 0o777, 0o700);
    assert.equal((await stat(join(packageDirectory, "ui/index.js"))).mode & 0o777, 0o600);
  });
});

test("registers the probe as trusted native code without overriding an application Package", async () => {
  await withDataDirectory(async (dataDirectory) => {
    await prepareUiIsolationProbe(dataDirectory);
    const path = join(dataDirectory, "packages/registrations.v1.json");
    const registrations = JSON.parse(await readFile(path, "utf8"));
    assert.equal(registrations.schemaVersion, 1);
    assert.equal(registrations.registrations.length, 1);
    const [entry] = registrations.registrations;
    assert.equal(entry.expectedIdentity, UI_ISOLATION_PROBE_IDENTITY);
    assert.equal(entry.directory, join(dataDirectory, "ui-isolation-probe-package"));
    assert.deepEqual(entry.trust, {
      kind: "trusted_native_code_v1",
      acceptedRisks: ["filesystem", "network", "process"],
    });
    assert.equal(entry.overrideApplication, false);
    assert.equal((await stat(path)).mode & 0o777, 0o600);
    assert.equal((await stat(join(dataDirectory, "packages"))).mode & 0o777, 0o700);
  });
});

// Verifies: TEAM.SYS.SHELL-R05. This fixture qualifies the attempts, not WKWebView isolation.
test("probe attempts all four commands with missing/fake keys, generation 1 and forged Package bridge messages", async () => {
  const { runInNewContext } = await import("node:vm");
  const source = await readFile(new URL("./ui/index.js", import.meta.url), "utf8");
  const posts = []; const fetches = []; const forged = []; const bridge = []; const text = [];
  const element = () => ({ textContent: "", append(...items) { text.push(...items); }, remove() {} });
  const window = {
    webkit: { messageHandlers: { ipc: { postMessage: (value) => { posts.push(JSON.parse(value)); } } } },
    parent: { postMessage: (value) => { forged.push(value); } }, top: {},
  };
  for (const [target, field] of [[window.parent, "document"], [window.top, "location"], [window, "localStorage"]]) {
    Object.defineProperty(target, field, { get() { throw new Error("denied"); } });
  }
  const mount = runInNewContext(`${source.replace("export async function mount", "async function mount")}\nmount;`, {
    window, self: { origin: "null" }, document: { createElement: element, documentElement: {} },
    getComputedStyle: () => ({ getPropertyValue: () => "accent" }),
    fetch: async (url, options) => { fetches.push({ url, options }); throw new Error("denied"); },
    setTimeout: () => 1,
  });
  await mount({ root: element(), client: { request: async (operation) => { bridge.push(operation); throw { code: "ui_contract_violation" }; } } });
  for (const command of ["application_update_status", "application_update_check", "application_update_restart", "application_update_quit"]) {
    const attempts = posts.filter(({ cmd }) => cmd === command);
    assert.equal(attempts.length, 2, command);
    assert.deepEqual(attempts.map((entry) => entry.__TAURI_INVOKE_KEY__), [undefined, "probe-fake-key"]);
    assert.ok(attempts.every(({ payload }) => payload.generation === "1"));
    assert.equal(fetches.filter(({ url }) => url.endsWith(command)).length, 2);
    assert.ok(forged.some(({ operation }) => operation === command));
    assert.ok(bridge.includes(command));
  }
  assert.ok(text.some(({ textContent }) => textContent === "Isolation probe PASS"));
  const smoke = await readFile(new URL("../smoke-packaged-app.mjs", import.meta.url), "utf8");
  assert.match(smoke, /handlerEntriesBeforeFrame/);
  assert.match(smoke, /requires the disposable verified Ready probe build, not default Disabled/);
  assert.match(smoke, /main consent must reach the non-destructive adapter exactly once/);
});
