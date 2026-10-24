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
    for (const check of [
      "wasm-instantiates",
      "eval-blocked",
      "function-constructor-blocked",
      "ipc-fetch-blocked",
      "parent-document-blocked",
      "top-location-blocked",
      "top-navigation-blocked",
    ]) {
      assert.match(entrypoint, new RegExp(`runCheck\\("${check}"`), `${check} probe check`);
    }

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
