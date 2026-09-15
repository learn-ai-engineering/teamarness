import type { TeamarnessBridge } from "./bridge";
import { isBridgeError } from "./contracts";
import type { BridgeErrorCode, Installation, Membership, Workspace } from "./contracts";
import type { ShellRoute } from "./shell";

export type ShellModel = Readonly<{
  installation: Installation | null;
  workspaces: readonly Workspace[];
  memberships: Readonly<Record<string, Membership>>;
  activeWorkspaceId: string | null;
  loading: boolean;
  error: BridgeErrorCode | null;
  notice: string | null;
}>;

const initialModel: ShellModel = {
  installation: null,
  workspaces: [],
  memberships: {},
  activeWorkspaceId: null,
  loading: false,
  error: null,
  notice: null,
};

export class ShellRenderer {
  public constructor(
    private readonly controls: Pick<HTMLElement, "innerHTML">,
    private readonly persistentPackagePane?: object,
  ) {}

  public packagePane(): object | undefined {
    return this.persistentPackagePane;
  }

  public render(model: ShellModel, route: ShellRoute = { kind: "workspace-chooser" }): void {
    this.controls.innerHTML = renderShell(model, route);
  }
}

export class ShellController {
  private model: ShellModel = initialModel;

  public constructor(
    private readonly bridge: TeamarnessBridge,
    private readonly update: (model: ShellModel) => void,
    private readonly copy: (value: string) => Promise<void> = (value) => navigator.clipboard.writeText(value),
  ) {}

  public snapshot(): ShellModel { return this.model; }

  public selectWorkspace(workspaceId: string): void {
    if (!this.model.workspaces.some(({ id }) => id === workspaceId)) return;
    this.set({ activeWorkspaceId: workspaceId });
  }

  public async load(): Promise<void> {
    await this.perform(async () => {
      const [installation, workspaces] = await Promise.all([
        this.bridge.installationDetails(),
        this.bridge.listWorkspaces(),
      ]);
      const memberships = await this.loadMemberships(workspaces);
      const activeWorkspaceId = workspaces.some(({ id }) => id === this.model.activeWorkspaceId)
        ? this.model.activeWorkspaceId
        : workspaces[0]?.id ?? null;
      this.set({ installation, workspaces, memberships, activeWorkspaceId });
    });
  }

  public async createWorkspace(name: string): Promise<void> {
    await this.perform(async () => {
      const created = await this.bridge.createWorkspace(name);
      this.set({
        workspaces: [...this.model.workspaces, created.workspace],
        memberships: { ...this.model.memberships, [created.workspace.id]: created.membership },
        activeWorkspaceId: created.workspace.id,
        notice: `Created ${created.workspace.name}`,
      });
    });
  }

  public async copyInvitation(workspaceId: string): Promise<void> {
    await this.perform(async () => {
      const invitation = await this.bridge.createWorkspaceInvitation(workspaceId);
      await this.copy(invitation.value);
      this.set({ notice: "Sensitive bearer admission material copied" });
    });
  }

  public async refreshMembership(workspaceId: string): Promise<void> {
    await this.perform(async () => {
      const membership = await this.bridge.currentMembership(workspaceId);
      this.set({
        memberships: { ...this.model.memberships, [workspaceId]: membership },
        notice: "Membership refreshed",
      });
    });
  }

  public async joinWorkspace(invitation: string): Promise<void> {
    await this.perform(async () => {
      const joined = await this.bridge.joinWorkspace(invitation);
      const existing = this.model.workspaces.filter(({ id }) => id !== joined.workspace.id);
      this.set({
        workspaces: [...existing, joined.workspace],
        memberships: { ...this.model.memberships, [joined.workspace.id]: joined.membership },
        activeWorkspaceId: joined.workspace.id,
        notice: `Joined ${joined.workspace.name}`,
      });
    });
  }

  private async loadMemberships(workspaces: readonly Workspace[]): Promise<Record<string, Membership>> {
    const pairs = await Promise.all(workspaces.map(async (workspace) =>
      [workspace.id, await this.bridge.currentMembership(workspace.id)] as const));
    return Object.fromEntries(pairs);
  }

  private async perform(operation: () => Promise<void>): Promise<void> {
    this.set({ loading: true, error: null, notice: null });
    try {
      await operation();
    } catch (error: unknown) {
      this.set({ error: isBridgeError(error) ? error.code : "invalid_input" });
    } finally {
      this.set({ loading: false });
    }
  }

  private set(change: Partial<ShellModel>): void {
    this.model = { ...this.model, ...change };
    this.update(this.model);
  }
}

const escape = (value: string): string => value
  .replaceAll("&", "&amp;")
  .replaceAll("<", "&lt;")
  .replaceAll(">", "&gt;")
  .replaceAll('"', "&quot;");

export function renderShell(
  model: ShellModel,
  route: ShellRoute = { kind: "workspace-chooser" },
): string {
  const status = `<p class="shell-status" role="status" aria-live="polite">${model.loading ? "Working…" : escape(model.notice ?? "")}</p>`;
  const error = `<div class="shell-alert" role="alert">${model.error
    ? `<p class="error">${escape(errorMessage(model.error))}</p><button id="retry" type="button">Retry</button>`
    : ""}</div>`;
  const feedback = `${status}${error}`;

  if (route.kind === "workspace-chooser") {
    const workspaces = model.workspaces.map((workspace, index) =>
      `<li><button type="button" data-choose-workspace="${index}">${escape(workspace.name)}</button></li>`).join("");
    return `${feedback}<section class="shell-screen" aria-labelledby="chooser-heading"><h1 id="chooser-heading">Choose a Workspace</h1>
      <ul class="workspace-list">${workspaces || "<li>No Workspaces yet.</li>"}</ul>
      <section><h2>Create a Workspace</h2><form id="create-workspace"><label for="workspace-name">Workspace name</label>
      <input id="workspace-name" required maxlength="64"><button ${model.loading ? "disabled" : ""}>Create</button></form></section>
      <section><h2>Join a Workspace</h2><form id="join-workspace"><label for="invitation-value">Invitation</label>
      <textarea id="invitation-value" required maxlength="8192"></textarea><button ${model.loading ? "disabled" : ""}>Join</button></form></section></section>`;
  }

  const workspaceId = route.kind === "package-instance" ? route.instance.workspaceId : route.workspaceId;
  const workspace = model.workspaces.find(({ id }) => id === workspaceId);
  if (!workspace) return `${feedback}<section class="shell-screen"><h1>Workspace unavailable</h1></section>`;

  if (route.kind === "workspace-invitations") {
    return `${feedback}<section class="shell-screen"><h1>Invitations</h1><p>Create an invitation for ${escape(workspace.name)}.</p>
      <button type="button" id="copy-invitation" ${model.loading ? "disabled" : ""}>Copy invitation</button>
      <p class="sensitive">Invitations are sensitive bearer admission material.</p></section>`;
  }
  if (route.kind === "workspace-settings") {
    return `${feedback}<section class="shell-screen"><h1>Workspace settings</h1><p>${escape(workspace.name)}</p>
      <p>Package Instance management will appear here.</p></section>`;
  }

  const membership = model.memberships[workspace.id];
  const members = membership?.members.map((member) =>
    `<li><span class="identifier">${escape(member.identity)}</span> — ${member.role}</li>`).join("") ?? "";
  return `${feedback}<section class="shell-screen"><h1>${escape(workspace.name)}</h1><h2>Members</h2>
    <ol class="members">${members || "<li>Membership is loading.</li>"}</ol>
    <button type="button" id="refresh-membership" ${model.loading ? "disabled" : ""}>Refresh members</button></section>`;
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
