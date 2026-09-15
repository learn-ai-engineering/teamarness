import { describe, expect, it } from "vitest";
import type { Installation, Membership } from "./contracts";
import {
  MEMBER_ALIAS_ADJECTIVES_V1,
  MEMBER_ALIAS_ANIMALS_V1,
  MEMBER_ALIAS_VERSION,
  isCanonicalPublicIdentity,
  presentMembers,
  presentationDigestHex,
} from "./member-presentation";

const localIdentity = "a".repeat(64);
const remoteIdentity = "1".repeat(64);
const installation: Installation = { identity: localIdentity, displayName: "Your Mac" };

function membership(identities: readonly string[]): Membership {
  return {
    workspaceId: "00112233445566778899aabbccddeeff",
    epoch: "2",
    version: "b".repeat(64),
    members: identities.map((identity, index) => ({ identity, role: index === 0 ? "manager" : "member" })),
  };
}

describe("v1 Member presentation", () => {
  it("commits non-empty, immutable adjective and animal lists", () => {
    expect(MEMBER_ALIAS_VERSION).toBe("v1");
    expect(MEMBER_ALIAS_ADJECTIVES_V1).toHaveLength(32);
    expect(MEMBER_ALIAS_ANIMALS_V1).toHaveLength(32);
    expect(Object.isFrozen(MEMBER_ALIAS_ADJECTIVES_V1)).toBe(true);
    expect(Object.isFrozen(MEMBER_ALIAS_ANIMALS_V1)).toBe(true);
  });

  it("validates only canonical 64-character lowercase hexadecimal Public Identities", () => {
    expect(isCanonicalPublicIdentity(remoteIdentity)).toBe(true);
    expect(isCanonicalPublicIdentity("A".repeat(64))).toBe(false);
    expect(isCanonicalPublicIdentity("1".repeat(63))).toBe(false);
    expect(isCanonicalPublicIdentity(`${"1".repeat(63)}g`)).toBe(false);
  });

  it("hashes the exact identity string with fixed SHA-256 vectors", async () => {
    await expect(presentationDigestHex(remoteIdentity)).resolves.toBe(
      "3138bb9bc78df27c473ecfd1410f7bd45ebac1f59cf3ff9cfe4db77aab7aedd3",
    );
    await expect(presentationDigestHex("A".repeat(64))).resolves.toBe(
      "d53eda7a637c99cc7fb566d96e9fa109bf15c478410a3f5eb4d4c4e26cd081f6",
    );
  });

  it("uses the local Installation Display Name and stable remote aliases without reordering", async () => {
    const presented = await presentMembers(installation, membership([remoteIdentity, localIdentity]));
    expect(presented).toEqual([
      { name: "Vivid Whale", detail: "Presentation-only member alias", role: "Manager" },
      { name: "Your Mac", detail: "This Installation", role: "Member" },
    ]);
    await expect(presentMembers(installation, membership([remoteIdentity, localIdentity]))).resolves.toEqual(presented);
  });

  it("uses an exact-string fingerprint for invalid identity input without generating an alias", async () => {
    const [member] = await presentMembers(installation, membership(["not canonical"]));
    expect(member).toEqual({
      name: "Member 9edd606b",
      detail: "Presentation-only member fingerprint",
      role: "Manager",
    });
  });

  it("falls back every remote Member in a duplicate alias set", async () => {
    const first = "12".padStart(64, "0");
    const second = "3e".padStart(64, "0");
    const presented = await presentMembers(installation, membership([first, remoteIdentity, second]));
    expect(presented.map(({ name }) => name)).toEqual([
      "Member b23a54d1",
      "Vivid Whale",
      "Member 8ec2d8f1",
    ]);
    expect(presented.every(({ name }) => name !== "Patient Turtle")).toBe(true);
  });
});
