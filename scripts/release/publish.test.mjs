import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { test } from "node:test";
import { publishRelease, preflight, provenance } from "./publish.mjs";
import { assetNames } from "./manifest.mjs";

const sha = "a".repeat(40), tag = "v0.1.0";
const event = { name: "push", repository: "learn-ai-engineering/teamarness", ref: `refs/tags/${tag}`, sha, deleted: false };
function fixture() {
  const files = Object.fromEntries(assetNames(tag).map((name) => [name, Buffer.from(name)]));
  const record = { schemaVersion: 1, sourceSha: sha, workingTreeDirty: false, tag, version: "0.1.0", identifier: "dev.teamarness.desktop", architecture: "arm64", appDigest: "b".repeat(64), digests: Object.fromEntries(Object.entries(files).map(([name, bytes]) => [name, createHash("sha256").update(bytes).digest("hex")])) };
  const calls = [], releases = []; let next = 1;
  const api = {
    repository: async () => ({ full_name: event.repository, private: false, visibility: "public", fork: false }),
    main: async () => ({ protected: true }), tagSha: async () => sha, ancestor: async () => true,
    releases: async (page) => releases.slice((page - 1) * 100, page * 100),
    release: async (id) => structuredClone(releases.find((r) => r.id === id)),
    assets: async (id) => structuredClone(releases.find((r) => r.id === id).assets),
    createDraft: async (body) => { calls.push("create"); const r = { ...body, id: next++, assets: [] }; releases.push(r); return structuredClone(r); },
    deleteAsset: async (id) => { calls.push("delete"); for (const r of releases) r.assets = r.assets.filter((a) => a.id !== id); },
    upload: async (id, name, bytes) => { calls.push(`upload:${name}`); releases.find((r) => r.id === id).assets.push({ id: next++, name, bytes, size: bytes.length }); },
    download: async (asset) => { calls.push(`read:${asset.name}`); return Buffer.from(asset.bytes); },
    publish: async (id, body) => { calls.push("publish"); Object.assign(releases.find((r) => r.id === id), body); },
  };
  const options = { api, event, tag, sourceSha: sha, files, record, qualify: async () => { calls.push("qualify"); } };
  return { ...options, options, calls, releases };
}
// Verifies: TEAM.DEL-R04, TEAM.DEL-R05
 test("empty first history publishes complete readback, stable/latest is final mutation", async () => {
  const f = fixture(); assert.equal((await publishRelease(f.options)).status, "published");
  assert.equal(f.calls.at(-1), "publish"); assert.equal(f.releases[0].make_latest, "true");
  assert.equal(f.releases[0].prerelease, false); assert.equal(f.releases[0].draft, false);
  assert.deepEqual(f.releases[0].assets.map((a) => a.name), assetNames(tag));
});
// Verifies: TEAM.DEL-R04
 test("preflight rejects events, forks, nonpublic destinations, wrong SHA and unprotected/unrelated main", async () => {
  for (const alter of [
    (f) => f.options.event = { ...event, name: "pull_request" },
    (f) => f.options.event = { ...event, deleted: true },
    (f) => f.options.event = { ...event, ref: "refs/tags/v01.1.0" },
    (f) => f.options.event = { ...event, repository: "fork/teamarness" },
    (f) => f.api.repository = async () => ({ full_name: event.repository, private: true }),
    (f) => f.api.repository = async () => ({ full_name: event.repository, private: false, visibility: "public", fork: true }),
    (f) => f.api.tagSha = async () => "c".repeat(40),
    (f) => f.api.main = async () => ({ protected: false }),
    (f) => f.api.ancestor = async () => false,
  ]) { const f = fixture(); alter(f); await assert.rejects(preflight(f.options)); assert.deepEqual(f.calls, []); }
});
// Verifies: TEAM.DEL-R05
 test("partial upload leaves a draft and same-SHA rerun replaces draft assets", async () => {
  const f = fixture(), upload = f.api.upload; let n = 0;
  f.api.upload = async (...args) => { if (++n === 2) throw Error("upload failed"); return upload(...args); };
  await assert.rejects(publishRelease(f.options), /upload failed/); assert.equal(f.releases[0].draft, true);
  f.api.upload = upload; await publishRelease(f.options); assert.equal(f.calls.at(-1), "publish");
});
// Verifies: TEAM.DEL-R05
 test("missing/extra local assets, dirty or wrong-source records fail before mutation", async () => {
  for (const alter of [(f) => delete f.files["latest.json"], (f) => f.files.key = Buffer.from("bad"), (f) => f.record.workingTreeDirty = true, (f) => f.record.sourceSha = "c".repeat(40), (f) => f.record.digests["latest.json"] = "0".repeat(64)]) {
    const f = fixture(); alter(f); await assert.rejects(publishRelease(f.options)); assert.deepEqual(f.calls, []);
  }
});
// Verifies: TEAM.DEL-R05
 test("missing/extra readback assets, digest failure and native/signature failure cannot publish", async () => {
  for (const mode of ["missing", "extra", "digest", "signature"]) {
    const f = fixture(), assets = f.api.assets;
    if (mode === "missing") f.api.assets = async (id) => (await assets(id)).slice(1);
    if (mode === "extra") f.api.assets = async (id) => [...await assets(id), { id: 999, name: "secret" }];
    if (mode === "digest") f.api.download = async () => Buffer.from("changed");
    if (mode === "signature") f.options.qualify = async () => { throw Error("official verifier rejected"); };
    await assert.rejects(publishRelease(f.options)); assert.ok(!f.calls.includes("publish")); assert.equal(f.releases[0].draft, true);
  }
});
// Verifies: TEAM.DEL-R05
 test("wrong-SHA drafts and published overwrites are immutable", async () => {
  for (const draft of [true, false]) {
    const f = fixture(); f.releases.push({ id: 1, tag_name: tag, target_commitish: "c".repeat(40), body: provenance("c".repeat(40)), draft, prerelease: false, assets: [] });
    await assert.rejects(publishRelease(f.options)); assert.deepEqual(f.calls, []);
  }
});
// Verifies: TEAM.DEL-R05
 test("paginated history rejects delayed/backfill/equal versions; ignores drafts/prereleases/noncanonical", async () => {
  const f = fixture(); for (let i = 0; i < 100; i++) f.releases.push({ id: 1000 + i, tag_name: "v9.0.0-rc", draft: false, prerelease: true });
  f.releases.push({ id: 2000, tag_name: "v0.2.0", draft: false, prerelease: false });
  await assert.rejects(publishRelease(f.options), /increase/); assert.deepEqual(f.calls, []);
  f.releases.pop(); await publishRelease(f.options);
});
// Verifies: TEAM.DEL-R05
 test("history and tag rechecked immediately before final publish", async () => {
  for (const mode of ["concurrent", "tag", "assets"]) {
    const f = fixture(); f.options.qualify = async () => {
      if (mode === "concurrent") f.releases.push({ id: 2000, tag_name: "v0.1.1", draft: false, prerelease: false });
      if (mode === "tag") f.api.tagSha = async () => "c".repeat(40);
      if (mode === "assets") f.releases[0].assets.pop();
    };
    await assert.rejects(publishRelease(f.options)); assert.ok(!f.calls.includes("publish"));
  }
});
// Verifies: TEAM.DEL-R05
 test("publication response loss rerun recognizes matching published bytes without mutation", async () => {
  const f = fixture(), publish = f.api.publish;
  f.api.publish = async (...args) => { await publish(...args); throw Error("response lost"); };
  await assert.rejects(publishRelease(f.options), /response lost/); f.calls.length = 0;
  assert.equal((await publishRelease(f.options)).status, "already-published");
  assert.ok(f.calls.every((c) => c.startsWith("read:") || c === "qualify"));
  f.files["latest.json"] = Buffer.from("new bytes"); await assert.rejects(publishRelease(f.options));
});
// Verifies: TEAM.DEL-R05
 test("dry-run qualifies local files and reads policy without any mutation", async () => {
  const f = fixture(); assert.equal((await publishRelease({ ...f.options, dryRun: true })).status, "dry-run"); assert.deepEqual(f.calls, ["qualify"]);
});
