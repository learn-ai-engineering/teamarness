import { REPOSITORY } from "./publish.mjs";

const API = `https://api.github.com/repos/${REPOSITORY}`;
const ASSET_HOSTS = new Set(["release-assets.githubusercontent.com", "objects.githubusercontent.com", "github-releases.githubusercontent.com"]);

export function githubAdapter({ token, fetcher = fetch }) {
  if (!token) throw new Error("CI GitHub token is required");
  async function request(url, { method = "GET", json, bytes, limit = 4 * 1024 * 1024, binary = false, missing = false } = {}) {
    const initial = new URL(url);
    if (!["api.github.com", "uploads.github.com"].includes(initial.hostname) || initial.protocol !== "https:") throw new Error("Invalid GitHub adapter destination");
    const signal = AbortSignal.timeout(15 * 60 * 1000);
    let current = url;
    for (let hop = 0; hop <= 5; hop++) {
      const parsed = new URL(current);
      if (parsed.protocol !== "https:" || parsed.username || parsed.password || parsed.port || parsed.hash
        || (hop > 0 && !ASSET_HOSTS.has(parsed.hostname))) throw new Error("Unsafe GitHub asset redirect");
      const headers = { Accept: binary ? "application/octet-stream" : "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28" };
      // Never forward the CI token to release-asset storage hosts.
      if (hop === 0) headers.Authorization = `Bearer ${token}`;
      if (json !== undefined) headers["Content-Type"] = "application/json";
      if (bytes !== undefined) headers["Content-Type"] = "application/octet-stream";
      const response = await fetcher(current, { method, headers, body: json === undefined ? bytes : JSON.stringify(json), redirect: "manual", signal });
      if ([301, 302, 303, 307, 308].includes(response.status)) {
        await response.body?.cancel();
        if (!binary || method !== "GET" || hop === 5) throw new Error("Unexpected GitHub redirect");
        current = new URL(response.headers.get("location"), current).href; continue;
      }
      if (response.status === 404 && missing) { await response.body?.cancel(); return undefined; }
      if (!response.ok) { await response.body?.cancel(); throw new Error(`GitHub ${method} failed (${response.status}); no response body logged`); }
      const chunks = []; let total = 0;
      for await (const chunk of response.body ?? []) {
        total += chunk.length;
        if (total > limit) throw new Error("GitHub response exceeds bound");
        chunks.push(chunk);
      }
      const result = Buffer.concat(chunks);
      return binary ? result : result.length ? JSON.parse(result.toString("utf8")) : undefined;
    }
    throw new Error("GitHub redirect limit exceeded");
  }
  const get = (path) => request(`${API}${path}`);
  const id = (value) => { if (!Number.isSafeInteger(value) || value < 1) throw new Error("Invalid GitHub object ID"); return value; };
  return {
    repository: () => get(""), main: () => get("/branches/main"),
    ancestor: async (sha) => (await get(`/compare/${encodeURIComponent(sha)}...main`)).merge_base_commit?.sha === sha,
    tagSha: async (tag) => {
      let object = (await get(`/git/ref/tags/${encodeURIComponent(tag)}`)).object;
      for (let depth = 0; object.type === "tag" && depth < 5; depth++) object = (await get(`/git/tags/${encodeURIComponent(object.sha)}`)).object;
      if (object.type !== "commit") throw new Error("Tag does not resolve to commit");
      return object.sha;
    },
    releases: (page) => get(`/releases?per_page=100&page=${id(page)}`),
    release: (releaseId) => get(`/releases/${id(releaseId)}`),
    assets: async (releaseId) => {
      const all = [];
      for (let page = 1; ; page++) {
        const batch = await get(`/releases/${id(releaseId)}/assets?per_page=100&page=${page}`);
        all.push(...batch); if (batch.length < 100) return all;
      }
    },
    createDraft: (body) => request(`${API}/releases`, { method: "POST", json: body }),
    deleteAsset: (assetId) => request(`${API}/releases/assets/${id(assetId)}`, { method: "DELETE" }),
    upload: (releaseId, name, bytes) => request(`https://uploads.github.com/repos/${REPOSITORY}/releases/${id(releaseId)}/assets?name=${encodeURIComponent(name)}`, { method: "POST", bytes }),
    download: (asset, limit) => request(`${API}/releases/assets/${id(asset.id)}`, { binary: true, limit }),
    publish: (releaseId, body) => request(`${API}/releases/${id(releaseId)}`, { method: "PATCH", json: body }),
  };
}

// Deliberately read-only. CLI fixtures can qualify policy and actual local bytes,
// but cannot be selected by a mutating publication command.
export function fixtureGithubAdapter(fixture) {
  const deny = async () => { throw new Error("Fixture GitHub adapter is read-only"); };
  return {
    repository: async () => structuredClone(fixture.repository), main: async () => structuredClone(fixture.main),
    tagSha: async () => fixture.tagSha, ancestor: async () => fixture.ancestor === true,
    releases: async (page) => structuredClone(fixture.releases.slice((page - 1) * 100, page * 100)),
    release: async (id) => structuredClone(fixture.releases.find((r) => r.id === id)),
    assets: async (id) => structuredClone(fixture.releases.find((r) => r.id === id).assets),
    download: async (asset) => Buffer.from(asset.base64, "base64"),
    createDraft: deny, deleteAsset: deny, upload: deny, publish: deny,
  };
}
