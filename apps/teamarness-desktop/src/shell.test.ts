import { describe, expect, it, vi } from "vitest";
import fixture from "../contracts/fixtures/bridge-contract.json";
import type { Installation, Membership, PrincipalsSnapshot, Workspace } from "./contracts";
import type { PackageHostSnapshot, PackageInstanceKey, PackageManagementBridge } from "./package-contracts";
import type { PackageLifecycleResult } from "./package-shell";
import { createInstallationPackageVisibility, type InstallationPackageVisibility } from "./package-visibility";
import { Shell, type ShellFrame, type ShellRoute } from "./shell";
import { STATUS_MESSAGE_HIDE_MS, type StatusMessageTimers } from "./status-message";
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
  registrations: [],
};

class Region {
  public innerHTML = "";
  public hidden = false;
  public readonly attributes = new Map<string, string>();

  public setAttribute(name: string, value: string): void { this.attributes.set(name, value); }
  public removeAttribute(name: string): void { this.attributes.delete(name); }
}

class TestControl {
  public readonly dataset: Record<string, string> = {};
  private listener: EventListener | null = null;

  public addEventListener(_type: string, listener: EventListener): void {
    this.listener = listener;
  }

  public click(): void {
    this.listener?.({ preventDefault: () => undefined } as unknown as Event);
  }

  /** Delivers a key press to a keydown listener. */
  public press(key: string): void {
    this.listener?.({ key, preventDefault: () => undefined } as unknown as Event);
  }
}

/** A clock the test advances by hand, so message timing needs no waiting. */
function manualTimers(): StatusMessageTimers & { advance(ms: number): void } {
  let now = 0;
  let nextHandle = 1;
  const scheduled = new Map<number, Readonly<{ at: number; callback: () => void }>>();
  return {
    setTimeout: (callback, ms) => {
      const handle = nextHandle++;
      scheduled.set(handle, { at: now + ms, callback });
      return handle;
    },
    clearTimeout: (handle) => { scheduled.delete(handle as number); },
    advance: (ms) => {
      now += ms;
      for (const [handle, timer] of [...scheduled]) {
        if (timer.at > now) continue;
        scheduled.delete(handle);
        timer.callback();
      }
    },
  };
}

class SettingsRegion extends Region {
  public readonly recover = new TestControl();
  public readonly dispose = new TestControl();
  public readonly invalidRecover = new TestControl();
  public readonly invalidDispose = new TestControl();
  public readonly suspend = new TestControl();
  public readonly retryLifecycle = new TestControl();
  public readonly retrySnapshot = new TestControl();
  public readonly addAgent = new TestControl();
  public readonly removeAgent = new TestControl();
  public readonly renameAgent = new TestControl();
  public readonly agentName = { value: "Helper" };
  public readonly renamedAgentName = { value: "Helper renamed" };

  public querySelector(selector: string): TestControl | null {
    if (selector === "#add-agent") return this.addAgent;
    if (selector === "#agent-name") return this.agentName as unknown as TestControl;
    if (selector === "[data-package-snapshot-retry]" && this.innerHTML.includes("data-package-snapshot-retry")) {
      return this.retrySnapshot;
    }
    return null;
  }

  public querySelectorAll(selector: string): readonly TestControl[] {
    if (selector === "[data-remove-agent]") {
      this.removeAgent.dataset.removeAgent = "2";
      return this.innerHTML.includes('data-remove-agent="2"') ? [this.removeAgent] : [];
    }
    if (selector === "[data-rename-agent]") {
      this.renameAgent.dataset.renameAgent = "2";
      (this.renameAgent as TestControl & { querySelector: () => unknown }).querySelector = () => this.renamedAgentName;
      return this.innerHTML.includes('data-rename-agent="2"') ? [this.renameAgent] : [];
    }
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
  public readonly refresh = new TestControl();

  public querySelector(selector: string): TestControl | null {
    if (selector === "#retry-membership-refresh" && this.innerHTML.includes('id="retry-membership-refresh"')) return this.refresh;
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
    principals: {},
    activeWorkspaceId,
    loading: false,
    error: null,
    notice: null,
    membershipRefreshUnavailable: null,
  };
}

function unusedPackageManagement(calls: string[] = []): PackageManagementBridge {
  const change = { revision: 1, restartRequired: false };
  return {
    chooseDirectory: async () => { calls.push("choose"); return "/Users/person/Packages/agent"; },
    inspectDirectory: async (directory) => {
      calls.push(`inspect:${directory}`);
      return {
        directory, identity: "com.example.agent", version: "1.0.0", displayName: "Agent",
        capabilities: [], provided: [], consumed: [], kind: "new",
      };
    },
    install: async () => { calls.push("install"); return change; },
    reload: async (identity) => { calls.push(`reload:${identity}`); return change; },
    remove: async (identity) => { calls.push(`remove:${identity}`); return change; },
    writeOverride: async () => { calls.push("override"); return { revision: null, restartRequired: true }; },
    restart: async () => { calls.push("restart"); },
    restartPackage: async (instance) => { calls.push(`restart-package:${instance.packageIdentity}`); },
    authoringCatalog: async () => { calls.push("authoring-catalog"); return []; },
  };
}

function harness(
  shellScreen: HTMLElement = new Region() as unknown as HTMLElement,
  packageBehavior: Readonly<{
    refresh?: () => Promise<PackageHostSnapshot>;
    hide?: () => Promise<void>;
    lifecycle?: (action: "suspend" | "recover" | "dispose", instance: PackageInstanceKey) => Promise<PackageLifecycleResult>;
  }> = {},
  workspaceRail: HTMLElement = new Region() as unknown as HTMLElement,
  contextualNavigation: HTMLElement = new Region() as unknown as HTMLElement,
  agentActions: Readonly<{ addAgent?: (id: string, name: string) => Promise<void>; removeAgent?: (id: string, agentId: string) => Promise<void>; renameAgent?: (id: string, agentId: string, name: string) => Promise<void> }> = {},
  packageVisibility: InstallationPackageVisibility = createInstallationPackageVisibility({ storage: null }),
  packageManagement: PackageManagementBridge = unusedPackageManagement(),
): {
  shell: Shell;
  frame: ShellFrame;
  calls: string[];
  routes: ShellRoute[];
  timers: ReturnType<typeof manualTimers>;
} {
  const calls: string[] = [];
  const routes: ShellRoute[] = [];
  const timers = manualTimers();
  const frame: ShellFrame = {
    workspaceRail,
    contextualNavigation,
    shellScreen,
    packageMount: new Region() as unknown as HTMLElement,
  };
  const shell = new Shell(frame, {
    packageShell: {
      refresh: packageBehavior.refresh ?? (async () => packages),
      select: async (instance, view = "main") => {
        calls.push(view === "main" ? `mount:${instance.packageIdentity}` : `mount:${instance.packageIdentity}:${view}`);
      },
      hide: () => {
        calls.push("unmount");
        return packageBehavior.hide?.() ?? Promise.resolve();
      },
      suspendBackend: async (instance) => {
        calls.push(`suspend:${instance.packageIdentity}`);
        return packageBehavior.lifecycle?.("suspend", instance) ?? { snapshot: packages, failure: null };
      },
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
    packageVisibility,
    packageManagement,
    workspaceActions: {
      selectWorkspace: (workspaceId) => { calls.push(`workspace:${workspaceId}`); },
      createWorkspace: async () => undefined,
      joinWorkspace: async () => undefined,
      rename: async () => undefined,
      addAgent: agentActions.addAgent ?? (async () => undefined),
      removeAgent: agentActions.removeAgent ?? (async () => undefined),
      renameAgent: agentActions.renameAgent ?? (async () => undefined),
      copyInvitation: async () => undefined,
      refreshMembership: async (id) => { calls.push(`refresh:${id}`); },
      retry: async () => { calls.push("retry"); },
    },
    onRouteChange: (route) => { routes.push(route); },
    timers,
  });
  return { shell, frame, calls, routes, timers };
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

  it("renders fixed landmarks, active-Workspace Package groups as name and status, and source markers only in settings", async () => {
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
    // Each sidebar row is two lines: the name, then the status.
    expect(test.frame.contextualNavigation.innerHTML).toContain('<li><button type="button" data-package-index="0"><span>One</span><small>Available</small></button></li>');
    expect(test.frame.contextualNavigation.innerHTML).toContain('<li><button type="button" data-package-index="1"><span>Private One</span><small>Needs recovery</small></button></li>');
    for (const removed of ["Package source", "Runs only on this Installation", "package-about", "About ", "data-package-page-index", "package-nav-row"]) {
      expect(test.frame.contextualNavigation.innerHTML).not.toContain(removed);
    }
    expect(test.frame.contextualNavigation.innerHTML).not.toContain("Other");
    expect(test.frame.contextualNavigation.innerHTML).toContain('aria-current="page"');
    // Workspace settings rows keep their source and Installation markers.
    expect(test.frame.shellScreen.innerHTML).toContain("Package source: Team");
    expect(test.frame.shellScreen.innerHTML).toContain("Package source: Personal");
    expect(test.frame.shellScreen.innerHTML).toContain("Runs only on this Installation");
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
    // The result shows in the section's message area, not in the Package's row.
    expect(screen.innerHTML).toContain('<div class="status-message status-message--error" role="alert" aria-live="assertive"><p>Private One: Could not recover.</p><button type="button" data-package-failure-action="recover">Retry Recover Backend</button>');
    expect(screen.innerHTML).not.toContain("package-lifecycle-feedback");
    expect(screen.innerHTML).not.toContain("out of date");

    screen.retryLifecycle.click();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(test.calls.filter((call) => call.startsWith("recover:"))).toEqual([
      `recover:${privatePackageKey.packageIdentity}`,
      `recover:${privatePackageKey.packageIdentity}`,
    ]);
    expect(test.calls).not.toContain(`recover:${packageKey.packageIdentity}`);
  });

  it("shows Recover and Dispose results in the message area, and the next action clears the last one", async () => {
    const screen = new SettingsRegion();
    const test = harness(screen as unknown as HTMLElement, {
      lifecycle: async (action) => ({ snapshot: packages, failure: action === "dispose" ? "operation" : null }),
    });
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(packages);
    await test.shell.navigate({ kind: "workspace-settings", workspaceId: workspace.id });

    screen.dispose.click();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.innerHTML).toContain("<p>Private One: Could not dispose.</p>");
    expect(screen.innerHTML).toContain(">Retry Dispose</button>");

    // An error stays until the next action, which replaces it with its own result.
    screen.recover.click();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.innerHTML).not.toContain("Could not dispose");
    expect(screen.innerHTML).not.toContain("data-package-failure-action");
    expect(screen.innerHTML).toContain('<div class="status-message status-message--success" role="status" aria-live="polite"><p>Private One: Recovered.</p></div>');

    test.timers.advance(5_000);
    expect(screen.innerHTML).not.toContain("Private One: Recovered.");
  });

  it("replaces a refresh failure message with the result once Package information refreshes", async () => {
    const screen = new SettingsRegion();
    const test = harness(screen as unknown as HTMLElement, {
      lifecycle: async () => ({ snapshot: null, failure: "operation-and-refresh" }),
    });
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(packages);
    await test.shell.navigate({ kind: "workspace-settings", workspaceId: workspace.id });

    screen.recover.click();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.innerHTML).toContain("Private One: Could not recover, and Package information could not be refreshed.");

    test.shell.updatePackages({ ...packages, revision: 2 });
    expect(screen.innerHTML).toContain("<p>Private One: Could not recover.</p>");
    expect(screen.innerHTML).toContain(">Retry Recover Backend</button>");
  });

  it.each([
    ["refresh", "Private One: Recovered, but Package information could not be refreshed."],
    ["operation-and-refresh", "Private One: Could not recover, and Package information could not be refreshed."],
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

  it("binds the post-mutation warning to membership refresh, not the add action", async () => {
    const screen = new RetryRegion();
    const test = harness(screen as unknown as HTMLElement, {}, undefined, undefined, {
      addAgent: async () => { test.calls.push("add"); },
    });
    await test.shell.updateWorkspace({ ...model(), membershipRefreshUnavailable: workspace.id, notice: "Agent added" });
    await test.shell.navigate({ kind: "workspace-settings", workspaceId: workspace.id });
    expect(screen.innerHTML).toContain('id="retry-membership-refresh"');
    screen.refresh.click();
    await Promise.resolve();
    expect(test.calls).toContain(`refresh:${workspace.id}`);
    expect(test.calls).not.toContain("add");
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


describe("Workspace Agent Shell controls", () => {
  const agentId = "e".repeat(64);
  const agentIdentity = "f".repeat(64);
  const agent = { identity: agentIdentity, role: "member" as const, principalId: agentId, kind: "agent" as const, stewardPrincipalId: "d".repeat(64) };
  const principal = { principalId: agentId, kind: "agent" as const, stewardPrincipalId: "d".repeat(64), keys: [agentIdentity], name: "Helper", fingerprint: agentIdentity.slice(0, 12) };

  it("routes name-only add, ID-only removal and steward rename through settings", async () => {
    const screen = new SettingsRegion();
    const calls: string[] = [];
    const test = harness(screen as unknown as HTMLElement, {}, undefined, undefined, {
      addAgent: async (id, name) => { calls.push(`add:${id}:${name}`); },
      removeAgent: async (id, agentId) => { calls.push(`remove:${id}:${agentId}`); },
      renameAgent: async (id, agentId, name) => { calls.push(`rename:${id}:${agentId}:${name}`); },
    });
    await test.shell.updateWorkspace({ ...model(), memberships: { [workspace.id]: { ...membership, members: [...membership.members, agent] } }, principals: { [workspace.id]: { ...fixture.principals, principals: [...fixture.principals.principals, principal] } as PrincipalsSnapshot } });
    screen.addAgent.click();
    screen.removeAgent.click();
    screen.renameAgent.click();
    expect(calls).toEqual([
      `add:${workspace.id}:Helper`,
      `remove:${workspace.id}:${agentId}`,
      `rename:${workspace.id}:${agentId}:Helper renamed`,
    ]);
  });
});

class VisibilityRegion extends Region {
  public readonly toggle = new TestControl();
  public focusedSelector: string | null = null;

  public querySelector(selector: string): (TestControl & { focus(): void }) | null {
    if (!selector.startsWith("[data-package-visibility-index=")) return null;
    return Object.assign(new TestControl(), { focus: () => { this.focusedSelector = selector; } });
  }

  public querySelectorAll(selector: string): readonly TestControl[] {
    if (selector !== "[data-package-visibility-index]") return [];
    this.toggle.dataset.packageVisibilityIndex = "0";
    return [this.toggle];
  }
}

describe("Installation Package visibility", () => {
  it("omits hidden Package Instances from navigation but keeps them in settings", async () => {
    const visibility = createInstallationPackageVisibility({ storage: null });
    visibility.setHidden(packageKey, true);
    const test = harness(undefined, {}, undefined, undefined, {}, visibility);
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(packages);
    await test.shell.navigate({ kind: "workspace-settings", workspaceId: workspace.id });

    expect(test.frame.contextualNavigation.innerHTML).not.toContain("<span>One</span>");
    expect(test.frame.contextualNavigation.innerHTML).toContain("<span>Private One</span>");
    expect(test.frame.shellScreen.innerHTML).toContain('aria-label="Show One in navigation"');
    expect(test.frame.shellScreen.innerHTML).toContain('aria-label="Hide Private One in navigation"');
  });

  it("toggles a Package Instance from settings and keeps focus on the toggle", async () => {
    const screen = new VisibilityRegion();
    const test = harness(screen as unknown as HTMLElement);
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(packages);
    await test.shell.navigate({ kind: "workspace-settings", workspaceId: workspace.id });
    expect(test.frame.contextualNavigation.innerHTML).toContain("<span>One</span>");

    screen.toggle.click();
    expect(test.frame.contextualNavigation.innerHTML).not.toContain("<span>One</span>");
    expect(screen.innerHTML).toContain('aria-label="Show One in navigation"');
    expect(screen.focusedSelector).toBe('[data-package-visibility-index="0"]');

    screen.toggle.click();
    expect(test.frame.contextualNavigation.innerHTML).toContain("<span>One</span>");
  });
});

class ManagementRegion extends Region {
  public focusedSelector: string | null = null;
  private readonly controls = new Map<string, TestControl>();

  public control(key: string): TestControl {
    let control = this.controls.get(key);
    if (!control) {
      control = new TestControl();
      this.controls.set(key, control);
    }
    return control;
  }

  public querySelector(selector: string): (TestControl & { focus(): void }) | null {
    if (selector === "[data-package-install-focus]" || selector === '[data-package-install-action="open"]') {
      return Object.assign(new TestControl(), { focus: () => { this.focusedSelector = selector; } });
    }
    return null;
  }

  public querySelectorAll(selector: string): readonly TestControl[] {
    if (selector === "[data-package-install-action]") {
      return [...this.innerHTML.matchAll(/data-package-install-action="([a-z-]+)"/g)].map(([, action]) => {
        const control = this.control(`install:${action}`);
        control.dataset.packageInstallAction = action!;
        return control;
      });
    }
    if (selector === "[data-package-registration-action]") {
      return [...this.innerHTML.matchAll(/data-package-registration-action="([a-z-]+)" data-package-registration-index="(\d+)"/g)].map(([, action, index]) => {
        const control = this.control(`registration:${action}:${index}`);
        control.dataset.packageRegistrationAction = action!;
        control.dataset.packageRegistrationIndex = index!;
        return control;
      });
    }
    return [];
  }
}

describe("Add and manage Personal Packages from the Workspace page", () => {
  const refusedRegistration = {
    directory: "/Users/person/Packages/broken",
    expectedIdentity: "com.example.broken",
    override: false,
    tier: "personal" as const,
    status: { kind: "refused" as const, code: "artifact_changed" },
  };

  it("puts an Add Personal Package button in the Packages section heading", async () => {
    const test = harness();
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(packages);
    await test.shell.navigate({ kind: "workspace-settings", workspaceId: workspace.id });
    const html = test.frame.shellScreen.innerHTML;
    expect(html).toMatch(/<h2 id="package-instances-heading">Package Instances<\/h2>\s*<button type="button" class="package-add" data-package-install-action="open" aria-label="Add Personal Package"/);
  });

  it("opens the add flow from +, previews, and installs only after Trust and install", async () => {
    const screen = new ManagementRegion();
    const calls: string[] = [];
    const test = harness(screen as unknown as HTMLElement, {}, undefined, undefined, {}, undefined, unusedPackageManagement(calls));
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(packages);
    await test.shell.navigate({ kind: "workspace-settings", workspaceId: workspace.id });

    screen.control("install:open").click();
    await vi.waitFor(() => expect(screen.innerHTML).toContain("Choose folder"));
    expect(screen.focusedSelector).toBe("[data-package-install-focus]");
    screen.control("install:choose").click();
    await vi.waitFor(() => expect(screen.innerHTML).toContain("Preview: Agent"));
    screen.control("install:continue").click();
    await vi.waitFor(() => expect(screen.innerHTML).toContain("Trust and install"));
    expect(calls).toEqual(["choose", "inspect:/Users/person/Packages/agent"]);

    screen.control("install:trust").click();
    await vi.waitFor(() => expect(screen.innerHTML).toContain("Installed on this device. It appears in all your Workspaces."));
    expect(calls.at(-1)).toBe("install");
  });

  it("lists refused Personal registrations with Reload and Remove, and Remove keeps data", async () => {
    const screen = new ManagementRegion();
    const calls: string[] = [];
    const withRefused = { ...packages, registrations: [refusedRegistration] };
    const test = harness(screen as unknown as HTMLElement, { refresh: async () => withRefused }, undefined, undefined, {}, undefined, unusedPackageManagement(calls));
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(withRefused);
    await test.shell.navigate({ kind: "workspace-settings", workspaceId: workspace.id });

    expect(screen.innerHTML).toContain("com.example.broken");
    expect(screen.innerHTML).toContain("A file changed after the manifest was written.");
    expect(screen.innerHTML).toContain('aria-label="Reload com.example.broken"');
    expect(screen.innerHTML).toContain('aria-label="Remove com.example.broken"');

    screen.control("registration:reload:0").click();
    await vi.waitFor(() => expect(screen.innerHTML).toContain("Reloaded."));
    expect(calls).toEqual(["reload:com.example.broken"]);

    screen.control("registration:remove:0").click();
    await vi.waitFor(() => expect(screen.innerHTML).toContain("Its data stays on this device"));
    screen.control("registration:confirm-remove:0").click();
    await vi.waitFor(() => expect(calls.at(-1)).toBe("remove:com.example.broken"));
  });
});

/** A region whose controls are found by their data attributes in the rendered HTML. */
class AttributeRegion extends Region {
  public focusedSelector: string | null = null;
  private readonly controls = new Map<string, TestControl>();

  public control(attribute: string, value: string): TestControl {
    const key = `${attribute}=${value}`;
    let control = this.controls.get(key);
    if (!control) {
      control = new TestControl();
      control.dataset[camel(attribute)] = value;
      this.controls.set(key, control);
    }
    return control;
  }

  public querySelector(selector: string): (TestControl & { focus(): void }) | null {
    const valued = /^\[(data-[a-z-]+)="([^"]*)"\]$/.exec(selector);
    const found = valued
      ? this.querySelectorAll(`[${valued[1]}]`).find((control) => control.dataset[camel(valued[1]!)] === valued[2])
      : this.querySelectorAll(selector)[0];
    if (!found) return null;
    return Object.assign(found, { focus: () => { this.focusedSelector = selector; } });
  }

  public querySelectorAll(selector: string): readonly TestControl[] {
    const attribute = /^\[(data-[a-z-]+)\]$/.exec(selector)?.[1];
    if (!attribute) return [];
    return [...this.innerHTML.matchAll(new RegExp(`${attribute}(?:="([^"]*)")?[ >]`, "g"))]
      .map(([, value]) => this.control(attribute, value ?? ""));
  }
}

function camel(attribute: string): string {
  return attribute.replace(/^data-/, "").replace(/-([a-z])/g, (_, letter: string) => letter.toUpperCase());
}

describe("Package page route", () => {
  const personal = {
    ...packages,
    instances: [packages.instances[0]!, { ...packages.instances[1]!, lifecycle: "active" as const }],
    registrations: [{
      directory: "/Users/person/Packages/private",
      expectedIdentity: privatePackageKey.packageIdentity,
      override: false,
      tier: "personal" as const,
      status: { kind: "admitted" as const },
    }],
  };

  it("opens from a Package row on the Workspace page, with no back control", async () => {
    const screen = new AttributeRegion();
    const calls: string[] = [];
    const test = harness(screen as unknown as HTMLElement, { refresh: async () => personal }, undefined, undefined, {}, undefined, unusedPackageManagement(calls));
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(personal);
    await test.shell.navigate({ kind: "workspace-settings", workspaceId: workspace.id });
    expect(screen.innerHTML).toContain('data-package-page-index="1" aria-label="About Private One">Private One</button></h3>');

    screen.control("data-package-page-index", "1").click();
    await vi.waitFor(() => expect(test.shell.route()).toEqual({
      kind: "package-page", workspaceId: workspace.id, packageIdentity: privatePackageKey.packageIdentity,
    }));
    await vi.waitFor(() => expect(screen.innerHTML).toContain("<dt>Package folder</dt><dd>/Users/person/Packages/private</dd>"));
    expect(screen.focusedSelector).toBe("[data-package-page-focus]");
    expect(calls).toContain("authoring-catalog");
    expect(screen.innerHTML).toContain('data-package-page-action="restart"');

    screen.control("data-package-page-action", "restart").click();
    await vi.waitFor(() => expect(screen.innerHTML).toContain("Restarted Private One in this Workspace."));
    expect(calls).toContain(`restart-package:${privatePackageKey.packageIdentity}`);
    screen.control("data-package-page-action", "pause").click();
    await vi.waitFor(() => expect(test.calls).toContain(`suspend:${privatePackageKey.packageIdentity}`));

    expect(screen.innerHTML).not.toContain("data-package-page-back");
    expect(screen.innerHTML).not.toContain("Back to");
  });

  it("keeps the sidebar opening the Package UI, with no About button", async () => {
    const nav = new AttributeRegion();
    const test = harness(new AttributeRegion() as unknown as HTMLElement, {}, undefined, nav as unknown as HTMLElement);
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(packages);
    expect(nav.querySelectorAll("[data-package-page-index]")).toEqual([]);

    nav.control("data-package-index", "0").click();
    await vi.waitFor(() => expect(test.calls).toContain("mount:com.example.one"));
    expect(test.shell.route()).toEqual({ kind: "package-instance", instance: packageKey });
    expect(test.frame.shellScreen.hidden).toBe(true);
    expect(test.frame.packageMount.hidden).toBe(false);
  });

  it("returns to the Workspace page after Remove, which says the data is kept", async () => {
    const screen = new AttributeRegion();
    const calls: string[] = [];
    let current: PackageHostSnapshot = personal;
    const management = unusedPackageManagement(calls);
    const test = harness(screen as unknown as HTMLElement, { refresh: async () => current }, undefined, undefined, {}, undefined, {
      ...management,
      remove: async (identity) => {
        calls.push(`remove:${identity}`);
        current = { ...packages, instances: [packages.instances[0]!], registrations: [] };
        return { revision: 2, restartRequired: false };
      },
    });
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(personal);
    await test.shell.navigate({ kind: "package-page", workspaceId: workspace.id, packageIdentity: privatePackageKey.packageIdentity });

    screen.control("data-package-page-action", "remove").click();
    await vi.waitFor(() => expect(screen.innerHTML).toContain("Its data stays on this device"));
    screen.control("data-package-page-action", "confirm-remove").click();
    await vi.waitFor(() => expect(test.shell.route()).toEqual({ kind: "workspace-settings", workspaceId: workspace.id }));
    expect(calls).toContain(`remove:${privatePackageKey.packageIdentity}`);
    await vi.waitFor(() => expect(screen.innerHTML).toContain("Removed. Its data stays on this device"));
  });

  it("opens a refused registration's page from its row", async () => {
    const screen = new AttributeRegion();
    const refused = {
      ...packages,
      registrations: [{
        directory: "/Users/person/Packages/broken",
        expectedIdentity: "com.example.broken",
        override: false,
        tier: "personal" as const,
        status: { kind: "refused" as const, code: "artifact_changed" },
      }],
    };
    const test = harness(screen as unknown as HTMLElement, { refresh: async () => refused });
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(refused);
    await test.shell.navigate({ kind: "workspace-settings", workspaceId: workspace.id });

    screen.control("data-package-registration-page", "0").click();
    await vi.waitFor(() => expect(screen.innerHTML).toContain("It could not start: A file changed after the manifest was written."));
    expect(test.shell.route()).toEqual({ kind: "package-page", workspaceId: workspace.id, packageIdentity: "com.example.broken" });
  });
});

describe("Package page Configuration tab", () => {
  const declaring: PackageHostSnapshot = {
    ...packages,
    packages: packages.packages.map((entry) => entry.identity === packageKey.packageIdentity ? { ...entry, views: ["settings"] } : entry),
  };
  const page: ShellRoute = { kind: "package-page", workspaceId: workspace.id, packageIdentity: packageKey.packageIdentity };
  const configuration: ShellRoute = { ...page, tab: "configuration" };
  const mountAttributes = (test: ReturnType<typeof harness>): Map<string, string> =>
    (test.frame.packageMount as unknown as Region).attributes;

  it("mounts the settings view below the page header and tabs, and unmounts it for About", async () => {
    const screen = new AttributeRegion();
    const test = harness(screen as unknown as HTMLElement, { refresh: async () => declaring });
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(declaring);
    await test.shell.navigate(page);
    await vi.waitFor(() => expect(screen.innerHTML).toContain(">Details</h2>"));
    expect(screen.innerHTML).toContain('data-package-page-tab="configuration">Configuration</button>');
    expect(test.frame.packageMount.hidden).toBe(true);

    screen.control("data-package-page-tab", "configuration").click();
    await vi.waitFor(() => expect(test.shell.route()).toEqual(configuration));
    await vi.waitFor(() => expect(test.calls).toContain("mount:com.example.one:settings"));
    await vi.waitFor(() => expect(screen.focusedSelector).toBe('[data-package-page-tab="configuration"]'));
    // The page header and tabs stay; the settings view is the tab panel below them.
    expect(test.frame.shellScreen.hidden).toBe(false);
    expect(test.frame.packageMount.hidden).toBe(false);
    expect(screen.innerHTML).toContain('aria-selected="true" tabindex="0" data-package-page-tab="configuration"');
    expect(screen.innerHTML).toContain('data-package-page-focus>One</h1>');
    expect(screen.innerHTML).not.toContain(">Details</h2>");
    expect(screen.innerHTML).not.toContain("Back to");
    expect(mountAttributes(test).get("data-placement")).toBe("package-page");
    expect(mountAttributes(test).get("role")).toBe("tabpanel");
    expect(mountAttributes(test).get("aria-labelledby")).toBe("package-page-tab-configuration");
    expect(screen.focusedSelector).toBe('[data-package-page-tab="configuration"]');

    screen.control("data-package-page-tab", "about").click();
    await vi.waitFor(() => expect(screen.innerHTML).toContain(">Details</h2>"));
    expect(test.shell.route()).toEqual(page);
    expect(test.calls.at(-1)).toBe("unmount");
    expect(test.frame.packageMount.hidden).toBe(true);
    expect(mountAttributes(test).get("data-placement")).toBeUndefined();
    expect(mountAttributes(test).get("aria-label")).toBe("Package interface");
    expect(screen.innerHTML).toContain(">Details</h2>");

    // Switching back mounts the settings view again.
    screen.control("data-package-page-tab", "configuration").click();
    await vi.waitFor(() => expect(test.calls.filter((call) => call === "mount:com.example.one:settings")).toHaveLength(2));
  });

  it("keeps the settings view when About is selected and Configuration again before the unmount finishes", async () => {
    let finishUnmount: () => void = () => undefined;
    let deferring = false;
    const screen = new AttributeRegion();
    const test = harness(screen as unknown as HTMLElement, {
      refresh: async () => declaring,
      hide: () => deferring ? new Promise<void>((resolve) => { finishUnmount = resolve; }) : Promise.resolve(),
    });
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(declaring);
    await test.shell.navigate(configuration);
    expect(test.frame.packageMount.hidden).toBe(false);

    deferring = true;
    const about = test.shell.navigate(page);
    deferring = false;
    await test.shell.navigate(configuration);
    finishUnmount();
    await about;

    expect(test.shell.route()).toEqual(configuration);
    expect(test.frame.packageMount.hidden).toBe(false);
    expect(test.frame.shellScreen.hidden).toBe(false);
    expect(mountAttributes(test).get("data-placement")).toBe("package-page");
  });

  it("leaves Configuration for About when the unmount fails", async () => {
    let failing = false;
    const screen = new AttributeRegion();
    const test = harness(screen as unknown as HTMLElement, {
      refresh: async () => declaring,
      hide: async () => { if (failing) throw new Error("unmount was not acknowledged"); },
    });
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(declaring);
    await test.shell.navigate(configuration);

    failing = true;
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    try {
      await expect(test.shell.navigate(page)).resolves.toBeUndefined();
      // The failure is not swallowed silently: its reason is logged, without a payload.
      expect(warn).toHaveBeenCalledWith("Package UI unmount cleanup failed", { reason: "unmount was not acknowledged" });
    } finally {
      warn.mockRestore();
    }
    expect(test.shell.route()).toEqual(page);
    expect(test.frame.packageMount.hidden).toBe(true);
    expect(test.frame.shellScreen.hidden).toBe(false);
    expect(mountAttributes(test).get("data-placement")).toBeUndefined();
    await vi.waitFor(() => expect(screen.innerHTML).toContain(">Details</h2>"));
  });

  it("moves between tabs with the arrow keys", async () => {
    const screen = new AttributeRegion();
    const test = harness(screen as unknown as HTMLElement, { refresh: async () => declaring });
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(declaring);
    await test.shell.navigate(page);

    screen.control("data-package-page-tabs", "").press("ArrowRight");
    await vi.waitFor(() => expect(test.shell.route()).toEqual(configuration));
    screen.control("data-package-page-tabs", "").press("Home");
    await vi.waitFor(() => expect(test.shell.route()).toEqual(page));
    await vi.waitFor(() => expect(screen.focusedSelector).toBe('[data-package-page-tab="about"]'));
  });

  it("unmounts the settings view when the person leaves the page", async () => {
    const test = harness(new AttributeRegion() as unknown as HTMLElement, { refresh: async () => declaring });
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(declaring);
    await test.shell.navigate(configuration);
    expect(test.calls).toEqual(["mount:com.example.one:settings"]);

    await test.shell.navigate({ kind: "workspace-settings", workspaceId: workspace.id });
    expect(test.calls.at(-1)).toBe("unmount");
    expect(test.frame.packageMount.hidden).toBe(true);

    // The sidebar still opens the main view, in the whole pane.
    await test.shell.navigate(configuration);
    await test.shell.navigate({ kind: "package-instance", instance: packageKey });
    expect(test.calls.at(-1)).toBe("mount:com.example.one");
    expect(mountAttributes(test).get("data-placement")).toBeUndefined();
    expect(test.frame.shellScreen.hidden).toBe(true);
  });

  it("unmounts the settings view when the active Workspace switches", async () => {
    const test = harness(new AttributeRegion() as unknown as HTMLElement, { refresh: async () => declaring });
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(declaring);
    await test.shell.navigate(configuration);
    await test.shell.updateWorkspace(model(otherWorkspace.id));
    expect(test.calls.at(-1)).toBe("unmount");
    expect(test.frame.packageMount.hidden).toBe(true);
  });

  it("offers no Configuration and mounts nothing for a Package that declares no settings view", async () => {
    const screen = new AttributeRegion();
    const test = harness(screen as unknown as HTMLElement);
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(packages);
    await test.shell.navigate(configuration);
    expect(test.shell.route()).toEqual(page);
    await vi.waitFor(() => expect(screen.innerHTML).toContain("Capabilities and Ports"));
    expect(screen.innerHTML).not.toContain("Configuration");
    expect(test.calls).not.toContain("mount:com.example.one:settings");
    expect(test.frame.packageMount.hidden).toBe(true);
  });

  it("closes Configuration when the Instance leaves this Workspace", async () => {
    const test = harness(new AttributeRegion() as unknown as HTMLElement, { refresh: async () => declaring });
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(declaring);
    await test.shell.navigate(configuration);
    test.shell.updatePackages({
      ...declaring,
      revision: declaring.revision + 1,
      instances: declaring.instances.filter(({ key }) => key.packageIdentity !== packageKey.packageIdentity || key.workspaceId !== workspace.id),
    });
    await vi.waitFor(() => expect(test.shell.route()).toEqual(page));
    expect(test.calls.at(-1)).toBe("unmount");
    await vi.waitFor(() => expect(test.frame.packageMount.hidden).toBe(true));
  });
});

describe("Packages section messages on the Workspace page", () => {
  const refused = {
    ...packages,
    registrations: [{
      directory: "/Users/person/Packages/broken",
      expectedIdentity: "com.example.broken",
      override: false,
      tier: "personal" as const,
      status: { kind: "refused" as const, code: "artifact_changed" },
    }],
  };

  it("reserves the message area, shows a result at the top, and hides a success after five seconds", async () => {
    const screen = new ManagementRegion();
    const test = harness(screen as unknown as HTMLElement, { refresh: async () => refused });
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(refused);
    await test.shell.navigate({ kind: "workspace-settings", workspaceId: workspace.id });
    const empty = '<div class="status-message-slot"><div class="status-message" role="status" aria-live="polite"></div></div>';
    expect(screen.innerHTML).toContain(empty);
    expect(screen.innerHTML.indexOf(empty)).toBeLessThan(screen.innerHTML.indexOf("package-instance-list"));

    screen.control("registration:reload:0").click();
    await vi.waitFor(() => expect(screen.innerHTML).toContain("<p>com.example.broken: Reloaded.</p>"));
    test.timers.advance(STATUS_MESSAGE_HIDE_MS);
    expect(screen.innerHTML).not.toContain("Reloaded.");
    expect(screen.innerHTML).toContain(empty);
  });

  it("keeps an error until the person dismisses it", async () => {
    const screen = new AttributeRegion();
    const test = harness(screen as unknown as HTMLElement, { refresh: async () => refused }, undefined, undefined, {}, undefined, {
      ...unusedPackageManagement(),
      reload: async () => { throw { code: "artifact_missing" }; },
    });
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(refused);
    await test.shell.navigate({ kind: "workspace-settings", workspaceId: workspace.id });

    screen.control("data-package-registration-action", "reload").dataset.packageRegistrationIndex = "0";
    screen.control("data-package-registration-action", "reload").click();
    await vi.waitFor(() => expect(screen.innerHTML).toContain('role="alert" aria-live="assertive"><p>com.example.broken: A file the manifest lists is missing.'));
    test.timers.advance(STATUS_MESSAGE_HIDE_MS * 10);
    expect(screen.innerHTML).toContain("A file the manifest lists is missing.");

    screen.control("data-package-message-dismiss", "").click();
    expect(screen.innerHTML).not.toContain("A file the manifest lists is missing.");
  });
});
