import type { TeamarnessBridge } from "./bridge";
import { isBridgeError } from "./contracts";
import type { BridgeErrorCode, Installation, Membership, Workspace } from "./contracts";
import { buildShellPresentation, type ShellPresentation } from "./presentation";
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

  public async render(model: ShellModel, route: ShellRoute = { kind: "workspace-chooser" }): Promise<void> {
    const presentation = await buildShellPresentation(model, routeWorkspaceId(route));
    this.controls.innerHTML = renderShell(presentation, route);
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
      this.set({ notice: "Workspace Invitation copied. Keep it private." });
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

export type ShellRendererInput = Readonly<{
  packageSettings?: string;
}>;

export function renderShell(
  presentation: ShellPresentation,
  route: ShellRoute,
  input: ShellRendererInput = {},
): string {
  const statusMessage = presentation.loading ? "Working…" : presentation.notice ?? "";
  const status = `<p class="shell-status" role="status" aria-live="polite">${escape(statusMessage)}</p>`;
  const error = `<div class="shell-alert" role="alert">${presentation.error
    ? `<p class="error">${escape(presentation.error.message)}</p>${presentation.error.canRetry ? '<button id="retry" type="button">Retry</button>' : ""}`
    : ""}</div>`;
  const feedback = `${status}${error}`;

  if (route.kind === "workspace-chooser") {
    const choices = presentation.workspaceChoices.map(({ name }, index) =>
      `<li><button type="button" data-choose-workspace="${index}">${escape(name)}</button></li>`).join("");
    const workspaceList = choices || (presentation.loading ? '<li class="empty-state">Loading Workspaces…</li>' : "");
    const workspaceListSection = workspaceList
      ? `<section class="screen-section" aria-labelledby="workspace-list-heading"><h2 id="workspace-list-heading">Your Workspaces</h2><ul class="workspace-list">${workspaceList}</ul></section>`
      : "";
    return `${feedback}<section class="shell-screen" aria-labelledby="chooser-heading"><header class="screen-heading"><p class="eyebrow">Workspaces</p><h1 id="chooser-heading">Create or join a Workspace</h1><p>Create a new Workspace, or join one with a Workspace Invitation.</p></header>
      ${workspaceListSection}
      <section class="screen-section"><h2>Create a Workspace</h2><form id="create-workspace"><label for="workspace-name">Workspace name</label>
      <input id="workspace-name" required maxlength="64" autocapitalize="off" autocorrect="off" spellcheck="false" writingsuggestions="false"><button ${presentation.loading ? "disabled" : ""}>Create</button></form></section>
      <section class="screen-section"><h2>Join a Workspace</h2><form id="join-workspace"><label for="invitation-value">Workspace Invitation</label>
      <textarea id="invitation-value" required maxlength="8192" autocapitalize="off" autocorrect="off" spellcheck="false" writingsuggestions="false"></textarea><button ${presentation.loading ? "disabled" : ""}>Join</button></form></section></section>`;
  }

  const workspace = presentation.workspace;
  if (!workspace) return `${feedback}<section class="shell-screen"><h1>Workspace unavailable</h1><p>Choose another Workspace or retry.</p></section>`;

  if (route.kind === "workspace-invitations") {
    return `${feedback}<section class="shell-screen"><header class="screen-heading"><p class="eyebrow">${escape(workspace.name)}</p><h1>Workspace Invitations</h1><p>Create a one-use Workspace Invitation.</p></header>
      <section class="screen-section"><h2>Invite another Installation</h2><button type="button" id="copy-invitation" ${presentation.loading ? "disabled" : ""}>Copy Workspace Invitation</button>
      <p class="sensitive">Workspace Invitations are sensitive bearer admission material. Share them privately.</p></section></section>`;
  }
  if (route.kind === "workspace-settings") {
    return `${feedback}<section class="shell-screen"><header class="screen-heading"><p class="eyebrow">${escape(workspace.name)}</p><h1>Workspace settings</h1><p>Workspace details are read-only in this version.</p></header>
      <section class="screen-section settings-panel" aria-labelledby="workspace-details-heading"><h2 id="workspace-details-heading">Workspace details</h2><dl><div><dt>Name</dt><dd>${escape(workspace.name)}</dd></div><div><dt>Members</dt><dd>${workspace.memberCount}</dd></div></dl></section>
      <section class="screen-section settings-panel" aria-labelledby="package-instances-heading"><h2 id="package-instances-heading">Package Instances</h2>${input.packageSettings ?? '<p class="empty-state">Package information is loading.</p>'}</section></section>`;
  }

  const memberRows = workspace.members.map((member) => `<li class="member-row"><span><strong>${escape(member.name)}</strong><small>${escape(member.detail)}</small></span><span class="member-role">${member.role}</span></li>`).join("");
  const members = memberRows || (workspace.membershipLoaded
    ? '<li class="empty-state">No Members are listed.</li>'
    : '<li class="empty-state">Membership is loading.</li>');
  return `${feedback}<section class="shell-screen"><header class="screen-heading"><p class="eyebrow">Workspace overview</p><h1>${escape(workspace.name)}</h1><p>${workspace.memberCount === 1 ? "1 Member" : `${workspace.memberCount} Members`}</p></header><section class="screen-section" aria-labelledby="members-heading"><div class="section-heading"><h2 id="members-heading">Members</h2>
    <button type="button" id="refresh-membership" ${presentation.loading ? "disabled" : ""}>Refresh members</button></div><ol class="members">${members}</ol></section></section>`;
}

function routeWorkspaceId(route: ShellRoute): string | null {
  return route.kind === "workspace-chooser"
    ? null
    : route.kind === "package-instance" ? route.instance.workspaceId : route.workspaceId;
}
