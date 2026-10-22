import { Channel, invoke } from "@tauri-apps/api/core";
import type {
  Installation,
  Invitation,
  Membership,
  PrincipalsSnapshot,
  Workspace,
  WorkspaceState,
} from "./contracts";

export interface TeamarnessBridge {
  installationDetails(): Promise<Installation>;
  listWorkspaces(): Promise<readonly Workspace[]>;
  createWorkspace(name: string, personName: string): Promise<WorkspaceState>;
  createWorkspaceInvitation(workspaceId: string): Promise<Invitation>;
  joinWorkspace(invitation: string, personName: string): Promise<WorkspaceState>;
  renameWorkspacePrincipal(workspaceId: string, principalId: string, name: string): Promise<void>;
  addWorkspaceAgent(workspaceId: string, name: string): Promise<void>;
  removeWorkspaceAgent(workspaceId: string, agentId: string): Promise<void>;
  principalsSnapshot(workspaceId: string): Promise<PrincipalsSnapshot>;
  subscribePrincipals(workspaceId: string, afterCursor: string, listener: (snapshot: PrincipalsSnapshot) => void, terminated: () => void): Promise<void>;
  currentMembership(workspaceId: string): Promise<Membership>;
}

export const bridge: TeamarnessBridge = Object.freeze({
  installationDetails: () => invoke<Installation>("installation_details"),
  listWorkspaces: () => invoke<Workspace[]>("list_workspaces"),
  createWorkspace: (name: string, personName: string) => invoke<WorkspaceState>("create_workspace", { name, personName }),
  createWorkspaceInvitation: (workspaceId: string) =>
    invoke<Invitation>("create_workspace_invitation", { workspaceId }),
  joinWorkspace: (invitation: string, personName: string) => invoke<WorkspaceState>("join_workspace", { invitation, personName }),
  renameWorkspacePrincipal: (workspaceId: string, principalId: string, name: string) => invoke<void>("rename_workspace_principal", { workspaceId, principalId, name }),
  addWorkspaceAgent: (workspaceId: string, name: string) => invoke<void>("add_workspace_agent", { workspaceId, name }),
  removeWorkspaceAgent: (workspaceId: string, agentId: string) => invoke<void>("remove_workspace_agent", { workspaceId, agentId }),
  principalsSnapshot: (workspaceId: string) => invoke<PrincipalsSnapshot>("principals_snapshot", { workspaceId }),
  subscribePrincipals: async (workspaceId: string, afterCursor: string, listener: (snapshot: PrincipalsSnapshot) => void, terminated: () => void) => {
    const onEvent = new Channel<{ kind: "snapshot"; snapshot: PrincipalsSnapshot } | { kind: "terminated" }>((event) => {
      if (event.kind === "terminated") terminated();
      else listener(event.snapshot);
    });
    await invoke("subscribe_principals", { workspaceId, afterCursor, onEvent });
  },
  currentMembership: (workspaceId: string) =>
    invoke<Membership>("current_membership", { workspaceId }),
});
