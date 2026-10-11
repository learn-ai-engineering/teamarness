import { Channel, invoke } from "@tauri-apps/api/core";
import type {
  Installation,
  Invitation,
  Membership,
  PersonNameSnapshot,
  Workspace,
  WorkspaceState,
} from "./contracts";

export interface TeamarnessBridge {
  installationDetails(): Promise<Installation>;
  listWorkspaces(): Promise<readonly Workspace[]>;
  createWorkspace(name: string, personName: string): Promise<WorkspaceState>;
  createWorkspaceInvitation(workspaceId: string): Promise<Invitation>;
  joinWorkspace(invitation: string, personName: string): Promise<WorkspaceState>;
  setOwnPersonName(workspaceId: string, name: string): Promise<void>;
  personNameSnapshot(workspaceId: string): Promise<PersonNameSnapshot>;
  subscribePersonNames(workspaceId: string, afterCursor: string, listener: (snapshot: PersonNameSnapshot) => void): Promise<void>;
  currentMembership(workspaceId: string): Promise<Membership>;
}

export const bridge: TeamarnessBridge = Object.freeze({
  installationDetails: () => invoke<Installation>("installation_details"),
  listWorkspaces: () => invoke<Workspace[]>("list_workspaces"),
  createWorkspace: (name: string, personName: string) => invoke<WorkspaceState>("create_workspace", { name, personName }),
  createWorkspaceInvitation: (workspaceId: string) =>
    invoke<Invitation>("create_workspace_invitation", { workspaceId }),
  joinWorkspace: (invitation: string, personName: string) => invoke<WorkspaceState>("join_workspace", { invitation, personName }),
  setOwnPersonName: (workspaceId: string, name: string) => invoke<void>("set_own_person_name", { workspaceId, name }),
  personNameSnapshot: (workspaceId: string) => invoke<PersonNameSnapshot>("person_name_snapshot", { workspaceId }),
  subscribePersonNames: async (workspaceId: string, afterCursor: string, listener: (snapshot: PersonNameSnapshot) => void) => {
    const onEvent = new Channel<PersonNameSnapshot>(listener);
    await invoke("subscribe_person_names", { workspaceId, afterCursor, onEvent });
  },
  currentMembership: (workspaceId: string) =>
    invoke<Membership>("current_membership", { workspaceId }),
});
