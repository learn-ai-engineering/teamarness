import type { PackageHostSnapshot, PackageInstanceKey } from "./package-contracts";
import {
  buildPackageNavigationPresentation,
  buildShellPresentation,
  type ShellPresentation,
} from "./presentation";
import type { InstallationThemeController, ThemePreference } from "./theme";
import { renderShell, type ShellModel } from "./view";

export type ShellRoute =
  | Readonly<{ kind: "workspace-chooser" }>
  | Readonly<{ kind: "workspace-overview"; workspaceId: string }>
  | Readonly<{ kind: "workspace-invitations"; workspaceId: string }>
  | Readonly<{ kind: "workspace-settings"; workspaceId: string }>
  | Readonly<{ kind: "package-instance"; instance: PackageInstanceKey }>;

export type ShellFrame = Readonly<{
  workspaceRail: HTMLElement;
  contextualNavigation: HTMLElement;
  shellScreen: HTMLElement;
  packageMount: HTMLElement;
}>;

type PackageRouter = Readonly<{
  select(instance: PackageInstanceKey): Promise<void>;
  hide(): Promise<void>;
}>;

type WorkspaceActions = Readonly<{
  selectWorkspace(workspaceId: string): void;
  createWorkspace(name: string): Promise<void>;
  joinWorkspace(invitation: string): Promise<void>;
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
  private observedActiveWorkspaceId: string | null = null;
  private packageRoutes: readonly PackageInstanceKey[] = [];
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
      this.setRoute({ kind: "workspace-overview", workspaceId: model.activeWorkspaceId });
    }
    if (await this.preparePresentation()) this.renderAll();
  }

  public updatePackages(snapshot: PackageHostSnapshot): void {
    this.packageSnapshot = snapshot;
    this.renderContextualNavigation();
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
    this.shellFrame.workspaceRail.innerHTML = `<div class="workspace-rail__brand" aria-label="Teamarness">T</div>
      <ul class="workspace-switcher">${workspaces}</ul>
      <button class="workspace-add" id="choose-workspace" type="button" aria-label="Add or join a Workspace">+</button>
      <details class="installation-menu"><summary>Installation</summary>
        ${presentation.installationName ? `<p>${escape(presentation.installationName)}</p>` : '<p>Loading…</p>'}
        <label for="theme-preference">Theme</label>
        <select id="theme-preference">
          ${themeOption("system", "System", preference)}
          ${themeOption("light", "Light", preference)}
          ${themeOption("dark", "Dark", preference)}
        </select>
      </details>`;
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
    this.shellFrame.workspaceRail.querySelector<HTMLSelectElement>("#theme-preference")?.addEventListener("change", (event) => {
      const value = (event.currentTarget as HTMLSelectElement).value;
      if (isThemePreference(value)) this.options.theme.selectPreference(value);
      this.renderWorkspaceRail();
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
      this.packageRoutes = [];
      this.shellFrame.contextualNavigation.innerHTML = '<div class="contextual-navigation__empty"><p>Choose a Workspace</p></div>';
      return;
    }

    this.packageRoutes = this.packageSnapshot?.instances
      .filter(({ key }) => key.workspaceId === active.id)
      .map(({ key }) => key) ?? [];
    const packages = buildPackageNavigationPresentation(this.packageSnapshot, active.id);
    const team = packages.teamPackages.map((row) => packageButton(row.name, row.actionIndex, this.selectedRoute, this.packageRoutes)).join("");
    const privatePackages = packages.privatePackages.map((row) => packageButton(row.name, row.actionIndex, this.selectedRoute, this.packageRoutes)).join("");

    this.shellFrame.contextualNavigation.innerHTML = `<header class="workspace-context"><p>Workspace</p><h1>${escape(activePresentation.name)}</h1></header>
      <nav class="contextual-navigation__body" aria-label="Active Workspace"><ul class="screen-navigation">
        ${screenButton("Overview", "workspace-overview", active.id, this.selectedRoute)}
        ${screenButton("Invitations", "workspace-invitations", active.id, this.selectedRoute)}
        ${screenButton("Settings", "workspace-settings", active.id, this.selectedRoute)}
      </ul>
      <section class="package-group" aria-labelledby="team-packages-heading"><h2 id="team-packages-heading">Team Packages</h2><ul>${team || "<li>None available</li>"}</ul></section>
      <section class="package-group" aria-labelledby="private-packages-heading"><h2 id="private-packages-heading">Private Packages</h2><ul>${privatePackages || "<li>None available</li>"}</ul></section></nav>`;

    if (!hasQueries(this.shellFrame.contextualNavigation)) return;
    this.shellFrame.contextualNavigation.querySelectorAll<HTMLButtonElement>("[data-screen]").forEach((button) => {
      button.addEventListener("click", () => {
        const screen = button.dataset.screen;
        if (screen === "workspace-overview" || screen === "workspace-invitations" || screen === "workspace-settings") {
          void this.navigate({ kind: screen, workspaceId: active.id });
        }
      });
    });
    this.shellFrame.contextualNavigation.querySelectorAll<HTMLButtonElement>("[data-package-index]").forEach((button) => {
      button.addEventListener("click", () => {
        const instance = this.packageRoutes[Number(button.dataset.packageIndex)];
        if (instance) void this.navigate({ kind: "package-instance", instance });
      });
    });
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
    this.shellFrame.shellScreen.innerHTML = renderShell(this.workspacePresentation, this.selectedRoute);
    this.bindSelectedScreen();
  }

  private bindSelectedScreen(): void {
    const model = this.workspaceModel;
    if (!model || !hasQueries(this.shellFrame.shellScreen)) return;
    this.shellFrame.shellScreen.querySelector<HTMLFormElement>("#create-workspace")?.addEventListener("submit", (event) => {
      event.preventDefault();
      const input = this.shellFrame.shellScreen.querySelector<HTMLInputElement>("#workspace-name");
      if (input) void this.options.workspaceActions.createWorkspace(input.value);
    });
    this.shellFrame.shellScreen.querySelector<HTMLFormElement>("#join-workspace")?.addEventListener("submit", (event) => {
      event.preventDefault();
      const input = this.shellFrame.shellScreen.querySelector<HTMLTextAreaElement>("#invitation-value");
      if (input) void this.options.workspaceActions.joinWorkspace(input.value);
    });
    this.shellFrame.shellScreen.querySelectorAll<HTMLButtonElement>("[data-choose-workspace]").forEach((button) => {
      button.addEventListener("click", () => {
        const workspace = model.workspaces[Number(button.dataset.chooseWorkspace)];
        if (workspace) this.options.workspaceActions.selectWorkspace(workspace.id);
      });
    });
    const workspaceId = routeWorkspaceId(this.selectedRoute);
    if (workspaceId) {
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

function themeOption(value: ThemePreference, label: string, selected: ThemePreference): string {
  return `<option value="${value}"${value === selected ? " selected" : ""}>${label}</option>`;
}

function isThemePreference(value: string): value is ThemePreference {
  return value === "system" || value === "light" || value === "dark";
}

function screenButton(label: string, kind: Exclude<ShellRoute["kind"], "workspace-chooser" | "package-instance">, workspaceId: string, route: ShellRoute): string {
  const current = route.kind === kind && routeWorkspaceId(route) === workspaceId ? ' aria-current="page"' : "";
  return `<li><button type="button" data-screen="${kind}"${current}>${label}</button></li>`;
}

function packageButton(
  name: string,
  index: number,
  route: ShellRoute,
  packageRoutes: readonly PackageInstanceKey[],
): string {
  const instance = packageRoutes[index];
  const current = route.kind === "package-instance" && instance
    && route.instance.packageIdentity === instance.packageIdentity
    && route.instance.workspaceId === instance.workspaceId
    ? ' aria-current="page"'
    : "";
  return `<li><button type="button" data-package-index="${index}"${current}>${escape(name)}</button></li>`;
}

function escape(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}
