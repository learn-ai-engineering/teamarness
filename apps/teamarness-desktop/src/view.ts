import type { TeamarnessBridge } from "./bridge";
import { isBridgeError } from "./contracts";
import type { Installation, Membership, PrincipalsSnapshot, ShellErrorCode, Workspace } from "./contracts";
import { buildShellPresentation, type ShellPresentation } from "./presentation";
import {
  browserStatusMessageTimers,
  renderStatusMessage,
  renderStatusMessageSlot,
  STATUS_MESSAGE_HIDE_MS,
  type StatusMessageTimers,
} from "./status-message";
import type { ShellRoute } from "./shell";

export type ShellModel = Readonly<{
  installation: Installation | null;
  workspaces: readonly Workspace[];
  memberships: Readonly<Record<string, Membership>>;
  principals: Readonly<Record<string, PrincipalsSnapshot>>;
  activeWorkspaceId: string | null;
  loading: boolean;
  error: ShellErrorCode | null;
  notice: string | null;
  membershipRefreshUnavailable: string | null;
}>;

const initialModel: ShellModel = {
  installation: null,
  workspaces: [],
  memberships: {},
  principals: {},
  activeWorkspaceId: null,
  loading: false,
  error: null,
  notice: null,
  membershipRefreshUnavailable: null,
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
  private readonly watched = new Map<string, number>();
  private readonly principalRetries = new Map<string, ReturnType<typeof setTimeout>>();
  private readonly principalRetryDelays = new Map<string, number>();
  private nextWatch = 0;
  private disposed = false;
  private noticeTimer: unknown = null;

  public constructor(
    private readonly bridge: TeamarnessBridge,
    private readonly update: (model: ShellModel) => void,
    private readonly copy: (value: Promise<string>) => Promise<void> = writeClipboardTextWhenReady,
    private readonly timers: StatusMessageTimers = browserStatusMessageTimers,
  ) {}

  public snapshot(): ShellModel { return this.model; }

  public dispose(): void {
    this.disposed = true;
    this.watched.clear();
    for (const retry of this.principalRetries.values()) clearTimeout(retry);
    this.principalRetries.clear();
    this.cancelNoticeTimer();
  }

  /** Closes the error notification; the failed action can still be retried from its control. */
  public dismissError(): void {
    this.set({ error: null });
  }

  /** Closes the warning that the roster could not be refreshed after an Agent change. */
  public dismissMembershipRefreshWarning(): void {
    this.set({ membershipRefreshUnavailable: null });
  }

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
      this.set({ installation, workspaces, memberships: this.mergeMemberships(memberships), activeWorkspaceId });
      await Promise.all(workspaces.map(({ id }) => this.loadOptionalPrincipals(id)));
    });
  }

  public async createWorkspace(name: string, personName: string): Promise<void> {
    await this.perform(async () => {
      const created = await this.bridge.createWorkspace(name, personName);
      this.set({
        workspaces: [...this.model.workspaces, created.workspace],
        memberships: this.mergeMemberships({ [created.workspace.id]: created.membership }),
        activeWorkspaceId: created.workspace.id,
        notice: `Created ${created.workspace.name}`,
      });
      await this.loadOptionalPrincipals(created.workspace.id);
    });
  }

  public async copyInvitation(workspaceId: string): Promise<void> {
    await this.perform(async () => {
      const invitation = this.bridge.createWorkspaceInvitation(workspaceId);
      const [created, copied] = await Promise.allSettled([
        invitation,
        this.copy(invitation.then(({ value }) => value)),
      ]);
      // A failed create also fails the copy; report the cause, not its echo.
      if (created.status === "rejected") throw created.reason;
      if (copied.status === "rejected") {
        this.set({ error: "clipboard_write" });
        return;
      }
      this.set({ notice: "Workspace Invitation copied. Keep it private." });
    });
  }

  public async refreshMembership(workspaceId: string): Promise<void> {
    await this.perform(async () => {
      const membership = await this.bridge.currentMembership(workspaceId);
      this.set({
        memberships: this.mergeMemberships({ [workspaceId]: membership }),
        notice: "Membership refreshed",
        membershipRefreshUnavailable: this.model.membershipRefreshUnavailable === workspaceId
          ? null : this.model.membershipRefreshUnavailable,
      });
      await this.loadOptionalPrincipals(workspaceId);
    });
  }

  public async joinWorkspace(invitation: string, personName: string): Promise<void> {
    await this.perform(async () => {
      const joined = await this.bridge.joinWorkspace(invitation, personName);
      const existing = this.model.workspaces.filter(({ id }) => id !== joined.workspace.id);
      this.set({
        workspaces: [...existing, joined.workspace],
        memberships: this.mergeMemberships({ [joined.workspace.id]: joined.membership }),
        activeWorkspaceId: joined.workspace.id,
        notice: `Joined ${joined.workspace.name}`,
      });
      await this.loadOptionalPrincipals(joined.workspace.id);
    });
  }

  /** Renames the Workspace on this Installation only; other Members keep their own name for it. */
  public async renameWorkspace(workspaceId: string, name: string): Promise<void> {
    await this.perform(async () => {
      const renamed = await this.bridge.renameWorkspace(workspaceId, name);
      this.set({
        workspaces: this.model.workspaces.map((workspace) => workspace.id === renamed.id ? renamed : workspace),
        notice: "Workspace renamed",
      });
    });
  }

  public async rename(workspaceId: string, name: string): Promise<void> {
    await this.perform(async () => {
      const self = this.model.memberships[workspaceId]?.members.find(({ identity, kind }) => kind === "person" && identity === this.model.installation?.identity);
      if (!self) throw { code: "invalid_input", retryable: false };
      await this.bridge.renameWorkspacePrincipal(workspaceId, self.principalId, name);
      this.set({ notice: "Name updated" });
      await this.loadOptionalPrincipals(workspaceId);
    });
  }

  public async addAgent(workspaceId: string, name: string): Promise<void> {
    await this.perform(async () => {
      await this.bridge.addWorkspaceAgent(workspaceId, name);
      this.set({ notice: "Agent added" });
      await this.refreshAfterAgentMutation(workspaceId);
    });
  }

  public async removeAgent(workspaceId: string, agentId: string): Promise<void> {
    await this.perform(async () => {
      await this.bridge.removeWorkspaceAgent(workspaceId, agentId);
      this.set({ notice: "Agent removed" });
      await this.refreshAfterAgentMutation(workspaceId);
    });
  }

  public async renameAgent(workspaceId: string, agentId: string, name: string): Promise<void> {
    await this.perform(async () => {
      await this.bridge.renameWorkspacePrincipal(workspaceId, agentId, name);
      this.set({ notice: "Agent name updated" });
      await this.loadOptionalPrincipals(workspaceId);
    });
  }

  private async refreshAfterAgentMutation(workspaceId: string): Promise<void> {
    try {
      const membership = await this.bridge.currentMembership(workspaceId);
      this.set({
        memberships: this.mergeMemberships({ [workspaceId]: membership }),
        membershipRefreshUnavailable: this.model.membershipRefreshUnavailable === workspaceId
          ? null : this.model.membershipRefreshUnavailable,
      });
    } catch {
      this.set({ membershipRefreshUnavailable: workspaceId });
    }
    await this.loadOptionalPrincipals(workspaceId);
  }

  private mergeMemberships(memberships: Readonly<Record<string, Membership>>): Record<string, Membership> {
    const merged = { ...this.model.memberships };
    for (const [id, membership] of Object.entries(memberships)) {
      const previous = merged[id];
      if (!previous || BigInt(membership.epoch) >= BigInt(previous.epoch)) merged[id] = membership;
    }
    return merged;
  }

  private receivePrincipals(snapshot: PrincipalsSnapshot): void {
    if (!this.model.workspaces.some(({ id }) => id === snapshot.workspaceId)) return;
    this.set({ principals: this.mergePrincipals({ [snapshot.workspaceId]: snapshot }) });
  }

  private mergePrincipals(snapshots: Readonly<Record<string, PrincipalsSnapshot>>): Record<string, PrincipalsSnapshot> {
    const merged = { ...this.model.principals };
    for (const [id, snapshot] of Object.entries(snapshots)) {
      const previous = merged[id];
      if (!previous || BigInt(snapshot.cursor) >= BigInt(previous.cursor)) merged[id] = snapshot;
    }
    return merged;
  }

  private async loadOptionalPrincipals(workspaceId: string): Promise<void> {
    try {
      this.receivePrincipals(await this.bridge.principalsSnapshot(workspaceId));
    } catch {
      // The Workspace is already durable; subscription can supply its optional presentation later.
    }
    this.watchPrincipals(workspaceId);
  }

  private watchPrincipals(workspaceId: string): void {
    if (this.disposed || this.watched.has(workspaceId) || this.principalRetries.has(workspaceId)) return;
    const attempt = ++this.nextWatch;
    this.watched.set(workspaceId, attempt);
    const cursor = this.model.principals[workspaceId]?.cursor ?? "0";
    const terminated = () => {
      if (this.disposed || this.watched.get(workspaceId) !== attempt) return;
      this.watched.delete(workspaceId);
      const delay = this.principalRetryDelays.get(workspaceId) ?? 1000;
      this.principalRetryDelays.set(workspaceId, Math.min(delay * 2, 30_000));
      this.principalRetries.set(workspaceId, setTimeout(() => {
        this.principalRetries.delete(workspaceId);
        this.watchPrincipals(workspaceId);
      }, delay));
    };
    void this.bridge.subscribePrincipals(workspaceId, cursor, (snapshot) => {
      if (this.disposed || this.watched.get(workspaceId) !== attempt) return;
      this.principalRetryDelays.delete(workspaceId);
      this.receivePrincipals(snapshot);
      // The Principal wake also covers membership transitions on a remote peer.
      void this.bridge.currentMembership(workspaceId).then((membership) => {
        if (this.disposed || !this.model.workspaces.some(({ id }) => id === workspaceId)) return;
        this.set({ memberships: this.mergeMemberships({ [workspaceId]: membership }) });
      }).catch(() => undefined);
    }, terminated).catch(terminated);
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
    if ("notice" in change) this.scheduleNoticeHide(change.notice ?? null);
    this.update(this.model);
  }

  /** A notice hides itself after the shared delay unless a later change replaced it (`TEAM.SYS.SHELL-R08`). */
  private scheduleNoticeHide(notice: string | null): void {
    this.cancelNoticeTimer();
    if (notice === null || this.disposed) return;
    this.noticeTimer = this.timers.setTimeout(() => {
      this.noticeTimer = null;
      if (this.model.notice === notice) this.set({ notice: null });
    }, STATUS_MESSAGE_HIDE_MS);
  }

  private cancelNoticeTimer(): void {
    if (this.noticeTimer === null) return;
    this.timers.clearTimeout(this.noticeTimer);
    this.noticeTimer = null;
  }
}

/**
 * Writes text that is still being produced. WebKit only allows a clipboard write
 * that starts during the user's click, so call this synchronously from the click
 * and let the write wait for the text.
 */
export function writeClipboardTextWhenReady(text: Promise<string>): Promise<void> {
  const blob = text.then((value) => new Blob([value], { type: "text/plain" }));
  return navigator.clipboard.write([new ClipboardItem({ "text/plain": blob })]);
}

const escape = (value: string): string => value
  .replaceAll("&", "&amp;")
  .replaceAll("<", "&lt;")
  .replaceAll(">", "&gt;")
  .replaceAll('"', "&quot;");

export type ShellRendererInput = Readonly<{
  packageSettings?: string;
  /** The rendered Package page, for the `package-page` route. */
  packagePage?: string;
}>;

export function renderShell(
  presentation: ShellPresentation,
  route: ShellRoute,
  input: ShellRendererInput = {},
): string {
  // One notification area at the top of the screen, in the space it always keeps (`TEAM.SYS.SHELL-R08`).
  const error = presentation.error
    ? renderStatusMessage(
      { message: presentation.error.message, failed: true },
      "data-workspace-error-dismiss",
      presentation.error.canRetry ? '<button id="retry" type="button">Retry</button>' : "",
    )
    : "";
  const refreshWarning = presentation.membershipRefreshUnavailable && routeWorkspaceId(route) === presentation.membershipRefreshUnavailable
    ? renderStatusMessage(
      { message: "Agent change succeeded, but members could not be refreshed. Check the roster before another change.", failed: true },
      "data-membership-refresh-dismiss",
      '<button id="retry-membership-refresh" type="button">Refresh members</button>',
    )
    : "";
  const status = presentation.loading
    ? { message: "Working…", failed: false }
    : presentation.notice === null ? null : { message: presentation.notice, failed: false };
  const feedback = error || refreshWarning
    ? `<div class="status-message-slot">${error}${refreshWarning}</div>`
    : renderStatusMessageSlot(status, "");

  if (route.kind === "workspace-chooser") {
    const choices = presentation.workspaceChoices.map(({ name }, index) =>
      `<li><button type="button" data-choose-workspace="${index}">${escape(name)}</button></li>`).join("");
    const workspaceList = choices || (presentation.loading ? '<li class="empty-state">Loading Workspaces…</li>' : "");
    const workspaceListSection = workspaceList
      ? `<section class="screen-section" aria-labelledby="workspace-list-heading"><h2 id="workspace-list-heading">Your Workspaces</h2><ul class="workspace-list">${workspaceList}</ul></section>`
      : "";
    return `<section class="shell-screen" aria-labelledby="chooser-heading"><header class="screen-heading"><p class="eyebrow">Workspaces</p><h1 id="chooser-heading">Create or join a Workspace</h1><p>Create a new Workspace, or join one with a Workspace Invitation.</p></header>${feedback}
      ${workspaceListSection}
      <section class="screen-section"><h2>Create a Workspace</h2><form id="create-workspace"><label for="workspace-name">Workspace name</label>
      <input id="workspace-name" required maxlength="64" autocapitalize="off" autocorrect="off" spellcheck="false" writingsuggestions="false"><label for="create-person-name">Your name in this Workspace</label><input id="create-person-name" required maxlength="256" autocomplete="name"><button ${presentation.loading ? "disabled" : ""}>Create</button></form></section>
      <section class="screen-section"><h2>Join a Workspace</h2><form id="join-workspace"><label for="invitation-value">Workspace Invitation</label>
      <textarea id="invitation-value" required maxlength="8192" autocapitalize="off" autocorrect="off" spellcheck="false" writingsuggestions="false"></textarea><label for="join-person-name">Your name in this Workspace</label><input id="join-person-name" required maxlength="256" autocomplete="name"><button ${presentation.loading ? "disabled" : ""}>Join</button></form></section></section>`;
  }

  if (route.kind === "package-page") {
    // The Package page keeps its own message space; a Workspace result shows above it only when present.
    return `${status === null && !error && !refreshWarning ? "" : feedback}${input.packagePage ?? '<section class="shell-screen"><p class="empty-state">Package information is loading.</p></section>'}`;
  }

  const workspace = presentation.workspace;
  if (!workspace) return `${feedback}<section class="shell-screen"><h1>Workspace unavailable</h1><p>Choose another Workspace or retry.</p></section>`;

  const memberRows = workspace.members.map((member, index) => `<li class="member-row"><span><strong>${escape(member.name)}</strong>${member.indicator ? `<small class="member-role">${escape(member.indicator)}</small>` : ""}<small><details><summary>View identity</summary>${escape(member.detail)}</details></small></span><span class="member-actions"><span class="member-role">${member.role}</span>${member.canRemoveAgent ? `<button type="button" data-remove-agent="${index}" aria-label="Remove agent ${escape(member.name)}" ${presentation.loading ? "disabled" : ""}>Remove agent</button>` : ""}${member.canRenameAgent ? `<form data-rename-agent="${index}"><label for="agent-name-${index}">Agent name</label><input id="agent-name-${index}" required maxlength="256" value="${escape(member.name)}"><button ${presentation.loading ? "disabled" : ""}>Save agent name</button></form>` : ""}</span></li>`).join("");
  const members = memberRows || (workspace.membershipLoaded
    ? '<li class="empty-state">No Members are listed.</li>'
    : '<li class="empty-state">Membership is loading.</li>');
  const disabled = presentation.loading ? " disabled" : "";
  const memberCount = workspace.memberCount === 1 ? "1 Member." : `${workspace.memberCount} Members.`;
  return `<section class="shell-screen workspace-page" aria-labelledby="workspace-page-heading"><header class="package-page-header workspace-page-header"><p class="package-page-kind">Workspace</p><h1 id="workspace-page-heading" class="package-page-title">${escape(workspace.name)}</h1><p class="package-page-subtitle">${memberCount}</p></header>${feedback}
    ${settingsGroup("workspace-name", "Workspace name", "The name this Workspace has on this Installation. Other Members keep their own name for it.", `<form id="rename-workspace"><label for="rename-workspace-name">Workspace name</label><div class="inline-field"><input id="rename-workspace-name" required maxlength="64" autocomplete="off" value="${escape(workspace.name)}"><button${disabled}>Rename</button></div></form>`)}
    ${settingsGroup("your-name", "Your name", "The name other Members see for you in this Workspace.", `<form id="rename-person"><label for="rename-person-name">Name in this Workspace</label><div class="inline-field"><input id="rename-person-name" required maxlength="256" autocomplete="name" value="${escape(workspace.ownPersonName)}"><button${disabled}>Save name</button></div></form>`)}
    ${settingsGroup("members", "Members", "People and Agents in this Workspace. You are the Steward of each Agent you add.", `<ol class="members">${members}</ol><form id="add-agent"><label for="agent-name">Add an Agent</label><div class="inline-field"><input id="agent-name" required maxlength="256" autocomplete="off" placeholder="Agent name"><button${disabled}>Add agent</button></div></form><div><button type="button" id="refresh-membership" class="secondary"${disabled}>Refresh members</button></div>`)}
    ${settingsGroup("invitations", "Workspace Invitations", "Each Workspace Invitation admits one Person, once.", `<div><button type="button" id="copy-invitation"${disabled}>Copy Workspace Invitation</button></div><p class="sensitive">Workspace Invitations are sensitive bearer admission material. Share them privately.</p>`)}
    ${settingsGroup("package-instances", "Package Instances", "Packages in this Workspace. Hide one from navigation, or open it for details and controls.", `${input.packageSettings ?? '<p class="empty-state">Package information is loading.</p>'}<div><button type="button" class="secondary package-add" data-package-install-action="open">Add Personal Package</button></div>`)}</section>`;
}

/** One Workspace settings group, in the Package page's two-column group structure. */
function settingsGroup(id: string, title: string, intro: string, body: string): string {
  return `<section class="package-page-group" aria-labelledby="${id}-heading"><div class="package-page-group-intro"><h2 id="${id}-heading">${escape(title)}</h2><p>${escape(intro)}</p></div><div class="package-page-group-body">${body}</div></section>`;
}

function routeWorkspaceId(route: ShellRoute): string | null {
  return route.kind === "workspace-chooser"
    ? null
    : route.kind === "package-instance" ? route.instance.workspaceId : route.workspaceId;
}

