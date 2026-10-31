import type { PackageHostSnapshot, PackageInstanceKey, PackageManagementBridge, PackageUiView } from "./package-contracts";
import { PackageManagementController, type RegistrationFeedback } from "./package-install";
import { buildPackagePagePresentation, PackagePageController, type PackagePageTab } from "./package-page";
import type { PackageLifecycleFailure, PackageLifecycleResult } from "./package-shell";
import type { InstallationPackageVisibility } from "./package-visibility";
import {
  buildPackageNavigationPresentation,
  buildShellPresentation,
  type PackageInstancePresentation,
  type PackageNavigationPresentation,
  type ShellPresentation,
} from "./presentation";
import type { StatusMessageTimers } from "./status-message";
import type { InstallationThemeController, ThemePreference } from "./theme";
import { THEME_OPTIONS } from "./theme-icons";
import { renderShell, type ShellModel } from "./view";

export type ShellRoute =
  | Readonly<{ kind: "workspace-chooser" }>
  | Readonly<{ kind: "workspace-settings"; workspaceId: string }>
  /** The main view of an open Package UI. */
  | Readonly<{ kind: "package-instance"; instance: PackageInstanceKey }>
  /**
   * A Package page. `tab` is About when absent. Configuration mounts the
   * Package's settings view below the page header.
   */
  | Readonly<{ kind: "package-page"; workspaceId: string; packageIdentity: string; tab?: PackagePageTab }>;

export type ShellFrame = Readonly<{
  workspaceRail: HTMLElement;
  contextualNavigation: HTMLElement;
  shellScreen: HTMLElement;
  packageMount: HTMLElement;
}>;

type PackageRouter = Readonly<{
  refresh(): Promise<PackageHostSnapshot>;
  select(instance: PackageInstanceKey, view?: PackageUiView): Promise<void>;
  hide(): Promise<void>;
  suspendBackend(instance: PackageInstanceKey): Promise<PackageLifecycleResult>;
  recoverBackend(instance: PackageInstanceKey): Promise<PackageLifecycleResult>;
  disposeBackend(instance: PackageInstanceKey): Promise<PackageLifecycleResult>;
}>;

type PackageLifecycleAction = "recover" | "dispose";

type PackageLifecycleFeedback = Readonly<{
  action: PackageLifecycleAction;
  instance: PackageInstanceKey;
  /** The Package's name when the action ran. */
  name: string;
  failure: PackageLifecycleFailure | null;
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
  packageManagement: PackageManagementBridge;
  theme: InstallationThemeController;
  packageVisibility: InstallationPackageVisibility;
  workspaceActions: WorkspaceActions;
  onRouteChange?: (route: ShellRoute) => void;
  /** Copies text for the person; defaults to the clipboard. */
  copy?: (value: string) => Promise<void>;
  /** The timer that hides success messages; defaults to the browser's. */
  timers?: StatusMessageTimers;
}>;

export class Shell {
  private selectedRoute: ShellRoute = { kind: "workspace-chooser" };
  private workspaceModel: ShellModel | null = null;
  private workspacePresentation: ShellPresentation | null = null;
  private packageSnapshot: PackageHostSnapshot | null = null;
  private packageSnapshotFailed = false;
  /** The last failed Recover or Dispose, for Retry and for a later successful refresh. */
  private packageLifecycleFeedback: PackageLifecycleFeedback | null = null;
  /** The message that result put in the Workspace page's message area. */
  private packageLifecycleMessage: RegistrationFeedback | null = null;
  private observedActiveWorkspaceId: string | null = null;
  private packageActionTargets: readonly PackageInstanceKey[] = [];
  private packagePresentation: PackageNavigationPresentation = { teamPackages: [], privatePackages: [] };
  private presentationRevision = 0;
  private packagePageTabFocus = false;
  /** Counts navigations, so one that finishes unmounting after a newer one started leaves the newer one's mount alone. */
  private navigation = 0;
  private readonly packageManagement: PackageManagementController;
  private readonly packagePage: PackagePageController;

  public constructor(
    private readonly shellFrame: ShellFrame,
    private readonly options: ShellOptions,
  ) {
    this.shellFrame.packageMount.hidden = true;
    this.packageManagement = new PackageManagementController(options.packageManagement, {
      changed: () => { this.renderPackageConsumers(); },
      refresh: async () => {
        try {
          this.updatePackages(await this.options.packageShell.refresh());
        } catch (error: unknown) {
          this.updatePackageSnapshotFailure();
          throw error;
        }
      },
    }, options.timers);
    this.packagePage = new PackagePageController(options.packageManagement, {
      suspend: (instance) => this.runPageLifecycle(() => this.options.packageShell.suspendBackend(instance)),
      recover: (instance) => this.runPageLifecycle(() => this.options.packageShell.recoverBackend(instance)),
      dispose: (instance) => this.runPageLifecycle(() => this.options.packageShell.disposeBackend(instance)),
    }, this.packageManagement, {
      changed: () => { this.renderPackageConsumers(); },
      refresh: async () => {
        try {
          this.updatePackages(await this.options.packageShell.refresh());
        } catch (error: unknown) {
          this.updatePackageSnapshotFailure();
          throw error;
        }
      },
      removed: (target) => { void this.navigate({ kind: "workspace-settings", workspaceId: target.workspaceId }); },
      copy: (value) => (this.options.copy ?? ((text) => navigator.clipboard.writeText(text)))(value),
    }, options.timers);
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
    this.packagePage.snapshotChanged(snapshot);
    const lifecycle = this.packageLifecycleFeedback;
    if (lifecycle && (lifecycle.failure === "refresh" || lifecycle.failure === "operation-and-refresh")) {
      // The refresh worked after all, so only the operation's own result is left to show.
      this.showPackageLifecycleFeedback(lifecycle.failure === "refresh"
        ? { ...lifecycle, failure: null }
        : { ...lifecycle, failure: "operation" });
    }
    this.renderPackageConsumers();
  }

  public updatePackageSnapshotFailure(): void {
    this.packageSnapshotFailed = true;
    this.renderPackageConsumers();
  }

  public async navigate(requested: ShellRoute): Promise<void> {
    const navigation = ++this.navigation;
    const route = this.availableRoute(requested);
    this.setRoute(route);
    if (route.kind === "package-instance") {
      placePackageMount(this.shellFrame.packageMount, "pane");
      this.shellFrame.shellScreen.hidden = true;
      this.shellFrame.packageMount.hidden = false;
      this.renderContextualNavigation();
      await this.options.packageShell.select(route.instance, "main");
      return;
    }

    if (route.kind === "package-page" && route.tab === "configuration") {
      // The settings view mounts below the page header and its tabs.
      placePackageMount(this.shellFrame.packageMount, "package-page");
      this.shellFrame.shellScreen.hidden = false;
      this.shellFrame.packageMount.hidden = false;
      // Select before awaiting anything, so a later route's unmount is ordered after this mount.
      const selected = this.options.packageShell.select(
        { packageIdentity: route.packageIdentity, workspaceId: route.workspaceId },
        "settings",
      );
      if (await this.preparePresentation()) this.renderAll();
      await selected;
      return;
    }

    // A failed unmount still leaves the Shell screen. Only an unacknowledged unmount is
    // recorded as a UI fault; other cleanup failures are logged here.
    await this.options.packageShell.hide().catch(warnUnmountFailure);
    if (navigation !== this.navigation) return;
    placePackageMount(this.shellFrame.packageMount, "pane");
    this.shellFrame.packageMount.hidden = true;
    this.shellFrame.shellScreen.hidden = false;
    if (await this.preparePresentation()) this.renderAll();
  }

  /** The Configuration tab needs a declared settings view and an Instance here; otherwise the page opens on About. */
  private availableRoute(route: ShellRoute): ShellRoute {
    if (route.kind !== "package-page" || route.tab === undefined) return route;
    if (route.tab === "configuration" && buildPackagePagePresentation(this.packageSnapshot, route)?.settings === true) {
      return route;
    }
    return { kind: "package-page", workspaceId: route.workspaceId, packageIdentity: route.packageIdentity };
  }

  /** Selects a Package page tab and keeps keyboard focus on it, because rendering replaces the tabs. */
  private async selectPackagePageTab(tab: string | undefined): Promise<void> {
    const route = this.selectedRoute;
    if (route.kind !== "package-page" || (tab !== "about" && tab !== "configuration")) return;
    if ((route.tab ?? "about") === tab) return;
    this.packagePageTabFocus = true;
    await this.navigate({
      kind: "package-page",
      workspaceId: route.workspaceId,
      packageIdentity: route.packageIdentity,
      ...(tab === "configuration" ? { tab } : {}),
    });
  }

  private setRoute(route: ShellRoute): void {
    if (route.kind === "package-page") {
      const current = this.packagePage.current();
      if (!current || current.packageIdentity !== route.packageIdentity || current.workspaceId !== route.workspaceId) {
        this.packagePage.close();
        this.packagePage.open({ workspaceId: route.workspaceId, packageIdentity: route.packageIdentity }, this.packageSnapshot);
      }
    } else {
      this.packagePage.close();
    }
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
    const visible = (row: PackageInstancePresentation): boolean => {
      const instance = this.packageActionTargets[row.actionIndex];
      return !instance || !this.options.packageVisibility.isHidden(instance);
    };
    const team = this.packagePresentation.teamPackages.filter(visible)
      .map((row) => packageButton(row, this.selectedRoute, this.packageActionTargets)).join("");
    const privatePackages = this.packagePresentation.privatePackages.filter(visible)
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
    if (this.selectedRoute.kind === "package-instance" || isPackageConfiguration(this.selectedRoute)) {
      void this.options.packageShell.hide().catch(warnUnmountFailure);
    }
    placePackageMount(this.shellFrame.packageMount, "pane");
    this.shellFrame.packageMount.hidden = true;
    this.shellFrame.shellScreen.hidden = false;
  }

  private renderSelectedScreen(): void {
    if (!this.workspacePresentation || this.selectedRoute.kind === "package-instance") return;
    if (this.selectedRoute.kind === "package-page") {
      this.shellFrame.shellScreen.innerHTML = renderShell(this.workspacePresentation, this.selectedRoute, {
        packagePage: `${renderPackageSnapshotFailure(this.packageSnapshotFailed, this.packageSnapshot !== null)}${this.packagePage.render(this.packageSnapshot, this.selectedRoute.tab ?? "about")}`,
      });
      this.bindSelectedScreen();
      const pageOpened = this.packagePage.takeFocusRequest();
      const tabSelected = this.packagePageTabFocus;
      this.packagePageTabFocus = false;
      if (!hasQueries(this.shellFrame.shellScreen)) return;
      if (pageOpened) {
        this.shellFrame.shellScreen.querySelector<HTMLElement>("[data-package-page-focus]")?.focus();
      } else if (tabSelected) {
        // Rendering replaced the tabs, so return keyboard focus to the selected one.
        this.shellFrame.shellScreen.querySelector<HTMLElement>(`[data-package-page-tab="${this.selectedRoute.tab ?? "about"}"]`)?.focus();
      }
      return;
    }
    const packageSettings = this.selectedRoute.kind === "workspace-settings"
      ? `${renderPackageSnapshotFailure(this.packageSnapshotFailed, this.packageSnapshot !== null)}${this.packageManagement.renderMessage(this.packageSnapshot)}${this.packageManagement.renderPanel(this.packageSnapshot)}${renderPackageInstanceSettings(
        this.packagePresentation,
        this.packageActionTargets,
        this.options.packageVisibility,
      )}${this.packageManagement.renderRegistrations(this.packageSnapshot)}`
      : undefined;
    this.shellFrame.shellScreen.innerHTML = renderShell(
      this.workspacePresentation,
      this.selectedRoute,
      { packageSettings },
    );
    this.bindSelectedScreen();
    this.focusPackageInstallStep();
  }

  private focusPackageInstallStep(): void {
    if (!this.packageManagement.takeFocusRequest() || !hasQueries(this.shellFrame.shellScreen)) return;
    const selector = this.packageManagement.state().kind === "closed"
      ? '[data-package-install-action="open"]'
      : "[data-package-install-focus]";
    this.shellFrame.shellScreen.querySelector<HTMLElement>(selector)?.focus();
  }

  private bindPackageManagement(): void {
    const screen = this.shellFrame.shellScreen;
    screen.querySelectorAll<HTMLButtonElement>("[data-package-install-action]").forEach((button) => {
      button.addEventListener("click", () => {
        void this.packageManagement.installAction(button.dataset.packageInstallAction);
      });
    });
    screen.querySelector<HTMLFormElement>("#package-install-path-form")?.addEventListener("submit", (event) => {
      event.preventDefault();
      const path = screen.querySelector<HTMLInputElement>("#package-install-path");
      if (path) void this.packageManagement.inspectPath(path.value);
    });
    screen.querySelectorAll<HTMLButtonElement>("[data-package-registration-action]").forEach((button) => {
      button.addEventListener("click", () => {
        const identity = this.packageManagement.registrationIdentity(
          this.packageSnapshot,
          Number(button.dataset.packageRegistrationIndex),
        );
        if (identity) {
          void this.packageManagement.registrationAction(
            button.dataset.packageRegistrationAction,
            identity,
            this.packageSnapshot,
          );
        }
      });
    });
    screen.querySelector<HTMLButtonElement>("[data-package-change-restart]")?.addEventListener("click", () => {
      void this.packageManagement.restartAfterChange();
    });
    screen.querySelector<HTMLButtonElement>("[data-package-message-dismiss]")?.addEventListener("click", () => {
      this.packageManagement.dismissMessage();
    });
    screen.querySelectorAll<HTMLButtonElement>("[data-package-registration-page]").forEach((button) => {
      button.addEventListener("click", () => {
        const identity = this.packageManagement.registrationIdentity(
          this.packageSnapshot,
          Number(button.dataset.packageRegistrationPage),
        );
        const workspaceId = routeWorkspaceId(this.selectedRoute);
        if (identity && workspaceId) void this.navigate({ kind: "package-page", workspaceId, packageIdentity: identity });
      });
    });
    screen.querySelectorAll<HTMLButtonElement>("[data-package-page-action]").forEach((button) => {
      button.addEventListener("click", () => {
        void this.packagePage.action(button.dataset.packagePageAction, this.packageSnapshot);
      });
    });
    screen.querySelectorAll<HTMLButtonElement>("[data-package-page-tab]").forEach((button) => {
      button.addEventListener("click", () => { void this.selectPackagePageTab(button.dataset.packagePageTab); });
    });
    screen.querySelector<HTMLElement>("[data-package-page-tabs]")?.addEventListener("keydown", (event) => {
      const tab = adjacentPackagePageTab(this.selectedRoute, screen, (event as KeyboardEvent).key);
      if (!tab) return;
      event.preventDefault();
      void this.selectPackagePageTab(tab);
    });
    this.bindPackagePageLinks(screen);
  }

  /** Package rows open their Package page through `data-package-page-index`. */
  private bindPackagePageLinks(region: HTMLElement): void {
    region.querySelectorAll<HTMLButtonElement>("[data-package-page-index]").forEach((button) => {
      button.addEventListener("click", () => {
        const instance = this.packageActionTargets[Number(button.dataset.packagePageIndex)];
        if (instance) {
          void this.navigate({ kind: "package-page", workspaceId: instance.workspaceId, packageIdentity: instance.packageIdentity });
        }
      });
    });
  }

  /** Applies a lifecycle result for the Package page, and throws when the operation failed. */
  private async runPageLifecycle(operation: () => Promise<PackageLifecycleResult>): Promise<void> {
    const result = await operation();
    if (result.snapshot) {
      this.packageSnapshot = result.snapshot;
      this.packageSnapshotFailed = false;
    }
    if (result.failure === "refresh" || result.failure === "operation-and-refresh") this.packageSnapshotFailed = true;
    if (result.failure === "operation" || result.failure === "operation-and-refresh") {
      throw new Error("Package lifecycle operation failed");
    }
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
      for (const selector of ["#refresh-membership", "#retry-membership-refresh"]) {
        this.shellFrame.shellScreen.querySelector<HTMLButtonElement>(selector)?.addEventListener("click", () => {
          void this.options.workspaceActions.refreshMembership(workspaceId);
        });
      }
    }
    this.shellFrame.shellScreen.querySelector<HTMLButtonElement>("#retry")?.addEventListener("click", () => {
      void this.options.workspaceActions.retry();
    });
    this.bindPackageSnapshotRetry(this.shellFrame.shellScreen);
    this.bindPackageManagement();
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
    this.shellFrame.shellScreen.querySelectorAll<HTMLButtonElement>("[data-package-visibility-index]").forEach((button) => {
      button.addEventListener("click", () => {
        const index = Number(button.dataset.packageVisibilityIndex);
        const instance = this.packageActionTargets[index];
        if (!instance) return;
        // Toggling is the next action, so an earlier result no longer applies.
        this.packageManagement.clearMessage();
        this.options.packageVisibility.setHidden(instance, !this.options.packageVisibility.isHidden(instance));
        this.renderPackageConsumers();
        // Re-rendering replaces the buttons, so return keyboard focus to the toggled one.
        this.shellFrame.shellScreen.querySelector<HTMLButtonElement>(`[data-package-visibility-index="${index}"]`)?.focus();
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
    const route = this.selectedRoute;
    // Configuration closes when its settings view is no longer available, for example after a Dispose.
    if (isPackageConfiguration(route) && this.availableRoute(route) !== route) {
      void this.navigate(route);
      return;
    }
    this.renderContextualNavigation();
    if (this.selectedRoute.kind === "workspace-settings" || this.selectedRoute.kind === "package-page") {
      this.renderSelectedScreen();
    }
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
    // A new action replaces the previous result in the message area.
    this.packageManagement.clearMessage();
    this.packageLifecycleFeedback = null;
    this.packageLifecycleMessage = null;
    const name = this.packageInstanceName(instance);
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
    this.showPackageLifecycleFeedback({ action, instance, name, failure: result.failure });
    this.renderPackageConsumers();
  }

  /**
   * Shows a Recover or Dispose result in the Workspace page's message area.
   * A failed operation keeps what Retry needs; anything else forgets it.
   */
  private showPackageLifecycleFeedback(feedback: PackageLifecycleFeedback): void {
    // A result the person dismissed, or a later action replaced, is not shown again.
    if (this.packageLifecycleFeedback && this.packageManagement.currentMessage() !== this.packageLifecycleMessage) {
      this.packageLifecycleFeedback = null;
      this.packageLifecycleMessage = null;
      return;
    }
    const message = packageLifecycleMessage(feedback);
    this.packageManagement.showMessage(message);
    this.packageLifecycleFeedback = feedback.failure === null ? null : feedback;
    this.packageLifecycleMessage = message;
  }

  private packageInstanceName(instance: PackageInstanceKey): string {
    const index = this.packageActionTargets.findIndex((target) => samePackageInstance(target, instance));
    return packageRows(this.packagePresentation).find(({ actionIndex }) => actionIndex === index)?.name
      ?? instance.packageIdentity;
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

function isPackageConfiguration(route: ShellRoute): route is Extract<ShellRoute, { kind: "package-page" }> {
  return route.kind === "package-page" && route.tab === "configuration";
}

/**
 * Where the Package mount sits: the whole pane for a Package UI, or below a
 * Package page's header as that page's Configuration tab panel.
 */
function placePackageMount(mount: HTMLElement, placement: "pane" | "package-page"): void {
  if (typeof mount.setAttribute !== "function") return;
  if (placement === "package-page") {
    mount.setAttribute("data-placement", "package-page");
    mount.setAttribute("role", "tabpanel");
    mount.setAttribute("aria-labelledby", "package-page-tab-configuration");
    mount.removeAttribute("aria-label");
  } else {
    mount.removeAttribute("data-placement");
    mount.removeAttribute("role");
    mount.removeAttribute("aria-labelledby");
    mount.setAttribute("aria-label", "Package interface");
  }
}

/** The tab an arrow, Home, or End key moves to, among the tabs the page shows. */
function adjacentPackagePageTab(route: ShellRoute, screen: HTMLElement, key: string): PackagePageTab | null {
  if (route.kind !== "package-page") return null;
  const tabs = [...screen.querySelectorAll<HTMLElement>("[data-package-page-tab]")]
    .map((tab) => tab.dataset.packagePageTab)
    .filter((tab): tab is PackagePageTab => tab === "about" || tab === "configuration");
  const index = tabs.indexOf(route.tab ?? "about");
  if (index < 0 || tabs.length === 0) return null;
  const step = key === "ArrowRight" ? 1 : key === "ArrowLeft" ? -1 : 0;
  const next = key === "Home" ? tabs[0] : key === "End" ? tabs.at(-1) : step ? tabs[(index + step + tabs.length) % tabs.length] : undefined;
  return next ?? null;
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

const EYE_OPEN_ICON = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/></svg>';

const EYE_CLOSED_ICON = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M9.88 9.88a3 3 0 1 0 4.24 4.24"/><path d="M10.73 5.08A10.43 10.43 0 0 1 12 5c6.5 0 10 7 10 7a13.16 13.16 0 0 1-1.67 2.68"/><path d="M6.61 6.61A13.53 13.53 0 0 0 2 12s3.5 7 10 7a9.74 9.74 0 0 0 5.39-1.61"/><path d="M2 2l20 20"/></svg>';

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
  return `<li><button type="button" data-package-index="${row.actionIndex}"${current}><span>${escape(row.name)}</span><small>${escape(row.availability)}</small></button></li>`;
}

function renderPackageInstanceSettings(
  presentation: PackageNavigationPresentation,
  packageActionTargets: readonly PackageInstanceKey[],
  visibility: InstallationPackageVisibility,
): string {
  const rows = packageRows(presentation);
  if (rows.length === 0) return '<p class="empty-state">No Packages are available for this Workspace.</p>';
  return `<div class="package-instance-list">${rows.map((row) => {
    const localMarker = row.installationLocal
      ? '<span class="package-installation-marker">Runs only on this Installation</span>'
      : "";
    const sourceMarker = `<span class="package-source" aria-label="Package source: ${row.sourceLabel}">${row.sourceLabel}</span>`;
    const instance = packageActionTargets[row.actionIndex];
    const visibilityToggle = instance ? packageVisibilityButton(row, visibility.isHidden(instance)) : "";
    return `<article class="package-instance" aria-labelledby="package-instance-name-${row.actionIndex}"><div class="package-instance-summary"><div><h3 id="package-instance-name-${row.actionIndex}"><button type="button" class="package-instance-link" data-package-page-index="${row.actionIndex}" aria-label="${escape(`About ${row.name}`)}">${escape(row.name)}</button></h3><p>${escape(row.availability)}</p>${sourceMarker}${localMarker}</div>${visibilityToggle}</div>
      ${renderPackageInstanceActions(row)}</article>`;
  }).join("")}</div>`;
}

function packageVisibilityButton(row: PackageInstancePresentation, hidden: boolean): string {
  const label = `${hidden ? "Show" : "Hide"} ${row.name} in navigation`;
  return `<button type="button" class="package-visibility-toggle" data-package-visibility-index="${row.actionIndex}" aria-pressed="${hidden ? "false" : "true"}" aria-label="${escape(label)}" title="${escape(label)}">${hidden ? EYE_CLOSED_ICON : EYE_OPEN_ICON}</button>`;
}

/** The message area's copy for a Recover or Dispose result; the slot names the Package. */
function packageLifecycleMessage(feedback: PackageLifecycleFeedback): RegistrationFeedback {
  const verb = feedback.action === "recover" ? "recover" : "dispose";
  const past = feedback.action === "recover" ? "Recovered" : "Disposed";
  const refreshFailed = feedback.failure === "refresh" || feedback.failure === "operation-and-refresh";
  const operationFailed = feedback.failure === "operation" || feedback.failure === "operation-and-refresh";
  const message = operationFailed
    ? `Could not ${verb}${refreshFailed ? ", and Package information could not be refreshed" : ""}.`
    : refreshFailed
      ? `${past}, but Package information could not be refreshed.`
      : `${past}.`;
  return {
    identity: feedback.instance.packageIdentity,
    name: feedback.name,
    message,
    failed: feedback.failure !== null,
    restart: false,
    ...(operationFailed ? { retry: feedback.action } : {}),
  };
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

/**
 * Logs a failed Package UI unmount for diagnosis; the person sees no change.
 * It logs an Error's message or a bridge error's code, never a payload.
 */
function warnUnmountFailure(error: unknown): void {
  const code = typeof error === "object" && error !== null && "code" in error && typeof error.code === "string"
    ? error.code
    : null;
  console.warn("Package UI unmount cleanup failed", { reason: error instanceof Error ? error.message : code ?? typeof error });
}

function isLifecycleAction(action: string | undefined): action is PackageLifecycleAction {
  return action === "recover" || action === "dispose";
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
