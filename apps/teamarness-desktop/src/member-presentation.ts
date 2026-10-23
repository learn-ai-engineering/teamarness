import type { Installation, Membership, PrincipalsSnapshot } from "./contracts";

export type MemberPresentation = Readonly<{
  name: string;
  detail: string;
  role: "Manager" | "Member";
  indicator: string | null;
  canRemoveAgent: boolean;
  canRenameAgent: boolean;
  agentId: string | null;
}>;

export async function presentMembers(
  installation: Installation,
  membership: Membership,
  snapshot?: PrincipalsSnapshot,
): Promise<readonly MemberPresentation[]> {
  const scoped = snapshot?.workspaceId === membership.workspaceId ? snapshot.principals : [];
  const counts = new Map<string, number>();
  for (const principal of scoped) {
    if (principal.name) counts.set(principal.name, (counts.get(principal.name) ?? 0) + 1);
  }
  const self = membership.members.find(({ identity }) => identity === installation.identity);
  return membership.members.map((member) => {
    const principal = scoped.find(({ principalId }) => principalId === member.principalId);
    const fingerprint = principal?.fingerprint ?? member.identity.slice(0, 12);
    const steward = scoped.find(({ principalId }) => principalId === member.stewardPrincipalId);
    return {
      name: principal?.name ?? `${member.kind === "agent" ? "Agent" : "Member"} ${fingerprint}`,
      detail: `${(counts.get(principal?.name ?? "") ?? 0) > 1 ? "Duplicate name · " : ""}Identity: ${member.identity}`,
      role: member.role === "manager" ? "Manager" : "Member",
      indicator: member.kind === "agent" ? `Agent · stewarded by ${steward?.name ?? (steward ? `Member ${steward.fingerprint}` : "unknown member")}` : null,
      canRemoveAgent: member.kind === "agent" && (self?.role === "manager" || (self?.kind === "person" && self.principalId === member.stewardPrincipalId)),
      canRenameAgent: member.kind === "agent" && self?.kind === "person" && self.principalId === member.stewardPrincipalId,
      agentId: member.kind === "agent" ? member.principalId : null,
    };
  });
}
