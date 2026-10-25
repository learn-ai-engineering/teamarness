import { isDeepStrictEqual } from "node:util";
import { tagVersion } from "./version.mjs";

export const ARCHIVE_LIMIT = 512 * 1024 * 1024;
export const FEED_LIMIT = 64 * 1024;
export const SIGNATURE_LIMIT = 4096;
export const TARGETS = ["darwin-aarch64", "darwin-aarch64-app"];
export const ENDPOINT = "https://github.com/learn-ai-engineering/teamarness/releases/latest/download/latest.json";

// Names follow the reviewed release configuration, never a build-output glob.
export function assetNames(tag) {
  const version = tagVersion(tag);
  return [`Teamarness_${version}_aarch64.dmg`, "Teamarness.app.tar.gz", "Teamarness.app.tar.gz.sig", "latest.json"];
}

function base64(value, length) {
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(value) || Buffer.from(value, "base64").toString("base64") !== value
    || (length !== undefined && Buffer.from(value, "base64").length !== length)) throw new Error("Malformed signature encoding");
  return Buffer.from(value, "base64");
}

// Structural validation only. The official native verifier remains cryptographic authority.
export function validateSignature(signature, version) {
  if (typeof signature !== "string" || Buffer.byteLength(signature) > SIGNATURE_LIMIT) throw new Error("Invalid signature size");
  const decoded = base64(signature.trim()).toString("utf8");
  const lines = decoded.trimEnd().split("\n");
  if (lines.length !== 4 || !lines[0].startsWith("untrusted comment: ") || !lines[2].startsWith("trusted comment: ")) throw new Error("Malformed signature text");
  const packet = base64(lines[1], 74);
  if (packet.subarray(0, 2).toString() !== "ED") throw new Error("Unsupported signature algorithm");
  base64(lines[3], 64);
  const versions = [...lines[2].matchAll(/(?:\t| )version:([^\s]+)/g)];
  if (versions.length !== 1 || versions[0][1] !== version) throw new Error("Signature version differs from expected tag");
}

export function createManifest({ tag, signature, notes = "" }) {
  const version = tagVersion(tag);
  validateSignature(signature, version);
  if (typeof notes !== "string" || Buffer.byteLength(notes) > 4096 || /[<>\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/u.test(notes)) throw new Error("Notes must be bounded plain text");
  const [, archive] = assetNames(tag);
  const platform = { url: `https://github.com/learn-ai-engineering/teamarness/releases/download/${tag}/${archive}`, signature };
  const manifest = { version, notes, platforms: Object.fromEntries(TARGETS.map((target) => [target, { ...platform }])) };
  if (Buffer.byteLength(JSON.stringify(manifest)) > FEED_LIMIT) throw new Error("Manifest exceeds feed cap");
  return manifest;
}

export function validateManifest(manifest, { tag, signature }) {
  const expected = createManifest({ tag, signature, notes: manifest.notes });
  if (!isDeepStrictEqual(manifest, expected)) throw new Error("Manifest version, targets, URL or signature differs from release contract");
  return manifest;
}
