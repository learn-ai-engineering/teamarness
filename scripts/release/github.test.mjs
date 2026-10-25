import assert from "node:assert/strict";
import { test } from "node:test";
import { githubAdapter, fixtureGithubAdapter } from "./github.mjs";
import { assertEnvironment, releaseEvent, runReleaseCli } from "./cli.mjs";

// Verifies: TEAM.DEL-R04, TEAM.DEL-R05
 test("real GitHub adapter resolves annotated tags, paginates assets and scopes write paths", async () => {
  const requests = [];
  const api = githubAdapter({ token: "fixture-token", fetcher: async (url, options) => {
    requests.push([url, options]);
    if (url.includes("/git/ref/")) return Response.json({ object: { type: "tag", sha: "b".repeat(40) } });
    if (url.includes("/git/tags/")) return Response.json({ object: { type: "commit", sha: "a".repeat(40) } });
    if (url.includes("assets?")) return Response.json(url.endsWith("page=1") ? Array.from({ length: 100 }, (_, id) => ({ id })) : [{ id: 101 }]);
    if (options.method === "DELETE") return new Response(null, { status: 204 });
    return Response.json({});
  } });
  assert.equal(await api.tagSha("v1.0.0"), "a".repeat(40)); assert.equal((await api.assets(1)).length, 101);
  await api.upload(1, "latest.json", Buffer.from("{}")); await api.publish(1, { draft: false, prerelease: false, make_latest: "true" }); await api.deleteAsset(2);
  assert.ok(requests.some(([url, options]) => url.startsWith("https://uploads.github.com/repos/learn-ai-engineering/teamarness/") && options.method === "POST"));
  assert.ok(requests.every(([, options]) => options.redirect === "manual" && options.headers.Authorization === "Bearer fixture-token"));
  assert.throws(() => api.deleteAsset("../1"), /Invalid GitHub object ID/);
});
// Verifies: TEAM.DEL-R05
 test("asset readback redirects never forward authentication and enforce actual byte caps", async () => {
  const calls = [];
  const api = githubAdapter({ token: "fixture-token", fetcher: async (url, options) => {
    calls.push([url, options]);
    return calls.length === 1 ? new Response(null, { status: 302, headers: { location: "https://release-assets.githubusercontent.com/asset" } }) : new Response("abc");
  } });
  assert.deepEqual(await api.download({ id: 1 }, 3), Buffer.from("abc")); assert.equal(calls[1][1].headers.Authorization, undefined);
  const bounded = githubAdapter({ token: "fixture-token", fetcher: async () => new Response("abcd") });
  await assert.rejects(bounded.download({ id: 1 }, 3), /bound/);
});
// Verifies: TEAM.DEL-R05
 test("unsafe redirects, loops, errors and malformed responses are fatal and redacted", async () => {
  for (const location of ["http://release-assets.githubusercontent.com/a", "https://evil.test/a", "https://user:secret@release-assets.githubusercontent.com/a", "https://release-assets.githubusercontent.com:444/a", "https://release-assets.githubusercontent.com/a#fragment"]) {
    const api = githubAdapter({ token: "fixture-token", fetcher: async () => new Response(null, { status: 302, headers: { location } }) });
    await assert.rejects(api.download({ id: 1 }, 100));
  }
  let calls = 0;
  const loop = githubAdapter({ token: "fixture-token", fetcher: async () => { calls++; return new Response(null, { status: 302, headers: { location: "https://release-assets.githubusercontent.com/a" } }); } });
  await assert.rejects(loop.download({ id: 1 }, 100)); assert.equal(calls, 6);
  const failed = githubAdapter({ token: "fixture-token", fetcher: async () => new Response("private response", { status: 500 }) });
  await assert.rejects(failed.repository(), (error) => !error.message.includes("private response") && !error.message.includes("fixture-token"));
});
// Verifies: TEAM.DEL-R04
 test("environment requires exact tools and native host; malformed release events fail closed", () => {
  const env = { node: "24.20.0", platform: "darwin", arch: "arm64", rust: "release: 1.99.1\nhost: aarch64-apple-darwin\n" };
  assert.doesNotThrow(() => assertEnvironment(env));
  for (const change of [{ node: "24.19.0" }, { arch: "x64" }, { platform: "linux" }, { rust: "release: 1.96.0\nhost: aarch64-apple-darwin\n" }]) assert.throws(() => assertEnvironment({ ...env, ...change }));
  assert.equal(releaseEvent({}, {}).deleted, true);
});
// Verifies: TEAM.DEL-R05
 test("fixture adapter is structurally read-only and cannot be selected for publication", async () => {
  const api = fixtureGithubAdapter({});
  for (const method of ["createDraft", "deleteAsset", "upload", "publish"]) await assert.rejects(api[method](), /read-only/);
  await assert.rejects(runReleaseCli(["publish", "--tag", "v0.1.0", "--fixture", "no-file"]), /Invalid release arguments/);
  await assert.rejects(runReleaseCli(["preflight", "--tag", "v0.1.0", "--dry-run"]), /Invalid release arguments/);
});
