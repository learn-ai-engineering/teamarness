import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import {
  EXHIBITS_FIXTURE_CONSUMER_DISPLAY_NAME,
  EXHIBITS_FIXTURE_CONSUMER_IDENTITY,
  prepareExhibitsFixtureConsumer,
} from "./prepare.mjs";

const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");

async function withDataDirectory(body) {
  const directory = await mkdtemp(join(tmpdir(), "teamarness-exhibits-consumer-"));
  try {
    const backend = join(directory, "fake-backend");
    await writeFile(backend, "#!/bin/sh\nexit 0\n");
    return await body(join(directory, "data"), backend);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

test("prepares a local consumer Package whose manifest sizes and digests match its files", async () => {
  await withDataDirectory(async (dataDirectory, backend) => {
    await mkdir(dataDirectory, { mode: 0o700 });
    const prepared = await prepareExhibitsFixtureConsumer(dataDirectory, { backend });
    assert.equal(prepared.identity, EXHIBITS_FIXTURE_CONSUMER_IDENTITY);
    assert.equal(prepared.displayName, EXHIBITS_FIXTURE_CONSUMER_DISPLAY_NAME);
    const manifest = JSON.parse(await readFile(join(prepared.packageDirectory, "teamarness.package.json"), "utf8"));
    assert.equal(manifest.identity, EXHIBITS_FIXTURE_CONSUMER_IDENTITY);
    assert.ok(manifest.capabilities.some(({ identity }) => identity === "teamarness.content-views"));
    for (const declared of [manifest.backends[0], manifest.ui.entrypoint]) {
      const bytes = await readFile(join(prepared.packageDirectory, declared.path));
      assert.equal(declared.size, bytes.length, `${declared.path} size`);
      assert.equal(declared.sha256, digest(bytes), `${declared.path} digest`);
    }
    const contract = await readFile(join(prepared.packageDirectory, manifest.ui.contract.path));
    assert.equal(manifest.ui.contract.sha256, digest(contract));
    const fixtures = JSON.parse(contract.toString("utf8")).$defs.fixture.enum;
    assert.deepEqual(fixtures, ["revision-1", "revision-2", "media", "isolation-probe"]);
    const ui = await readFile(join(prepared.packageDirectory, "ui/index.js"), "utf8");
    for (const label of ["Publish revision 1", "Publish revision 2", "Publish media", "Publish second Exhibit",
      "Publish isolation probe", "Open unsandboxed frame probe"]) {
      assert.ok(ui.includes(`"${label}"`), label);
    }
    assert.equal((await stat(join(prepared.packageDirectory, manifest.backends[0].path))).mode & 0o777, 0o700);
  });
});

test("registers the consumer as trusted native code and keeps other local registrations", async () => {
  await withDataDirectory(async (dataDirectory, backend) => {
    await mkdir(join(dataDirectory, "packages"), { recursive: true, mode: 0o700 });
    const path = join(dataDirectory, "packages/registrations.v1.json");
    const other = { directory: "/elsewhere", expectedIdentity: "dev.example.other", trust: { kind: "trusted_native_code_v1", acceptedRisks: [] }, overrideApplication: false };
    await writeFile(path, JSON.stringify({ schemaVersion: 1, registrations: [other] }));
    await prepareExhibitsFixtureConsumer(dataDirectory, { backend });
    await prepareExhibitsFixtureConsumer(dataDirectory, { backend });
    const { schemaVersion, registrations } = JSON.parse(await readFile(path, "utf8"));
    assert.equal(schemaVersion, 1);
    assert.deepEqual(registrations.map(({ expectedIdentity }) => expectedIdentity),
      ["dev.example.other", EXHIBITS_FIXTURE_CONSUMER_IDENTITY]);
    assert.deepEqual(registrations[1].trust, {
      kind: "trusted_native_code_v1",
      acceptedRisks: ["filesystem", "network", "process"],
    });
    assert.equal(registrations[1].overrideApplication, false);
    assert.equal((await stat(path)).mode & 0o777, 0o600);
  });
});
