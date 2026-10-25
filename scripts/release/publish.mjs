import { createHash } from "node:crypto";
import { assetNames } from "./manifest.mjs";
import { stableVersion, tagVersion } from "./version.mjs";

export const REPOSITORY = "learn-ai-engineering/teamarness";
export const provenance = (sha) => `Teamarness stable release\n\nSource-SHA: ${sha}\n`;
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");
const sameNames = (actual, expected) => actual.length === expected.length && [...actual].sort().every((name, at) => name === [...expected].sort()[at]);

export async function preflight({ api, event, tag, sourceSha }) {
  tagVersion(tag);
  if (event.name !== "push" || event.deleted !== false || event.repository !== REPOSITORY
    || event.ref !== `refs/tags/${tag}` || event.sha !== sourceSha || !/^[a-f0-9]{40}$/.test(sourceSha)) throw new Error("Unsupported release event/repository/ref/source SHA");
  const repository = await api.repository();
  if (repository.full_name !== REPOSITORY || repository.private !== false || repository.visibility !== "public" || repository.fork !== false) throw new Error("Release destination must be the public same-source repository, not a fork");
  if ((await api.main()).protected !== true || !await api.ancestor(sourceSha)) throw new Error("Source must be reachable from protected main");
  if (await api.tagSha(tag) !== sourceSha) throw new Error("Tag/source SHA mismatch");
}

async function history(api) {
  const all = [];
  for (let page = 1; ; page++) {
    const batch = await api.releases(page);
    if (!Array.isArray(batch) || batch.length > 100) throw new Error("Invalid release history page");
    all.push(...batch);
    if (batch.length < 100) return all;
  }
}
function increasing(releases, tag) {
  const candidate = tagVersion(tag).split(".").map(BigInt);
  for (const release of releases) {
    if (release.draft || release.prerelease) continue;
    let version;
    try { version = stableVersion(tagVersion(release.tag_name)).split(".").map(BigInt); } catch { continue; }
    const different = candidate.findIndex((part, at) => part !== version[at]);
    if (different === -1 || candidate[different] < version[different]) throw new Error("Stable publication must strictly increase published history");
  }
}
function matching(release, tag, sha) {
  if (release.tag_name !== tag || release.target_commitish !== sha || release.body !== provenance(sha) || release.prerelease !== false) throw new Error("Release tag/source provenance mismatch; overwrites forbidden");
}
export function validateRecord({ record, files, tag, sourceSha }) {
  const names = assetNames(tag);
  if (!sameNames(Object.keys(files), names) || !sameNames(Object.keys(record.digests ?? {}), names)) throw new Error("Missing or extra qualified assets");
  if (record.schemaVersion !== 1 || record.sourceSha !== sourceSha || record.workingTreeDirty !== false || record.tag !== tag
    || record.version !== tagVersion(tag) || record.identifier !== "dev.teamarness.desktop" || record.architecture !== "arm64" || !/^[a-f0-9]{64}$/.test(record.appDigest)) throw new Error("Untrusted qualification record/source SHA");
  for (const name of names) if (!Buffer.isBuffer(files[name]) || files[name].length === 0 || digest(files[name]) !== record.digests[name]) throw new Error(`Qualified digest mismatch: ${name}`);
  return names;
}
async function readback({ api, release, names, files, qualify }) {
  const assets = await api.assets(release.id);
  if (!sameNames(assets.map((asset) => asset.name), names)) throw new Error("Missing or extra draft assets");
  const downloaded = {};
  for (const asset of assets) {
    const expected = files[asset.name];
    if (asset.size !== expected.length) throw new Error("Readback size mismatch");
    downloaded[asset.name] = await api.download(asset, expected.length);
    if (digest(downloaded[asset.name]) !== digest(expected)) throw new Error("Readback digest mismatch");
  }
  await qualify(downloaded);
  return assets;
}

// Caller supplies one serialized publish job. Rechecks defend delayed builds and stale drafts;
// GitHub's workflow concurrency and protected tag/environment rules are operational prerequisites.
export async function publishRelease(options) {
  const { api, event, tag, sourceSha, files, record, qualify, dryRun = false } = options;
  const names = validateRecord(options);
  await preflight({ api, event, tag, sourceSha });
  let releases = await history(api);
  const matches = releases.filter((release) => release.tag_name === tag);
  if (matches.length > 1) throw new Error("Ambiguous release tag");
  let release = matches[0];
  if (release) matching(release, tag, sourceSha);
  if (release && !release.draft) {
    await readback({ api, release, names, files, qualify });
    return { status: "already-published" }; // Never mutate a public release, even after response loss.
  }
  increasing(releases, tag);
  if (dryRun) { await qualify(files); return { status: "dry-run" }; }
  if (!release) release = await api.createDraft({ tag_name: tag, target_commitish: sourceSha, name: tag, body: provenance(sourceSha), draft: true, prerelease: false });
  matching(release, tag, sourceSha);
  if (!release.draft) throw new Error("Published overwrite forbidden");
  for (const asset of await api.assets(release.id)) await api.deleteAsset(asset.id);
  for (const name of names) await api.upload(release.id, name, files[name]);
  const verified = await readback({ api, release, names, files, qualify });
  await preflight({ api, event, tag, sourceSha });
  releases = await history(api);
  increasing(releases, tag);
  const current = await api.release(release.id);
  matching(current, tag, sourceSha);
  if (!current.draft) throw new Error("Draft changed during qualification");
  const finalAssets = await api.assets(release.id);
  if (!sameNames(finalAssets.map((a) => a.name), names) || finalAssets.some((a) => !verified.some((v) => v.id === a.id && v.name === a.name && v.size === a.size))) throw new Error("Required assets changed after readback");
  await api.publish(release.id, { draft: false, prerelease: false, make_latest: "true" });
  return { status: "published" }; // No remote mutation follows this commit point.
}
