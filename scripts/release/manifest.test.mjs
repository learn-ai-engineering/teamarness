import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { assetNames, createManifest, validateManifest, validateSignature, ARCHIVE_LIMIT } from "./manifest.mjs";

const signature = await readFile(new URL("../../apps/teamarness-desktop/src-tauri/tests/fixtures/updater/versioned.sig", import.meta.url), "utf8");
const input = { tag: "v1.2.3", signature, notes: "Release notes\nPlain text." };

test("both target keys carry the complete signature and exact repository/tag URL", () => {
  const manifest = createManifest(input);
  assert.equal(manifest.version, "1.2.3");
  assert.deepEqual(manifest.platforms["darwin-aarch64"], manifest.platforms["darwin-aarch64-app"]);
  assert.equal(manifest.platforms["darwin-aarch64"].signature, signature);
  assert.equal(manifest.platforms["darwin-aarch64"].url, "https://github.com/learn-ai-engineering/teamarness/releases/download/v1.2.3/Teamarness.app.tar.gz");
  validateManifest(manifest, input);
  assert.deepEqual(assetNames("v1.2.3"), ["Teamarness_1.2.3_aarch64.dmg", "Teamarness.app.tar.gz", "Teamarness.app.tar.gz.sig", "latest.json"]);
  assert.equal(ARCHIVE_LIMIT, 512 * 1024 * 1024);
});

test("malformed, truncated, overlong and unversioned signatures are rejected", () => {
  validateSignature(signature, "1.2.3");
  for (const bad of ["", "not base64", signature.slice(0, -20), "x".repeat(4097), Buffer.from("untrusted comment: signature\nabc\ntrusted comment: timestamp:1\nabc\n").toString("base64")]) assert.throws(() => createManifest({ ...input, signature: bad }));
  assert.throws(() => createManifest({ ...input, tag: "v9.9.9" }));
});

test("notes are bounded plain text and feed targets/URLs are closed", () => {
  for (const notes of ["<script>x</script>", "hello\u0000", "a".repeat(4097), 4]) assert.throws(() => createManifest({ ...input, notes }));
  for (const url of ["http://github.com/learn-ai-engineering/teamarness/releases/download/v1.2.3/Teamarness.app.tar.gz", "https://github.com.evil/x", "https://user@github.com/learn-ai-engineering/teamarness/releases/download/v1.2.3/Teamarness.app.tar.gz", "https://github.com/learn-ai-engineering/teamarness/releases/latest/download/Teamarness.app.tar.gz", "https://github.com/learn-ai-engineering/teamarness/releases/download/v1.2.3/Teamarness.app.tar.gz#fragment"]) {
    const manifest = createManifest(input);
    manifest.platforms["darwin-aarch64"].url = url;
    assert.throws(() => validateManifest(manifest, input));
  }
  for (const mutate of [m => delete m.platforms["darwin-aarch64-app"], m => m.platforms.extra = {}, m => m.version = "9.9.9", m => m.extra = "unexpected"]) {
    const manifest = createManifest(input); mutate(manifest);
    assert.throws(() => validateManifest(manifest, input));
  }
});
