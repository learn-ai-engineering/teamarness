import type { TeamarnessBridge } from "./bridge";
import { isBridgeError } from "./contracts";
import type { Installation, Membership, PrincipalsSnapshot, ShellErrorCode, Workspace } from "./contracts";
import { buildShellPresentation, type ShellPresentation } from "./presentation";
import {
  browserStatusMessageTimers,
  renderStatusMessage,
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
  /** The settings group whose action produced `error` or `notice`; null shows it at the top of the screen. */
  feedbackSection: FeedbackSection | null;
}>;

/** A group of the Workspace settings or chooser screen that shows its own action results (`TEAM.SYS.SHELL-R08`). */
export type FeedbackSection = "workspace-name" | "your-name" | "members" | "invitations" | "delete" | "create" | "join";

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
  feedbackSection: null,
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
        // The new Workspace's page has no create group, so the result shows at its top.
        feedbackSection: null,
      });
      await this.loadOptionalPrincipals(created.workspace.id);
    }, "create");
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
    }, "invitations");
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
    }, "members");
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
        feedbackSection: null,
      });
      await this.loadOptionalPrincipals(joined.workspace.id);
    }, "join");
  }

  /** Deletes the Workspace and all of its data on this Installation (ADR 0050). */
  public async deleteWorkspace(workspaceId: string): Promise<void> {
    await this.perform(async () => {
      const name = this.model.workspaces.find(({ id }) => id === workspaceId)?.name ?? "the Workspace";
      await this.bridge.deleteWorkspace(workspaceId);
      this.watched.delete(workspaceId);
      const retry = this.principalRetries.get(workspaceId);
      if (retry !== undefined) clearTimeout(retry);
      this.principalRetries.delete(workspaceId);
      const workspaces = this.model.workspaces.filter(({ id }) => id !== workspaceId);
      const memberships = Object.fromEntries(Object.entries(this.model.memberships).filter(([id]) => id !== workspaceId));
      const principals = Object.fromEntries(Object.entries(this.model.principals).filter(([id]) => id !== workspaceId));
      this.set({
        workspaces,
        memberships,
        principals,
        activeWorkspaceId: this.model.activeWorkspaceId === workspaceId ? workspaces[0]?.id ?? null : this.model.activeWorkspaceId,
        notice: `Deleted ${name}`,
        // The deleted Workspace's page is gone, so the result shows at the top of the next screen.
        feedbackSection: null,
      });
    }, "delete");
  }

  /** Renames the Workspace on this Installation only; other Members keep their own name for it. */
  public async renameWorkspace(workspaceId: string, name: string): Promise<void> {
    await this.perform(async () => {
      const renamed = await this.bridge.renameWorkspace(workspaceId, name);
      this.set({
        workspaces: this.model.workspaces.map((workspace) => workspace.id === renamed.id ? renamed : workspace),
        notice: "Workspace renamed",
      });
    }, "workspace-name");
  }

  public async rename(workspaceId: string, name: string): Promise<void> {
    await this.perform(async () => {
      const self = this.model.memberships[workspaceId]?.members.find(({ identity, kind }) => kind === "person" && identity === this.model.installation?.identity);
      if (!self) throw { code: "invalid_input", retryable: false };
      await this.bridge.renameWorkspacePrincipal(workspaceId, self.principalId, name);
      this.set({ notice: "Name updated" });
      await this.loadOptionalPrincipals(workspaceId);
    }, "your-name");
  }

  public async addAgent(workspaceId: string, name: string): Promise<void> {
    await this.perform(async () => {
      await this.bridge.addWorkspaceAgent(workspaceId, name);
      this.set({ notice: "Agent added" });
      await this.refreshAfterAgentMutation(workspaceId);
    }, "members");
  }

  public async removeAgent(workspaceId: string, agentId: string): Promise<void> {
    await this.perform(async () => {
      await this.bridge.removeWorkspaceAgent(workspaceId, agentId);
      this.set({ notice: "Agent removed" });
      await this.refreshAfterAgentMutation(workspaceId);
    }, "members");
  }

  public async renameAgent(workspaceId: string, agentId: string, name: string): Promise<void> {
    await this.perform(async () => {
      await this.bridge.renameWorkspacePrincipal(workspaceId, agentId, name);
      this.set({ notice: "Agent name updated" });
      await this.loadOptionalPrincipals(workspaceId);
    }, "members");
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

  /** Runs one action; its result shows in `section`, or at the top of the screen when none is given. */
  private async perform(operation: () => Promise<void>, section: FeedbackSection | null = null): Promise<void> {
    this.set({ loading: true, error: null, notice: null, feedbackSection: section });
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
  /** The Agent whose name is being edited in place, by Principal ID. */
  editingAgentId?: string | null;
  /** The Add Personal Package flow is open, so its + button is hidden until the flow closes. */
  packageInstallOpen?: boolean;
}>;

const ICON_ATTRIBUTES = 'viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="square" stroke-linejoin="miter" aria-hidden="true" focusable="false"';
const REMOVE_ICON = `<svg ${ICON_ATTRIBUTES}><path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3"/></svg>`;
const EDIT_ICON = `<svg ${ICON_ATTRIBUTES}><path d="M4 20h4L19 9l-4-4L4 16v4zM13 7l4 4"/></svg>`;

export function renderShell(
  presentation: ShellPresentation,
  route: ShellRoute,
  input: ShellRendererInput = {},
): string {
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
    ? null
    : presentation.notice === null ? null : renderStatusMessage({ message: presentation.notice, failed: false }, "");
  const result = error || status || "";
  /**
   * The result of the last action, shown in the group that started it, or at the top of the
   * screen for a load or a result whose group is gone (`TEAM.SYS.SHELL-R08`).
   */
  const feedbackFor = (section: FeedbackSection | null): string => {
    const own = presentation.feedbackSection === section ? result : "";
    const extra = section === "members" ? refreshWarning : "";
    return own || extra ? `<div class="status-message-slot">${own}${extra}</div>` : "";
  };
  const topFeedback = (sections: readonly FeedbackSection[]): string =>
    presentation.feedbackSection === null || !sections.includes(presentation.feedbackSection)
      ? (result ? `<div class="status-message-slot">${result}</div>` : "")
      : "";

  if (route.kind === "workspace-chooser") {
    const choices = presentation.workspaceChoices.map(({ name }, index) =>
      `<li><button type="button" data-choose-workspace="${index}">${escape(name)}</button></li>`).join("");
    const workspaceList = choices || (presentation.loading ? '<li class="empty-state">Loading Workspaces…</li>' : "");
    const workspaceListSection = workspaceList
      ? `<section class="screen-section" aria-labelledby="workspace-list-heading"><h2 id="workspace-list-heading">Your Workspaces</h2><ul class="workspace-list">${workspaceList}</ul></section>`
      : "";
    return `<section class="shell-screen" aria-labelledby="chooser-heading"><header class="screen-heading"><p class="eyebrow">Workspaces</p><h1 id="chooser-heading">Create or join a Workspace</h1><p>Create a new Workspace, or join one with a Workspace Invitation.</p></header>${topFeedback(["create", "join"])}
      ${workspaceListSection}
      <section class="screen-section"><h2>Create a Workspace</h2>${feedbackFor("create")}<form id="create-workspace"><label for="workspace-name">Workspace name</label>
      <input id="workspace-name" required maxlength="64" autocapitalize="off" autocorrect="off" spellcheck="false" writingsuggestions="false"><label for="create-person-name">Your name in this Workspace</label><input id="create-person-name" required maxlength="256" autocomplete="name"><button ${presentation.loading ? "disabled" : ""}>Create</button></form></section>
      <section class="screen-section"><h2>Join a Workspace</h2>${feedbackFor("join")}<form id="join-workspace"><label for="invitation-value">Workspace Invitation</label>
      <textarea id="invitation-value" required maxlength="8192" autocapitalize="off" autocorrect="off" spellcheck="false" writingsuggestions="false"></textarea><label for="join-person-name">Your name in this Workspace</label><input id="join-person-name" required maxlength="256" autocomplete="name"><button ${presentation.loading ? "disabled" : ""}>Join</button></form></section></section>`;
  }

  if (route.kind === "package-page") {
    // The Package page keeps its own message space; a Workspace result shows above it only when present.
    return `${error || refreshWarning || status ? `<div class="status-message-slot">${error}${refreshWarning}${status ?? ""}</div>` : ""}${input.packagePage ?? '<section class="shell-screen"><p class="empty-state">Package information is loading.</p></section>'}`;
  }

  const workspace = presentation.workspace;
  if (!workspace) return `${result || refreshWarning ? `<div class="status-message-slot">${result}${refreshWarning}</div>` : ""}<section class="shell-screen"><h1>Workspace unavailable</h1><p>Choose another Workspace or retry.</p></section>`;

  const disabled = presentation.loading ? " disabled" : "";
  const memberRows = workspace.members.map((member, index) => {
    const editing = member.canRenameAgent && member.agentId !== null && member.agentId === input.editingAgentId;
    const name = editing
      ? `<form class="member-name-form" data-rename-agent="${index}"><input class="member-name-input" required maxlength="256" autocomplete="off" spellcheck="false" aria-label="Agent name" value="${escape(member.name)}"${disabled}></form>`
      : member.canRenameAgent
        ? `<button type="button" class="member-name-edit" data-edit-agent="${index}" aria-label="Rename agent ${escape(member.name)}" title="Rename"${disabled}><strong>${escape(member.name)}</strong>${EDIT_ICON}</button>`
        : `<strong>${escape(member.name)}</strong>`;
    const details = [member.indicator, member.duplicateName ? "Duplicate name" : null].filter((value) => value !== null);
    const remove = member.canRemoveAgent
      ? `<button type="button" class="icon-button member-remove" data-remove-agent="${index}" aria-label="Remove agent ${escape(member.name)}" title="Remove agent"${disabled}>${REMOVE_ICON}</button>`
      : "";
    return `<li class="member-row"><span class="member-identity">${name}${details.map((detail) => `<small>${escape(detail)}</small>`).join("")}</span><span class="member-actions"><span class="member-role">${member.role}</span>${remove}</span></li>`;
  }).join("");
  const members = memberRows || (workspace.membershipLoaded
    ? '<li class="empty-state">No Members are listed.</li>'
    : '<li class="empty-state">Membership is loading.</li>');
  const memberCount = workspace.memberCount === 1 ? "1 Member." : `${workspace.memberCount} Members.`;
  const sections: readonly FeedbackSection[] = ["workspace-name", "your-name", "members", "invitations", ...(workspace.canDelete ? ["delete" as const] : [])];
  return `<section class="shell-screen workspace-page" aria-labelledby="workspace-page-heading"><header class="package-page-header workspace-page-header"><p class="package-page-kind">Workspace</p><h1 id="workspace-page-heading" class="package-page-title">${escape(workspace.name)}</h1><p class="package-page-subtitle">${memberCount}</p></header>${topFeedback(sections)}
    ${settingsGroup("workspace-name", "Workspace name", "The name this Workspace has on this Installation. Other Members keep their own name for it.", `${feedbackFor("workspace-name")}<form id="rename-workspace"><label for="rename-workspace-name">Workspace name</label><div class="inline-field"><input id="rename-workspace-name" required maxlength="64" autocomplete="off" value="${escape(workspace.name)}"><button${disabled}>Rename</button></div></form>`)}
    ${settingsGroup("your-name", "Your name", "The name other Members see for you in this Workspace.", `${feedbackFor("your-name")}<form id="rename-person"><label for="rename-person-name">Name in this Workspace</label><div class="inline-field"><input id="rename-person-name" required maxlength="256" autocomplete="name" value="${escape(workspace.ownPersonName)}"><button${disabled}>Save name</button></div></form>`)}
    ${settingsGroup("members", "Members", "People and Agents in this Workspace. You are the Steward of each Agent you add, and can rename it by selecting its name.", `${feedbackFor("members")}<ol class="members">${members}</ol><form id="add-agent"><label for="agent-name">Add an Agent</label><div class="inline-field"><input id="agent-name" required maxlength="256" autocomplete="off" placeholder="Agent name"><button${disabled}>Add agent</button></div></form><div><button type="button" id="refresh-membership" class="secondary"${disabled}>Refresh members</button></div>`)}
    ${settingsGroup("invitations", "Workspace Invitations", "Each Workspace Invitation admits one Person, once.", `${feedbackFor("invitations")}<div><button type="button" id="copy-invitation"${disabled}>Copy Workspace Invitation</button></div><p class="sensitive">Workspace Invitations are sensitive bearer admission material. Share them privately.</p>`)}
    ${settingsGroup("package-instances", "Package Instances", "Packages in this Workspace. Hide one from navigation, or open it for details and controls.", input.packageSettings ?? '<p class="empty-state">Package information is loading.</p>', input.packageSettings === undefined || input.packageInstallOpen === true ? "" : '<button type="button" class="package-add" data-package-install-action="open" aria-label="Add Personal Package" title="Add Personal Package">+</button>')}
    ${workspace.canDelete ? renderDeleteWorkspace(workspace.name, presentation.loading, feedbackFor("delete")) : ""}</section>`;
}

/**
 * Deleting a Workspace takes typing its exact name: the confirm button stays disabled until the
 * field matches, and the Shell checks it again on submit (ADR 0050).
 */
function renderDeleteWorkspace(name: string, loading: boolean, feedback: string): string {
  return `<section class="package-page-group workspace-delete" aria-labelledby="delete-workspace-heading"><div class="package-page-group-intro"><h2 id="delete-workspace-heading">Delete Workspace</h2><p>You created this Workspace and no one else is a Member.</p></div><div class="package-page-group-body">${feedback}<p>Deleting erases this Workspace and all of its data on this Installation, including its Agents and every Package's data. Files in folders mapped by Documents stay on disk. This cannot be undone.</p><form id="delete-workspace" data-workspace-name="${escape(name)}"><label for="delete-workspace-confirm">Type <strong>${escape(name)}</strong> to confirm</label><div class="inline-field"><input id="delete-workspace-confirm" autocomplete="off" spellcheck="false"><button type="submit" class="danger" disabled${loading ? ' aria-busy="true"' : ""}>Delete Workspace</button></div></form></div></section>`;
}

/** One Workspace settings group, in the Package page's two-column group structure, with an optional control beside its title. */
function settingsGroup(id: string, title: string, intro: string, body: string, headingAction = ""): string {
  const heading = `<h2 id="${id}-heading">${escape(title)}</h2>`;
  return `<section class="package-page-group" aria-labelledby="${id}-heading"><div class="package-page-group-intro">${headingAction ? `<div class="section-heading">${heading}${headingAction}</div>` : heading}<p>${escape(intro)}</p></div><div class="package-page-group-body">${body}</div></section>`;
}

function routeWorkspaceId(route: ShellRoute): string | null {
  return route.kind === "workspace-chooser"
    ? null
    : route.kind === "package-instance" ? route.instance.workspaceId : route.workspaceId;
}

