import { invoke } from "@tauri-apps/api/core";
import type {
  Installation,
  Invitation,
  Membership,
  Workspace,
  WorkspaceState,
} from "./contracts";

export interface TeamarnessBridge {
  installationDetails(): Promise<Installation>;
  listWorkspaces(): Promise<readonly Workspace[]>;
  createWorkspace(name: string): Promise<WorkspaceState>;
  createWorkspaceInvitation(workspaceId: string): Promise<Invitation>;
  joinWorkspace(invitation: string): Promise<WorkspaceState>;
  currentMembership(workspaceId: string): Promise<Membership>;
}

export const bridge: TeamarnessBridge = Object.freeze({
  installationDetails: () => invoke<Installation>("installation_details"),
  listWorkspaces: () => invoke<Workspace[]>("list_workspaces"),
  createWorkspace: (name: string) => invoke<WorkspaceState>("create_workspace", { name }),
  createWorkspaceInvitation: (workspaceId: string) =>
    invoke<Invitation>("create_workspace_invitation", { workspaceId }),
  joinWorkspace: (invitation: string) => invoke<WorkspaceState>("join_workspace", { invitation }),
  currentMembership: (workspaceId: string) =>
    invoke<Membership>("current_membership", { workspaceId }),
});
