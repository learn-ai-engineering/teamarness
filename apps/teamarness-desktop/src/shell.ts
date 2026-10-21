import type { PackageHostSnapshot, PackageInstanceKey } from "./package-contracts";
import type { PackageLifecycleFailure, PackageLifecycleResult } from "./package-shell";
import {
  buildPackageNavigationPresentation,
  buildShellPresentation,
  type PackageInstancePresentation,
  type PackageNavigationPresentation,
  type ShellPresentation,
} from "./presentation";
import type { InstallationThemeController, ThemePreference } from "./theme";
import { THEME_OPTIONS } from "./theme-icons";
import { renderShell, type ShellModel } from "./view";

export type ShellRoute =
  | Readonly<{ kind: "workspace-chooser" }>
  | Readonly<{ kind: "workspace-settings"; workspaceId: string }>
  | Readonly<{ kind: "package-instance"; instance: PackageInstanceKey }>;

export type ShellFrame = Readonly<{
  workspaceRail: HTMLElement;
  contextualNavigation: HTMLElement;
  shellScreen: HTMLElement;
  packageMount: HTMLElement;
}>;

type PackageRouter = Readonly<{
  refresh(): Promise<PackageHostSnapshot>;
  select(instance: PackageInstanceKey): Promise<void>;
  hide(): Promise<void>;
  recoverBackend(instance: PackageInstanceKey): Promise<PackageLifecycleResult>;
  disposeBackend(instance: PackageInstanceKey): Promise<PackageLifecycleResult>;
}>;

type PackageLifecycleAction = "recover" | "dispose";

type PackageLifecycleFeedback = Readonly<{
  action: PackageLifecycleAction;
  instance: PackageInstanceKey;
  failure: PackageLifecycleFailure;
}>;

type WorkspaceActions = Readonly<{
  selectWorkspace(workspaceId: string): void;
  createWorkspace(name: string, personName: string): Promise<void>;
  joinWorkspace(invitation: string, personName: string): Promise<void>;
  rename(workspaceId: string, name: string): Promise<void>;
  addAgent(workspaceId: string, name: string): Promise<void>;
  removeAgent(workspaceId: string, agentId: string): Promise<void>;
  renameAgent(workspaceId: string, agentId: string, name: string): Promise<void>;
  copyInvitation(workspaceId: string): Promise<void>;
  refreshMembership(workspaceId: string): Promise<void>;
  retry(): Promise<void>;
}>;

type ShellOptions = Readonly<{
  packageShell: PackageRouter;
  theme: InstallationThemeController;
  workspaceActions: WorkspaceActions;
  onRouteChange?: (route: ShellRoute) => void;
}>;

export class Shell {
  private selectedRoute: ShellRoute = { kind: "workspace-chooser" };
  private workspaceModel: ShellModel | null = null;
  private workspacePresentation: ShellPresentation | null = null;
  private packageSnapshot: PackageHostSnapshot | null = null;
  private packageSnapshotFailed = false;
  private packageLifecycleFeedback: PackageLifecycleFeedback | null = null;
  private observedActiveWorkspaceId: string | null = null;
  private packageActionTargets: readonly PackageInstanceKey[] = [];
  private packagePresentation: PackageNavigationPresentation = { teamPackages: [], privatePackages: [] };
  private presentationRevision = 0;

  public constructor(
    private readonly shellFrame: ShellFrame,
    private readonly options: ShellOptions,
  ) {
    this.shellFrame.packageMount.hidden = true;
  }

  public frame(): ShellFrame { return this.shellFrame; }

  public route(): ShellRoute { return this.selectedRoute; }

  public async updateWorkspace(model: ShellModel): Promise<void> {
    this.workspaceModel = model;
    if (model.workspaces.length === 0) {
      if (this.selectedRoute.kind !== "workspace-chooser") {
        this.deactivatePackage();
        this.setRoute({ kind: "workspace-chooser" });
      }
      this.observedActiveWorkspaceId = null;
    } else if (model.activeWorkspaceId !== null && model.activeWorkspaceId !== this.observedActiveWorkspaceId) {
      this.observedActiveWorkspaceId = model.activeWorkspaceId;
      this.deactivatePackage();
      this.setRoute({ kind: "workspace-settings", workspaceId: model.activeWorkspaceId });
    }
    if (await this.preparePresentation()) this.renderAll();
  }

  public updatePackages(snapshot: PackageHostSnapshot): void {
    this.packageSnapshot = snapshot;
    this.packageSnapshotFailed = false;
    if (this.packageLifecycleFeedback?.failure === "refresh") {
      this.packageLifecycleFeedback = null;
    } else if (this.packageLifecycleFeedback?.failure === "operation-and-refresh") {
      this.packageLifecycleFeedback = { ...this.packageLifecycleFeedback, failure: "operation" };
    }
    this.renderPackageConsumers();
  }

  public updatePackageSnapshotFailure(): void {
    this.packageSnapshotFailed = true;
    this.renderPackageConsumers();
  }

  public async navigate(route: ShellRoute): Promise<void> {
    this.setRoute(route);
    if (route.kind === "package-instance") {
      this.shellFrame.shellScreen.hidden = true;
      this.shellFrame.packageMount.hidden = false;
      this.renderContextualNavigation();
      await this.options.packageShell.select(route.instance);
      return;
    }

    await this.options.packageShell.hide();
    this.shellFrame.packageMount.hidden = true;
    this.shellFrame.shellScreen.hidden = false;
    if (await this.preparePresentation()) this.renderAll();
  }

  private setRoute(route: ShellRoute): void {
    this.selectedRoute = route;
    this.options.onRouteChange?.(route);
  }

  private async preparePresentation(): Promise<boolean> {
    const model = this.workspaceModel;
    if (!model) return false;
    const revision = ++this.presentationRevision;
    const presentation = await buildShellPresentation(model, routeWorkspaceId(this.selectedRoute));
    if (revision !== this.presentationRevision) return false;
    this.workspacePresentation = presentation;
    return true;
  }

  private renderAll(): void {
    this.renderWorkspaceRail();
    this.renderContextualNavigation();
    this.renderSelectedScreen();
  }

  private renderWorkspaceRail(): void {
    const model = this.workspaceModel;
    const presentation = this.workspacePresentation;
    if (!model || !presentation) return;
    const workspaces = presentation.workspaceChoices.map((workspace, index) => {
      const current = index === presentation.activeWorkspaceIndex ? ' aria-current="page"' : "";
      return `<li><button type="button" data-workspace-index="${index}"${current} aria-label="Switch to ${escape(workspace.name)}">${escape(workspaceMark(workspace.name))}</button></li>`;
    }).join("");
    const preference = this.options.theme.getPreference();
    const addCurrent = this.selectedRoute.kind === "workspace-chooser" ? ' aria-current="page"' : "";
    this.shellFrame.workspaceRail.innerHTML = `<div class="workspace-rail__brand" aria-label="Teamarness">T</div>
      <ul class="workspace-switcher">${workspaces}</ul>
      <button class="workspace-add" id="choose-workspace" type="button" aria-label="Add or join a Workspace"${addCurrent}>+</button>
      <div class="theme-switcher" role="group" aria-label="Theme">${THEME_OPTIONS.map((option) => themeButton(option, preference)).join("")}</div>`;
    if (!hasQueries(this.shellFrame.workspaceRail)) return;
    this.shellFrame.workspaceRail.querySelectorAll<HTMLButtonElement>("[data-workspace-index]").forEach((button) => {
      button.addEventListener("click", () => {
        const workspace = model.workspaces[Number(button.dataset.workspaceIndex)];
        if (workspace) this.options.workspaceActions.selectWorkspace(workspace.id);
      });
    });
    this.shellFrame.workspaceRail.querySelector<HTMLButtonElement>("#choose-workspace")?.addEventListener("click", () => {
      void this.navigate({ kind: "workspace-chooser" });
    });
    this.shellFrame.workspaceRail.querySelectorAll<HTMLButtonElement>("[data-theme-preference]").forEach((button) => {
      button.addEventListener("click", () => {
        const value = button.dataset.themePreference;
        if (value === undefined || !isThemePreference(value)) return;
        this.options.theme.selectPreference(value);
        this.renderWorkspaceRail();
        // Re-rendering replaces the buttons, so return keyboard focus to the chosen one.
        this.shellFrame.workspaceRail.querySelector<HTMLButtonElement>(`[data-theme-preference="${value}"]`)?.focus();
      });
    });
  }

  private renderContextualNavigation(): void {
    const model = this.workspaceModel;
    const presentation = this.workspacePresentation;
    if (!model || !presentation) return;
    const activeIndex = presentation.activeWorkspaceIndex;
    const active = activeIndex === null ? undefined : model.workspaces[activeIndex];
    const activePresentation = activeIndex === null ? undefined : presentation.workspaceChoices[activeIndex];
    if (!active || !activePresentation) {
      this.packageActionTargets = [];
      this.shellFrame.contextualNavigation.innerHTML = '<header class="workspace-context"><p>Workspaces</p><h1>Add a Workspace</h1></header>';
      return;
    }

    this.packageActionTargets = this.packageSnapshot?.instances
      .filter(({ key }) => key.workspaceId === active.id)
      .map(({ key }) => key) ?? [];
    this.packagePresentation = buildPackageNavigationPresentation(this.packageSnapshot, active.id);
    const team = this.packagePresentation.teamPackages
      .map((row) => packageButton(row, this.selectedRoute, this.packageActionTargets)).join("");
    const privatePackages = this.packagePresentation.privatePackages
      .map((row) => packageButton(row, this.selectedRoute, this.packageActionTargets)).join("");

    const settingsCurrent = this.selectedRoute.kind === "workspace-settings" ? ' aria-current="page"' : "";
    this.shellFrame.contextualNavigation.innerHTML = `<header class="workspace-context"><div><p>Workspace</p><h1>${escape(activePresentation.name)}</h1></div>
      <button type="button" class="workspace-settings-link" data-screen="workspace-settings" aria-label="Workspace settings" title="Workspace settings"${settingsCurrent}>${SETTINGS_ICON}</button></header>
      <nav class="contextual-navigation__body" aria-label="Active Workspace">
      ${renderPackageSnapshotFailure(this.packageSnapshotFailed, this.packageSnapshot !== null)}
      <section class="package-group" aria-labelledby="team-packages-heading"><h2 id="team-packages-heading">Team Packages</h2><ul>${team || "<li>None available</li>"}</ul></section>
      <section class="package-group" aria-labelledby="private-packages-heading"><h2 id="private-packages-heading">Personal Packages</h2><ul>${privatePackages || "<li>None available</li>"}</ul></section></nav>`;

    if (!hasQueries(this.shellFrame.contextualNavigation)) return;
    this.shellFrame.contextualNavigation.querySelectorAll<HTMLButtonElement>("[data-screen]").forEach((button) => {
      button.addEventListener("click", () => {
        if (button.dataset.screen === "workspace-settings") {
          void this.navigate({ kind: "workspace-settings", workspaceId: active.id });
        }
      });
    });
    this.shellFrame.contextualNavigation.querySelectorAll<HTMLButtonElement>("[data-package-index]").forEach((button) => {
      button.addEventListener("click", () => {
        const instance = this.packageActionTargets[Number(button.dataset.packageIndex)];
        if (instance) void this.navigate({ kind: "package-instance", instance });
      });
    });
    this.bindPackageSnapshotRetry(this.shellFrame.contextualNavigation);
  }

  private deactivatePackage(): void {
    if (this.selectedRoute.kind === "package-instance") {
      void this.options.packageShell.hide().catch(() => undefined);
    }
    this.shellFrame.packageMount.hidden = true;
    this.shellFrame.shellScreen.hidden = false;
  }

  private renderSelectedScreen(): void {
    if (!this.workspacePresentation || this.selectedRoute.kind === "package-instance") return;
    const packageSettings = this.selectedRoute.kind === "workspace-settings"
      ? `${renderPackageSnapshotFailure(this.packageSnapshotFailed, this.packageSnapshot !== null)}${renderPackageInstanceSettings(
        this.packagePresentation,
        this.packageActionTargets,
        this.packageLifecycleFeedback,
      )}`
      : undefined;
    this.shellFrame.shellScreen.innerHTML = renderShell(
      this.workspacePresentation,
      this.selectedRoute,
      { packageSettings },
    );
    this.bindSelectedScreen();
  }

  private bindSelectedScreen(): void {
    const model = this.workspaceModel;
    if (!model || !hasQueries(this.shellFrame.shellScreen)) return;
    this.shellFrame.shellScreen.querySelector<HTMLFormElement>("#create-workspace")?.addEventListener("submit", (event) => {
      event.preventDefault();
      const input = this.shellFrame.shellScreen.querySelector<HTMLInputElement>("#workspace-name");
      const person = this.shellFrame.shellScreen.querySelector<HTMLInputElement>("#create-person-name");
      if (input && person) void this.options.workspaceActions.createWorkspace(input.value, person.value);
    });
    this.shellFrame.shellScreen.querySelector<HTMLFormElement>("#join-workspace")?.addEventListener("submit", (event) => {
      event.preventDefault();
      const input = this.shellFrame.shellScreen.querySelector<HTMLTextAreaElement>("#invitation-value");
      const person = this.shellFrame.shellScreen.querySelector<HTMLInputElement>("#join-person-name");
      if (input && person) void this.options.workspaceActions.joinWorkspace(input.value, person.value);
    });
    this.shellFrame.shellScreen.querySelectorAll<HTMLButtonElement>("[data-choose-workspace]").forEach((button) => {
      button.addEventListener("click", () => {
        const workspace = model.workspaces[Number(button.dataset.chooseWorkspace)];
        if (workspace) this.options.workspaceActions.selectWorkspace(workspace.id);
      });
    });
    const workspaceId = routeWorkspaceId(this.selectedRoute);
    if (workspaceId) {
      this.shellFrame.shellScreen.querySelector<HTMLFormElement>("#rename-person")?.addEventListener("submit", (event) => {
        event.preventDefault();
        const name = this.shellFrame.shellScreen.querySelector<HTMLInputElement>("#rename-person-name");
        if (name) void this.options.workspaceActions.rename(workspaceId, name.value);
      });
      this.shellFrame.shellScreen.querySelector<HTMLFormElement>("#add-agent")?.addEventListener("submit", (event) => {
        event.preventDefault();
        const name = this.shellFrame.shellScreen.querySelector<HTMLInputElement>("#agent-name");
        if (name) void this.options.workspaceActions.addAgent(workspaceId, name.value);
      });
      this.shellFrame.shellScreen.querySelectorAll<HTMLButtonElement>("[data-remove-agent]").forEach((button) => {
        button.addEventListener("click", () => {
          const member = this.workspacePresentation?.workspace?.members[Number(button.dataset.removeAgent)];
          if (member?.canRemoveAgent && member.agentId) void this.options.workspaceActions.removeAgent(workspaceId, member.agentId);
        });
      });
      this.shellFrame.shellScreen.querySelectorAll<HTMLFormElement>("[data-rename-agent]").forEach((form) => {
        form.addEventListener("submit", (event) => {
          event.preventDefault();
          const member = this.workspacePresentation?.workspace?.members[Number(form.dataset.renameAgent)];
          const name = form.querySelector<HTMLInputElement>("input");
          if (member?.canRenameAgent && member.agentId && name) void this.options.workspaceActions.renameAgent(workspaceId, member.agentId, name.value);
        });
      });
      this.shellFrame.shellScreen.querySelector<HTMLButtonElement>("#copy-invitation")?.addEventListener("click", () => {
        void this.options.workspaceActions.copyInvitation(workspaceId);
      });
      this.shellFrame.shellScreen.querySelector<HTMLButtonElement>("#refresh-membership")?.addEventListener("click", () => {
        void this.options.workspaceActions.refreshMembership(workspaceId);
      });
    }
    this.shellFrame.shellScreen.querySelector<HTMLButtonElement>("#retry")?.addEventListener("click", () => {
      void this.options.workspaceActions.retry();
    });
    this.bindPackageSnapshotRetry(this.shellFrame.shellScreen);
    this.shellFrame.shellScreen.querySelectorAll<HTMLButtonElement>("[data-package-action]").forEach((button) => {
      button.addEventListener("click", () => {
        const index = Number(button.dataset.packageActionIndex);
        const instance = this.packageActionTargets[index];
        const row = packageRows(this.packagePresentation).find(({ actionIndex }) => actionIndex === index);
        const action = button.dataset.packageAction;
        if (!instance || !row || !lifecycleActionAllowed(row, action)) return;
        void this.runPackageLifecycle(action, instance);
      });
    });
    this.shellFrame.shellScreen.querySelectorAll<HTMLButtonElement>("[data-package-failure-action]").forEach((button) => {
      button.addEventListener("click", () => {
        const feedback = this.packageLifecycleFeedback;
        const action = button.dataset.packageFailureAction;
        if (!feedback || !isLifecycleAction(action) || action !== feedback.action) return;
        void this.runPackageLifecycle(action, feedback.instance);
      });
    });
  }

  private renderPackageConsumers(): void {
    this.renderContextualNavigation();
    if (this.selectedRoute.kind === "workspace-settings") this.renderSelectedScreen();
  }

  private bindPackageSnapshotRetry(region: HTMLElement): void {
    region.querySelector<HTMLButtonElement>("[data-package-snapshot-retry]")?.addEventListener("click", () => {
      void this.retryPackageSnapshot();
    });
  }

  private async retryPackageSnapshot(): Promise<void> {
    try {
      this.updatePackages(await this.options.packageShell.refresh());
    } catch {
      this.updatePackageSnapshotFailure();
    }
  }

  private async runPackageLifecycle(
    action: PackageLifecycleAction,
    instance: PackageInstanceKey,
  ): Promise<void> {
    const result = action === "recover"
      ? await this.options.packageShell.recoverBackend(instance)
      : await this.options.packageShell.disposeBackend(instance);
    if (result.snapshot) {
      this.packageSnapshot = result.snapshot;
      this.packageSnapshotFailed = false;
    }
    if (result.failure === "refresh" || result.failure === "operation-and-refresh") {
      this.packageSnapshotFailed = true;
    }
    this.packageLifecycleFeedback = result.failure
      ? { action, instance, failure: result.failure }
      : null;
    this.renderPackageConsumers();
  }
}

export function shellFrameFromDocument(root: ParentNode = document): ShellFrame {
  const workspaceRail = root.querySelector<HTMLElement>("#workspace-rail");
  const contextualNavigation = root.querySelector<HTMLElement>("#contextual-navigation");
  const shellScreen = root.querySelector<HTMLElement>("#shell-screen");
  const packageMount = root.querySelector<HTMLElement>("#package-mount");
  if (!workspaceRail || !contextualNavigation || !shellScreen || !packageMount) {
    throw new Error("Shell frame is unavailable");
  }
  return Object.freeze({ workspaceRail, contextualNavigation, shellScreen, packageMount });
}

function hasQueries(element: HTMLElement): boolean {
  return typeof element.querySelector === "function" && typeof element.querySelectorAll === "function";
}

function routeWorkspaceId(route: ShellRoute): string | null {
  return route.kind === "workspace-chooser"
    ? null
    : route.kind === "package-instance" ? route.instance.workspaceId : route.workspaceId;
}

function workspaceMark(name: string): string {
  return name.trim().slice(0, 2).toLocaleUpperCase() || "W";
}

function themeButton(option: (typeof THEME_OPTIONS)[number], selected: ThemePreference): string {
  const pressed = option.preference === selected ? "true" : "false";
  return `<button type="button" class="theme-switcher__option" data-theme-preference="${option.preference}" aria-pressed="${pressed}" aria-label="${option.label}" title="${option.label}">${option.icon}</button>`;
}

function isThemePreference(value: string): value is ThemePreference {
  return value === "system" || value === "light" || value === "dark";
}

const SETTINGS_ICON = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>';

function packageButton(
  row: PackageInstancePresentation,
  route: ShellRoute,
  packageActionTargets: readonly PackageInstanceKey[],
): string {
  const instance = packageActionTargets[row.actionIndex];
  const current = route.kind === "package-instance" && instance
    && route.instance.packageIdentity === instance.packageIdentity
    && route.instance.workspaceId === instance.workspaceId
    ? ' aria-current="page"'
    : "";
  const localMarker = row.installationLocal
    ? '<span class="package-installation-marker">Runs only on this Installation</span>'
    : "";
  const sourceMarker = `<span class="package-source" aria-label="Package source: ${row.sourceLabel}">${row.sourceLabel}</span>`;
  return `<li><button type="button" data-package-index="${row.actionIndex}"${current}><span>${escape(row.name)}</span><small>${escape(row.availability)}</small>${sourceMarker}${localMarker}</button></li>`;
}

function renderPackageInstanceSettings(
  presentation: PackageNavigationPresentation,
  packageActionTargets: readonly PackageInstanceKey[],
  feedback: PackageLifecycleFeedback | null,
): string {
  const rows = packageRows(presentation);
  if (rows.length === 0) return '<p class="empty-state">No Packages are available for this Workspace.</p>';
  return `<div class="package-instance-list">${rows.map((row) => {
    const localMarker = row.installationLocal
      ? '<span class="package-installation-marker">Runs only on this Installation</span>'
      : "";
    const sourceMarker = `<span class="package-source" aria-label="Package source: ${row.sourceLabel}">${row.sourceLabel}</span>`;
    const instance = packageActionTargets[row.actionIndex];
    const rowFeedback = instance && feedback && samePackageInstance(instance, feedback.instance)
      ? renderPackageLifecycleFeedback(row, feedback)
      : "";
    return `<article class="package-instance" aria-labelledby="package-instance-name-${row.actionIndex}"><div><h3 id="package-instance-name-${row.actionIndex}">${escape(row.name)}</h3><p>${escape(row.availability)}</p>${sourceMarker}${localMarker}</div>
      ${rowFeedback}${renderPackageInstanceActions(row)}</article>`;
  }).join("")}</div>`;
}

function renderPackageLifecycleFeedback(
  row: PackageInstancePresentation,
  feedback: PackageLifecycleFeedback,
): string {
  const verb = feedback.action === "recover" ? "recover" : "dispose of";
  const past = feedback.action === "recover" ? "Recovered" : "Disposed of";
  const refreshFailed = feedback.failure === "refresh" || feedback.failure === "operation-and-refresh";
  const operationFailed = feedback.failure === "operation" || feedback.failure === "operation-and-refresh";
  const message = operationFailed
    ? `Could not ${verb} ${row.name}${refreshFailed ? ", and Package information could not be refreshed" : ""}.`
    : `${past} ${row.name}, but Package information could not be refreshed.`;
  const retry = operationFailed
    ? `<button type="button" data-package-failure-action="${feedback.action}">Retry ${escape(lifecycleActionLabel(feedback.action))}</button>`
    : "";
  return `<div class="package-lifecycle-feedback" role="alert"><p>${escape(message)}</p>${retry}</div>`;
}

function renderPackageSnapshotFailure(failed: boolean, hasSnapshot: boolean): string {
  if (!failed) return "";
  const message = hasSnapshot
    ? "Package information may be out of date. Showing the latest available Package information."
    : "Package information is unavailable.";
  return `<div class="package-snapshot-failure" role="alert"><p>${message}</p><button type="button" data-package-snapshot-retry>Retry Package information</button></div>`;
}

// Only Package Instances that need recovery get lifecycle controls; healthy rows stay quiet.
function renderPackageInstanceActions(row: PackageInstancePresentation): string {
  if (!row.canRecover) return "";
  const buttons = [
    lifecycleButton("Recover Backend", "recover", row.actionIndex, row.name),
    row.canDispose ? lifecycleButton("Dispose", "dispose", row.actionIndex, row.name) : "",
  ].join("");
  return `<div class="package-instance-actions">${buttons}</div>`;
}

function lifecycleButton(label: string, action: PackageLifecycleAction, index: number, name: string): string {
  return `<button type="button" data-package-action="${action}" data-package-action-index="${index}" aria-label="${label} ${escape(name)}">${label}</button>`;
}

function packageRows(presentation: PackageNavigationPresentation): readonly PackageInstancePresentation[] {
  return [...presentation.teamPackages, ...presentation.privatePackages];
}

function lifecycleActionAllowed(row: PackageInstancePresentation, action: string | undefined): action is PackageLifecycleAction {
  return row.canRecover && (action === "recover" || (action === "dispose" && row.canDispose));
}

function isLifecycleAction(action: string | undefined): action is PackageLifecycleAction {
  return action === "recover" || action === "dispose";
}

function lifecycleActionLabel(action: PackageLifecycleAction): string {
  return action === "recover" ? "Recover Backend" : "Dispose";
}

function samePackageInstance(left: PackageInstanceKey, right: PackageInstanceKey): boolean {
  return left.packageIdentity === right.packageIdentity && left.workspaceId === right.workspaceId;
}

function escape(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}
