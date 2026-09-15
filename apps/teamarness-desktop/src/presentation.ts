import type { BridgeErrorCode } from "./contracts";
import { presentMembers, type MemberPresentation } from "./member-presentation";
import type { PackageHostSnapshot } from "./package-contracts";
import type { ShellModel } from "./view";

export type ShellPresentation = Readonly<{
  installationName: string | null;
  workspaceChoices: readonly Readonly<{ name: string }>[];
  activeWorkspaceIndex: number | null;
  workspace: Readonly<{
    name: string;
    members: readonly MemberPresentation[];
    memberCount: number;
    membershipLoaded: boolean;
  }> | null;
  loading: boolean;
  notice: string | null;
  error: Readonly<{ message: string; canRetry: boolean }> | null;
}>;

export type PackageNavigationPresentation = Readonly<{
  teamPackages: readonly Readonly<{ name: string; actionIndex: number }>[];
  privatePackages: readonly Readonly<{ name: string; actionIndex: number }>[];
}>;

export async function buildShellPresentation(
  model: ShellModel,
  selectedWorkspaceId: string | null,
): Promise<ShellPresentation> {
  const workspace = model.workspaces.find(({ id }) => id === selectedWorkspaceId) ?? null;
  const membership = workspace ? model.memberships[workspace.id] : undefined;
  const members = model.installation && membership
    ? await presentMembers(model.installation, membership)
    : [];
  const activeWorkspaceIndex = model.activeWorkspaceId === null
    ? null
    : model.workspaces.findIndex(({ id }) => id === model.activeWorkspaceId);

  return {
    installationName: model.installation?.displayName ?? null,
    workspaceChoices: model.workspaces.map(({ name }) => ({ name })),
    activeWorkspaceIndex: activeWorkspaceIndex === -1 ? null : activeWorkspaceIndex,
    workspace: workspace ? {
      name: workspace.name,
      members,
      memberCount: members.length,
      membershipLoaded: membership !== undefined && model.installation !== null,
    } : null,
    loading: model.loading,
    notice: model.notice,
    error: model.error ? { message: errorMessage(model.error), canRetry: true } : null,
  };
}

export function buildPackageNavigationPresentation(
  snapshot: PackageHostSnapshot | null,
  activeWorkspaceId: string,
): PackageNavigationPresentation {
  if (!snapshot) return { teamPackages: [], privatePackages: [] };
  const rows = snapshot.instances
    .filter(({ key }) => key.workspaceId === activeWorkspaceId)
    .map((instance, actionIndex) => {
      const catalog = snapshot.packages.find(({ identity }) => identity === instance.key.packageIdentity);
      return {
        name: catalog?.displayName ?? "Unavailable Package",
        source: catalog?.source ?? "bundled",
        actionIndex,
      };
    });
  return {
    teamPackages: rows
      .filter(({ source }) => source === "bundled")
      .map(({ name, actionIndex }) => ({ name, actionIndex })),
    privatePackages: rows
      .filter(({ source }) => source === "local")
      .map(({ name, actionIndex }) => ({ name, actionIndex })),
  };
}

function errorMessage(code: BridgeErrorCode): string {
  const messages: Record<BridgeErrorCode, string> = {
    invalid_input: "The supplied value is invalid.",
    authentication: "Workspace invitation authentication failed.",
    expired: "The Workspace invitation has expired.",
    already_used: "Another Installation already used this invitation.",
    readiness: "Invitation addressing is not ready. Try again.",
    timeout: "Workspace onboarding timed out. Try again.",
    overload: "Workspace onboarding is busy. Try again.",
    invalid_bootstrap: "The received Workspace history is invalid.",
    local_conflict: "Local Workspace state conflicts with this invitation.",
    admission_no_longer_current: "This admission is no longer current.",
    uncertain_storage_outcome: "Storage outcome is uncertain. Restart Teamarness.",
    shutdown: "Teamarness is shutting down. Try again after reopening.",
  };
  return messages[code];
}
