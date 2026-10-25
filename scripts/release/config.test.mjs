import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { ENDPOINT } from "./manifest.mjs";

const configRoot = new URL("../../apps/teamarness-desktop/src-tauri/", import.meta.url);

test("release opt-in alone configures updater/ad-hoc signing; base identity/isolation/resources stay unchanged", async () => {
  const base = JSON.parse(await readFile(new URL("tauri.conf.json", configRoot), "utf8"));
  const release = JSON.parse(await readFile(new URL("tauri.release.conf.json", configRoot), "utf8"));
  assert.equal(base.identifier, "dev.teamarness.desktop");
  assert.equal(base.bundle.createUpdaterArtifacts, undefined);
  assert.equal(base.bundle.macOS?.signingIdentity, undefined);
  assert.equal(base.build.features, undefined);
  assert.deepEqual(release.build.features, ["production-release"]);
  assert.match(release.build.beforeBuildCommand, /build-application-packages.mjs --release/);
  assert.equal(release.bundle.createUpdaterArtifacts, true);
  assert.equal(release.bundle.macOS.signingIdentity, "-");
  // The runtime resolves Backends from Package Directories. Release staging signs those
  // copies before hashing; redundant externalBin copies must not be re-signed later.
  assert.deepEqual(release.bundle.externalBin, []);
  for (const key of ["identifier", "version", "app"]) assert.equal(release[key], undefined);
  assert.equal(release.bundle.resources, undefined);
  assert.deepEqual(release.plugins.updater, { pubkey: "", requireSignedVersion: true, endpoints: [ENDPOINT] });
  assert.deepEqual(JSON.parse(await readFile(new URL("capabilities/default.json", configRoot), "utf8")).webviews, ["main"]);
});
