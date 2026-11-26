import type { Installation, Membership, PrincipalsSnapshot } from "./contracts";

export type MemberPresentation = Readonly<{
  name: string;
  /** Another Principal in the Workspace has the same name. Identities are never shown. */
  duplicateName: boolean;
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
    const steward = scoped.find(({ principalId }) => principalId === member.stewardPrincipalId);
    return {
      // A name not yet received shows as unnamed; identities and fingerprints are never shown.
      name: principal?.name ?? (member.kind === "agent" ? "Unnamed Agent" : "Unnamed Member"),
      duplicateName: principal?.name != null && (counts.get(principal.name) ?? 0) > 1,
      role: member.role === "manager" ? "Manager" : "Member",
      indicator: member.kind === "agent" ? `Agent · stewarded by ${steward?.name ?? "an unnamed Member"}` : null,
      canRemoveAgent: member.kind === "agent" && (self?.role === "manager" || (self?.kind === "person" && self.principalId === member.stewardPrincipalId)),
      canRenameAgent: member.kind === "agent" && self?.kind === "person" && self.principalId === member.stewardPrincipalId,
      agentId: member.kind === "agent" ? member.principalId : null,
    };
  });
}
