import { describe, expect, it, vi } from "vitest";
import fixture from "../contracts/fixtures/bridge-contract.json";
import type { Installation, Membership, Workspace } from "./contracts";
import type { PackageHostSnapshot, PackageInstanceKey } from "./package-contracts";
import type { PackageLifecycleResult } from "./package-shell";
import { Shell, type ShellFrame, type ShellRoute } from "./shell";
import type { ShellModel } from "./view";

const installation = fixture.installation as Installation;
const workspace = fixture.workspace as Workspace;
const membership = fixture.membership as Membership;
const otherWorkspace: Workspace = { ...workspace, id: "ffeeddccbbaa99887766554433221100", name: "Other" };
const packageKey: PackageInstanceKey = { packageIdentity: "com.example.one", workspaceId: workspace.id };
const privatePackageKey: PackageInstanceKey = { packageIdentity: "com.example.private", workspaceId: workspace.id };
const packages: PackageHostSnapshot = {
  revision: 739217,
  state: "running",
  packages: [
    {
      identity: packageKey.packageIdentity,
      version: "1.0.0-secret-version",
      displayName: "One",
      source: "bundled",
      trust: "application_bundled",
      uiContractDigest: "secret-contract-digest",
    },
    {
      identity: privatePackageKey.packageIdentity,
      version: "2.0.0-secret-version",
      displayName: "Private One",
      source: "local",
      trust: "trusted_native_code_v1",
      uiContractDigest: "private-secret-contract-digest",
    },
  ],
  instances: [
    { key: packageKey, lifecycle: "active", processGeneration: 414, mountState: "mounted" },
    { key: privatePackageKey, lifecycle: "failed", processGeneration: 415, mountState: "failed" },
    { key: { ...packageKey, workspaceId: otherWorkspace.id }, lifecycle: "suspended", processGeneration: 3, mountState: null },
  ],
  diagnostics: [{
    version: 1,
    namespace: "teamarness.package-host",
    code: "secret-diagnostic-code",
    stage: "secret-diagnostic-stage",
    affectedPackage: packageKey.packageIdentity,
    affectedInstance: packageKey,
    retryable: true,
    recovery: "secret-diagnostic-recovery",
    diagnosticId: "secret-diagnostic-id",
    packageError: null,
  }],
};

class Region {
  public innerHTML = "";
  public hidden = false;
}

class TestControl {
  public readonly dataset: Record<string, string> = {};
  private listener: EventListener | null = null;

  public addEventListener(_type: string, listener: EventListener): void {
    this.listener = listener;
  }

  public click(): void {
    this.listener?.({} as Event);
  }
}

class SettingsRegion extends Region {
  public readonly recover = new TestControl();
  public readonly dispose = new TestControl();
  public readonly invalidRecover = new TestControl();
  public readonly invalidDispose = new TestControl();
  public readonly suspend = new TestControl();
  public readonly retryLifecycle = new TestControl();
  public readonly retrySnapshot = new TestControl();

  public querySelector(selector: string): TestControl | null {
    if (selector === "[data-package-snapshot-retry]" && this.innerHTML.includes("data-package-snapshot-retry")) {
      return this.retrySnapshot;
    }
    return null;
  }

  public querySelectorAll(selector: string): readonly TestControl[] {
    if (selector === "[data-package-failure-action]") {
      this.retryLifecycle.dataset.packageFailureAction = "recover";
      this.retryLifecycle.dataset.packageActionIndex = "1";
      return this.innerHTML.includes("data-package-failure-action") ? [this.retryLifecycle] : [];
    }
    if (selector !== "[data-package-action]") return [];
    this.recover.dataset.packageAction = "recover";
    this.recover.dataset.packageActionIndex = "1";
    this.dispose.dataset.packageAction = "dispose";
    this.dispose.dataset.packageActionIndex = "1";
    this.invalidRecover.dataset.packageAction = "recover";
    this.invalidRecover.dataset.packageActionIndex = "0";
    this.invalidDispose.dataset.packageAction = "dispose";
    this.invalidDispose.dataset.packageActionIndex = "0";
    this.suspend.dataset.packageAction = "suspend";
    this.suspend.dataset.packageActionIndex = "0";
    return [this.recover, this.dispose, this.invalidRecover, this.invalidDispose, this.suspend];
  }
}

class NavRegion extends Region {
  public readonly settings = new TestControl();

  public querySelector(): TestControl | null {
    return null;
  }

  public querySelectorAll(selector: string): readonly TestControl[] {
    if (selector !== "[data-screen]") return [];
    this.settings.dataset.screen = "workspace-settings";
    return [this.settings];
  }
}

class ThemeRailRegion extends Region {
  public readonly dark = new TestControl();
  public focusedSelector: string | null = null;

  public querySelector(selector: string): (TestControl & { focus(): void }) | null {
    if (!selector.startsWith("[data-theme-preference=")) return null;
    return Object.assign(new TestControl(), { focus: () => { this.focusedSelector = selector; } });
  }

  public querySelectorAll(selector: string): readonly TestControl[] {
    if (selector !== "[data-theme-preference]") return [];
    this.dark.dataset.themePreference = "dark";
    return [this.dark];
  }
}

class RetryRegion extends Region {
  public readonly retry = new TestControl();

  public querySelector(selector: string): TestControl | null {
    return selector === "#retry" && this.innerHTML.includes('id="retry"') ? this.retry : null;
  }

  public querySelectorAll(): readonly TestControl[] {
    return [];
  }
}

function model(activeWorkspaceId: string | null = workspace.id): ShellModel {
  return {
    installation,
    workspaces: [workspace, otherWorkspace],
    memberships: { [workspace.id]: membership },
    personNames: {},
    activeWorkspaceId,
    loading: false,
    error: null,
    notice: null,
  };
}

function harness(
  shellScreen: HTMLElement = new Region() as unknown as HTMLElement,
  packageBehavior: Readonly<{
    refresh?: () => Promise<PackageHostSnapshot>;
    lifecycle?: (action: "recover" | "dispose", instance: PackageInstanceKey) => Promise<PackageLifecycleResult>;
  }> = {},
  workspaceRail: HTMLElement = new Region() as unknown as HTMLElement,
  contextualNavigation: HTMLElement = new Region() as unknown as HTMLElement,
): {
  shell: Shell;
  frame: ShellFrame;
  calls: string[];
  routes: ShellRoute[];
} {
  const calls: string[] = [];
  const routes: ShellRoute[] = [];
  const frame: ShellFrame = {
    workspaceRail,
    contextualNavigation,
    shellScreen,
    packageMount: new Region() as unknown as HTMLElement,
  };
  const shell = new Shell(frame, {
    packageShell: {
      refresh: packageBehavior.refresh ?? (async () => packages),
      select: async (instance) => { calls.push(`mount:${instance.packageIdentity}`); },
      hide: async () => { calls.push("unmount"); },
      recoverBackend: async (instance) => {
        calls.push(`recover:${instance.packageIdentity}`);
        return packageBehavior.lifecycle?.("recover", instance) ?? { snapshot: packages, failure: null };
      },
      disposeBackend: async (instance) => {
        calls.push(`dispose:${instance.packageIdentity}`);
        return packageBehavior.lifecycle?.("dispose", instance) ?? { snapshot: packages, failure: null };
      },
    },
    theme: {
      getPreference: () => "system",
      selectPreference: (preference) => { calls.push(`theme:${preference}`); },
      subscribe: () => () => undefined,
    },
    workspaceActions: {
      selectWorkspace: (workspaceId) => { calls.push(`workspace:${workspaceId}`); },
      createWorkspace: async () => undefined,
      joinWorkspace: async () => undefined,
      rename: async () => undefined,
      copyInvitation: async () => undefined,
      refreshMembership: async () => undefined,
      retry: async () => { calls.push("retry"); },
    },
    onRouteChange: (route) => { routes.push(route); },
  });
  return { shell, frame, calls, routes };
}

describe("persistent Shell frame", () => {
  it("keeps frame and Package mount identities across feedback, routes, and Package refreshes", async () => {
    const test = harness();
    const frame = test.shell.frame();
    const mount = frame.packageMount;

    await test.shell.updateWorkspace(model());
    await test.shell.updateWorkspace({ ...model(), notice: "Updated" });
    test.shell.updatePackages(packages);
    await test.shell.navigate({ kind: "workspace-settings", workspaceId: workspace.id });
    test.shell.updatePackages({ ...packages, revision: 2 });

    expect(test.shell.frame()).toBe(frame);
    expect(test.shell.frame().packageMount).toBe(mount);
    expect(test.frame.shellScreen.innerHTML).toContain("Workspace Invitations");
  });

  it("selects overview at startup and whenever the active Workspace switches", async () => {
    const test = harness();
    await test.shell.updateWorkspace(model());
    expect(test.shell.route()).toEqual({ kind: "workspace-settings", workspaceId: workspace.id });

    await test.shell.updateWorkspace(model(otherWorkspace.id));
    expect(test.shell.route()).toEqual({ kind: "workspace-settings", workspaceId: otherWorkspace.id });
    expect(test.routes).toEqual([
      { kind: "workspace-settings", workspaceId: workspace.id },
      { kind: "workspace-settings", workspaceId: otherWorkspace.id },
    ]);
  });

  it("uses the chooser on first run and exposes discriminated Shell routes", async () => {
    const test = harness();
    await test.shell.updateWorkspace({ ...model(null), workspaces: [], memberships: {} });
    expect(test.shell.route()).toEqual({ kind: "workspace-chooser" });

    const routes: ShellRoute[] = [
      { kind: "workspace-chooser" },
      { kind: "workspace-settings", workspaceId: workspace.id },
      { kind: "package-instance", instance: packageKey },
    ];
    for (const route of routes) await test.shell.navigate(route);
    expect(test.shell.route()).toEqual(routes.at(-1));
  });

  it("unmounts Package UI for Shell routes without Backend lifecycle calls", async () => {
    const test = harness();
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(packages);
    await test.shell.navigate({ kind: "package-instance", instance: packageKey });
    await test.shell.navigate({ kind: "workspace-settings", workspaceId: workspace.id });

    expect(test.calls).toEqual(["mount:com.example.one", "unmount"]);
    expect(test.frame.packageMount.hidden).toBe(true);
    expect(test.frame.shellScreen.hidden).toBe(false);
  });

  it("renders fixed landmarks, active-Workspace Package groups, safe availability, and the labelled local marker", async () => {
    const test = harness();
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(packages);
    await test.shell.navigate({ kind: "workspace-settings", workspaceId: workspace.id });

    expect(test.frame.workspaceRail.innerHTML).not.toContain("Installation");
    expect(test.frame.workspaceRail.innerHTML).not.toContain(installation.displayName);
    expect(test.frame.workspaceRail.innerHTML).not.toContain("<select");
    expect(test.frame.contextualNavigation.innerHTML).toContain('aria-label="Workspace settings"');
    expect(test.frame.contextualNavigation.innerHTML).not.toContain("Overview");
    expect(test.frame.contextualNavigation.innerHTML).toContain("Team Packages");
    expect(test.frame.contextualNavigation.innerHTML).toContain("Personal Packages");
    expect(test.frame.contextualNavigation.innerHTML).toContain("Package source: Team");
    expect(test.frame.contextualNavigation.innerHTML).toContain("Package source: Personal");
    expect(test.frame.contextualNavigation.innerHTML).toContain("One");
    expect(test.frame.contextualNavigation.innerHTML).toContain("Available");
    expect(test.frame.contextualNavigation.innerHTML).toContain("Private One");
    expect(test.frame.contextualNavigation.innerHTML).toContain("Needs recovery");
    expect(test.frame.contextualNavigation.innerHTML).toContain("Runs only on this Installation");
    expect(test.frame.contextualNavigation.innerHTML).not.toContain("Other");
    expect(test.frame.contextualNavigation.innerHTML).toContain('aria-current="page"');
  });

  it("renders empty Package groups and an intentional empty settings state", async () => {
    const test = harness();
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages({ ...packages, packages: [], instances: [] });
    await test.shell.navigate({ kind: "workspace-settings", workspaceId: workspace.id });

    expect(test.frame.contextualNavigation.innerHTML.match(/None available/g)).toHaveLength(2);
    expect(test.frame.shellScreen.innerHTML).toContain("No Packages are available for this Workspace.");
  });

  it("routes full Package keys through private settings mappings and gates lifecycle actions", async () => {
    const screen = new SettingsRegion();
    const test = harness(screen as unknown as HTMLElement);
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(packages);
    await test.shell.navigate({ kind: "workspace-settings", workspaceId: workspace.id });

    expect(screen.innerHTML).toContain("Package Instances");
    expect(screen.innerHTML).toContain("One");
    expect(screen.innerHTML).toContain("Private One");
    expect(screen.innerHTML).not.toContain("Suspend");
    expect(screen.innerHTML).not.toContain("disabled");
    expect(screen.innerHTML.match(/Recover Backend Private One/g)).toHaveLength(1);
    expect(screen.innerHTML.match(/Dispose Private One/g)).toHaveLength(1);
    expect(screen.innerHTML).not.toContain("Recover Backend One");
    expect(screen.innerHTML).not.toContain("Dispose One");
    expect(screen.innerHTML).not.toContain(packageKey.packageIdentity);
    expect(screen.innerHTML).not.toContain(privatePackageKey.packageIdentity);

    screen.invalidRecover.click();
    screen.invalidDispose.click();
    screen.suspend.click();
    screen.recover.click();
    screen.dispose.click();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(test.calls).toEqual([
      "unmount",
      `recover:${privatePackageKey.packageIdentity}`,
      `dispose:${privatePackageKey.packageIdentity}`,
    ]);
  });

  it("keeps a failed Package snapshot visible and retries it from the owning screens", async () => {
    const screen = new SettingsRegion();
    let refreshes = 0;
    const test = harness(screen as unknown as HTMLElement, {
      refresh: async () => { refreshes += 1; return packages; },
    });
    await test.shell.updateWorkspace(model());
    test.shell.updatePackageSnapshotFailure();
    expect(test.frame.contextualNavigation.innerHTML).toContain("Package information is unavailable");

    test.shell.updatePackages(packages);
    test.shell.updatePackageSnapshotFailure();
    await test.shell.navigate({ kind: "workspace-settings", workspaceId: workspace.id });

    expect(test.frame.contextualNavigation.innerHTML).toContain("Package information may be out of date");
    expect(screen.innerHTML).toContain("Showing the latest available Package information");
    expect(screen.innerHTML).toContain("Retry Package information");

    screen.retrySnapshot.click();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(refreshes).toBe(1);
    expect(test.frame.contextualNavigation.innerHTML).not.toContain("out of date");
    expect(screen.innerHTML).not.toContain("latest available Package information");
  });

  it("keeps lifecycle operation failure in settings and retries the Package that was acted on", async () => {
    const screen = new SettingsRegion();
    const test = harness(screen as unknown as HTMLElement, {
      lifecycle: async () => ({ snapshot: packages, failure: "operation" }),
    });
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(packages);
    await test.shell.navigate({ kind: "package-instance", instance: packageKey });
    await test.shell.navigate({ kind: "workspace-settings", workspaceId: workspace.id });

    screen.recover.click();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.innerHTML).toContain("Could not recover Private One.");
    expect(screen.innerHTML).not.toContain("out of date");

    screen.retryLifecycle.click();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(test.calls.filter((call) => call.startsWith("recover:"))).toEqual([
      `recover:${privatePackageKey.packageIdentity}`,
      `recover:${privatePackageKey.packageIdentity}`,
    ]);
    expect(test.calls).not.toContain(`recover:${packageKey.packageIdentity}`);
  });

  it.each([
    ["refresh", "Recovered Private One, but Package information could not be refreshed."],
    ["operation-and-refresh", "Could not recover Private One, and Package information could not be refreshed."],
  ] as const)("preserves and marks the latest snapshot when lifecycle result is %s", async (failure, wording) => {
    const screen = new SettingsRegion();
    const test = harness(screen as unknown as HTMLElement, {
      lifecycle: async () => ({ snapshot: null, failure }),
    });
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(packages);
    await test.shell.navigate({ kind: "workspace-settings", workspaceId: workspace.id });

    screen.recover.click();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.innerHTML).toContain(wording);
    expect(screen.innerHTML).toContain("Showing the latest available Package information");
    expect(screen.innerHTML).toContain("One");
    expect(screen.innerHTML).toContain("Private One");
  });

  it("renders theme icon buttons that reflect and change the Installation theme preference", async () => {
    const rail = new ThemeRailRegion();
    const test = harness(undefined, {}, rail as unknown as HTMLElement);
    await test.shell.updateWorkspace(model());

    expect(rail.innerHTML).toContain('role="group" aria-label="Theme"');
    expect(rail.innerHTML.match(/data-theme-preference=/g)).toHaveLength(3);
    for (const label of ["System theme", "Light theme", "Dark theme"]) expect(rail.innerHTML).toContain(`aria-label="${label}"`);
    expect(rail.innerHTML).toContain('data-theme-preference="system" aria-pressed="true"');
    expect(rail.innerHTML).toContain('data-theme-preference="dark" aria-pressed="false"');

    rail.dark.click();
    expect(test.calls).toContain("theme:dark");
    expect(rail.focusedSelector).toBe('[data-theme-preference="dark"]');
  });

  it("opens the single Workspace settings screen from the contextual gear control", async () => {
    const nav = new NavRegion();
    const test = harness(undefined, {}, undefined, nav as unknown as HTMLElement);
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(packages);
    await test.shell.navigate({ kind: "package-instance", instance: packageKey });
    expect(nav.innerHTML).not.toMatch(/class="workspace-settings-link"[^>]*aria-current/);

    nav.settings.click();
    await vi.waitFor(() => expect(nav.innerHTML).toMatch(/class="workspace-settings-link"[^>]*aria-current="page"/), { timeout: 5_000 });
    expect(test.shell.route()).toEqual({ kind: "workspace-settings", workspaceId: workspace.id });
    expect(test.frame.shellScreen.innerHTML).toContain("Workspace details");
    expect(test.frame.shellScreen.innerHTML).toContain("Members");
    expect(test.frame.shellScreen.innerHTML).toContain("Workspace Invitations");
    expect(test.frame.shellScreen.innerHTML).toContain("Package Instances");
  });

  it("marks the add Workspace control current only on the Add Workspace screen", async () => {
    const test = harness();
    await test.shell.updateWorkspace(model());
    expect(test.frame.workspaceRail.innerHTML).not.toMatch(/id="choose-workspace"[^>]*aria-current/);

    await test.shell.navigate({ kind: "workspace-chooser" });
    expect(test.frame.workspaceRail.innerHTML).toMatch(/id="choose-workspace"[^>]*aria-current="page"/);
  });

  it("labels contextual navigation as Add a Workspace when no Workspace exists", async () => {
    const test = harness();
    await test.shell.updateWorkspace({ ...model(null), workspaces: [], memberships: {} });
    expect(test.frame.contextualNavigation.innerHTML).toContain("Add a Workspace");
    expect(test.frame.contextualNavigation.innerHTML).not.toContain("Choose a Workspace");
  });

  it.each([
    ["bundled", packageKey, "One"],
    ["local", privatePackageKey, "Private One"],
  ] as const)("uses the same generic navigation path for %s Packages", async (_source, key, name) => {
    const test = harness();
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(packages);
    await test.shell.navigate({ kind: "package-instance", instance: key });
    expect(test.frame.contextualNavigation.innerHTML).toContain(name);
    expect(test.calls).toEqual([`mount:${key.packageIdentity}`]);
  });

  it("binds bridge-error retry feedback to the supported reload action", async () => {
    const screen = new RetryRegion();
    const test = harness(screen as unknown as HTMLElement);
    await test.shell.updateWorkspace({ ...model(), error: "timeout" });
    expect(screen.innerHTML).toContain('id="retry"');
    screen.retry.click();
    await Promise.resolve();
    expect(test.calls).toContain("retry");
  });

  it("keeps protocol data out of normal text, attributes, and accessible labels on every Workspace screen", async () => {
    const test = harness();
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(packages);
    const routes: ShellRoute[] = [
      { kind: "workspace-chooser" },
      { kind: "workspace-settings", workspaceId: workspace.id },
    ];
    for (const route of routes) {
      await test.shell.navigate(route);
      const html = `${test.frame.workspaceRail.innerHTML}${test.frame.contextualNavigation.innerHTML}${test.frame.shellScreen.innerHTML}`;
      for (const forbidden of [
        workspace.id,
        membership.workspaceId,
        membership.version,
        String(workspace.createdAtUnixMs),
        packageKey.packageIdentity,
        "1.0.0-secret-version",
        "2.0.0-secret-version",
        "bundled",
        "local",
        "application_bundled",
        "trusted_native_code_v1",
        "running",
        "active",
        "failed",
        "mounted",
        "414",
        "415",
        "739217",
        "secret-contract-digest",
        "teamarness.package-host",
        "secret-diagnostic-code",
        "secret-diagnostic-stage",
        "secret-diagnostic-recovery",
        "secret-diagnostic-id",
      ]) expect(html).not.toContain(forbidden);
      if (route.kind === "workspace-settings") {
        for (const { identity } of membership.members) {
          expect(html).toContain(`<summary>View identity</summary>Identity: ${identity}`);
        }
      }
    }
  });
});
