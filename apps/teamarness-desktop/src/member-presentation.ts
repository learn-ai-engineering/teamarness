import type { Installation, Membership, PersonNameSnapshot } from "./contracts";

export type MemberPresentation = Readonly<{
  name: string;
  detail: string;
  role: "Manager" | "Member";
}>;

export async function presentMembers(
  _installation: Installation,
  membership: Membership,
  names?: PersonNameSnapshot,
): Promise<readonly MemberPresentation[]> {
  const scoped = names?.workspaceId === membership.workspaceId ? names.persons : [];
  const counts = new Map<string, number>();
  for (const person of scoped) {
    if (person.name) counts.set(person.name, (counts.get(person.name) ?? 0) + 1);
  }
  return Promise.all(membership.members.map(async (member) => {
    const person = scoped.find(({ identity }) => identity === member.identity);
    // Core's unnamed fingerprint is the first 12 characters of the Installation identity.
    const fingerprint = person?.fingerprint ?? member.identity.slice(0, 12);
    return {
      name: person?.name ?? `Member ${fingerprint}`,
      detail: `${(counts.get(person?.name ?? "") ?? 0) > 1 ? "Duplicate name · " : ""}Identity: ${member.identity}`,
      role: member.role === "manager" ? "Manager" : "Member",
    };
  }));
}
