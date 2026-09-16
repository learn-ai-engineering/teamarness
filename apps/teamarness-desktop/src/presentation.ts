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

export type PackageInstancePresentation = Readonly<{
  name: string;
  availability: string;
  actionIndex: number;
  installationLocal: boolean;
  canSuspend: boolean;
  canRecover: boolean;
  canDispose: boolean;
}>;

export type PackageNavigationPresentation = Readonly<{
  teamPackages: readonly PackageInstancePresentation[];
  privatePackages: readonly PackageInstancePresentation[];
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
    .flatMap((instance, actionIndex) => {
      const catalog = snapshot.packages.find(({ identity }) => identity === instance.key.packageIdentity);
      if (!catalog) return [];
      const descriptor = lifecycleDescriptor(instance.lifecycle);
      return [{
        name: catalog.displayName,
        actionIndex,
        installationLocal: catalog.source === "local",
        ...descriptor,
      }];
    });
  return {
    teamPackages: rows.filter(({ installationLocal }) => !installationLocal),
    privatePackages: rows.filter(({ installationLocal }) => installationLocal),
  };
}

type LifecycleDescriptor = Pick<
  PackageInstancePresentation,
  "availability" | "canSuspend" | "canRecover" | "canDispose"
>;

function lifecycleDescriptor(
  lifecycle: PackageHostSnapshot["instances"][number]["lifecycle"],
): LifecycleDescriptor {
  switch (lifecycle) {
    case "admitted":
      return { availability: "Getting ready", canSuspend: false, canRecover: false, canDispose: true };
    case "activating":
    case "recovering":
      return { availability: "Getting ready", canSuspend: false, canRecover: false, canDispose: false };
    case "active":
      return { availability: "Available", canSuspend: true, canRecover: false, canDispose: true };
    case "suspending":
      return { availability: "Pausing", canSuspend: false, canRecover: false, canDispose: false };
    case "suspended":
      return { availability: "Paused", canSuspend: false, canRecover: true, canDispose: true };
    case "failed":
    case "crash_loop":
      return { availability: "Needs recovery", canSuspend: false, canRecover: true, canDispose: true };
    case "disposing":
      return { availability: "Removing", canSuspend: false, canRecover: false, canDispose: false };
    case "disposed":
      return { availability: "Unavailable", canSuspend: false, canRecover: false, canDispose: false };
  }
}

function errorMessage(code: BridgeErrorCode): string {
  const messages: Record<BridgeErrorCode, string> = {
    invalid_input: "The supplied value is invalid.",
    authentication: "Workspace Invitation authentication failed.",
    expired: "The Workspace Invitation has expired.",
    already_used: "Another Installation already used this Workspace Invitation.",
    readiness: "Workspace Invitation addressing is not ready. Try again.",
    timeout: "Workspace onboarding timed out. Try again.",
    overload: "Workspace onboarding is busy. Try again.",
    invalid_bootstrap: "The received Workspace history is invalid.",
    local_conflict: "Local Workspace state conflicts with this Workspace Invitation.",
    admission_no_longer_current: "This admission is no longer current.",
    uncertain_storage_outcome: "Storage outcome is uncertain. Restart Teamarness.",
    shutdown: "Teamarness is shutting down. Try again after reopening.",
  };
  return messages[code];
}
