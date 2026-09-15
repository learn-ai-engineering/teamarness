import type { Installation, MemberRole, Membership } from "./contracts";

export const MEMBER_ALIAS_VERSION = "v1" as const;

export const MEMBER_ALIAS_ADJECTIVES_V1 = Object.freeze([
  "Agile", "Amber", "Bold", "Bright", "Calm", "Clever", "Cool", "Curious",
  "Daring", "Gentle", "Golden", "Happy", "Kind", "Lively", "Lucky", "Merry",
  "Nimble", "Patient", "Quiet", "Rapid", "Ready", "Silver", "Steady", "Sunny",
  "Swift", "Thoughtful", "Tranquil", "Vivid", "Warm", "Wise", "Witty", "Young",
] as const);

export const MEMBER_ALIAS_ANIMALS_V1 = Object.freeze([
  "Albatross", "Badger", "Bear", "Beaver", "Bison", "Crane", "Dolphin", "Falcon",
  "Finch", "Fox", "Gecko", "Heron", "Koala", "Lark", "Lynx", "Marten",
  "Otter", "Owl", "Panda", "Penguin", "Rabbit", "Raven", "Seal", "Sparrow",
  "Stag", "Swan", "Tiger", "Turtle", "Whale", "Wolf", "Wren", "Yak",
] as const);

export type MemberPresentation = Readonly<{
  name: string;
  detail: "This Installation" | "Presentation-only member alias" | "Presentation-only member fingerprint";
  role: "Manager" | "Member";
}>;

type DigestedMember = Readonly<{
  digest: string;
  alias: string | null;
  isLocal: boolean;
  role: MemberRole;
}>;

export function isCanonicalPublicIdentity(identity: string): boolean {
  return /^[0-9a-f]{64}$/.test(identity);
}

export async function presentationDigestHex(identity: string): Promise<string> {
  const bytes = new TextEncoder().encode(identity);
  const digest = await globalThis.crypto.subtle.digest("SHA-256", bytes);
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

export async function presentMembers(
  installation: Installation,
  membership: Membership,
): Promise<readonly MemberPresentation[]> {
  const digested = await Promise.all(membership.members.map(async (member): Promise<DigestedMember> => {
    const digest = await presentationDigestHex(member.identity);
    const isLocal = member.identity === installation.identity;
    return {
      digest,
      alias: !isLocal && isCanonicalPublicIdentity(member.identity) ? aliasFromDigest(digest) : null,
      isLocal,
      role: member.role,
    };
  }));
  const aliasCounts = new Map<string, number>();
  for (const member of digested) {
    if (member.alias) aliasCounts.set(member.alias, (aliasCounts.get(member.alias) ?? 0) + 1);
  }
  return digested.map((member): MemberPresentation => {
    if (member.isLocal) {
      return { name: installation.displayName, detail: "This Installation", role: roleLabel(member.role) };
    }
    if (member.alias && aliasCounts.get(member.alias) === 1) {
      return { name: member.alias, detail: "Presentation-only member alias", role: roleLabel(member.role) };
    }
    return {
      name: `Member ${member.digest.slice(0, 8)}`,
      detail: "Presentation-only member fingerprint",
      role: roleLabel(member.role),
    };
  });
}

function aliasFromDigest(digest: string): string {
  const adjectiveWord = Number.parseInt(digest.slice(0, 8), 16);
  const animalWord = Number.parseInt(digest.slice(8, 16), 16);
  const adjective = MEMBER_ALIAS_ADJECTIVES_V1[adjectiveWord % MEMBER_ALIAS_ADJECTIVES_V1.length];
  const animal = MEMBER_ALIAS_ANIMALS_V1[animalWord % MEMBER_ALIAS_ANIMALS_V1.length];
  return `${adjective} ${animal}`;
}

function roleLabel(role: MemberRole): "Manager" | "Member" {
  return role === "manager" ? "Manager" : "Member";
}
