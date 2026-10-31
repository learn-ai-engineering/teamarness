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

// Verifies: TEAM.DEL-R05, TEAM.SYS.SHELL-R07.
for (const codePoint of [0x0080, 0x0085, 0x009f]) {
  const label = `U+${codePoint.toString(16).toUpperCase().padStart(4, "0")}`;
  const notes = `Release${String.fromCodePoint(codePoint)}notes`;
  test(`createManifest rejects ${label} in notes`, () => {
    assert.throws(() => createManifest({ ...input, notes }), /Notes must be bounded plain text/);
  });
  test(`validateManifest rejects ${label} in notes`, () => {
    const manifest = createManifest(input);
    manifest.notes = notes;
    assert.throws(() => validateManifest(manifest, input), /Notes must be bounded plain text/);
  });
}

// Verifies: TEAM.DEL-R05, TEAM.SYS.SHELL-R07.
test("createManifest and validateManifest preserve whitespace, Unicode and the UTF-8 byte boundary", () => {
  for (const notes of ["", "Release\tindent\r\nNext line", "Café — 更新 🙂 ~\u00a0", "a".repeat(4096), "é".repeat(2048), "🙂".repeat(1024)]) {
    const manifest = createManifest({ ...input, notes });
    assert.equal(manifest.notes, notes);
    assert.equal(validateManifest(manifest, input).notes, notes);
  }
});

// Verifies: TEAM.DEL-R05, TEAM.SYS.SHELL-R07.
test("createManifest and validateManifest reject C0, DEL, angles and excess UTF-8 bytes", () => {
  for (const notes of ["<", ">", "\u0000", "\u0008", "\u000b", "\u000c", "\u000e", "\u001f", "\u007f", "a".repeat(4097), "é".repeat(2048) + "a", "🙂".repeat(1024) + "a", 4]) {
    assert.throws(() => createManifest({ ...input, notes }), /Notes must be bounded plain text/);
    const manifest = createManifest(input);
    manifest.notes = notes;
    assert.throws(() => validateManifest(manifest, input), /Notes must be bounded plain text/);
  }
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
