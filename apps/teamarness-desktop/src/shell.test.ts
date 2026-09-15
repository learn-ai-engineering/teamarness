import { describe, expect, it } from "vitest";
import fixture from "../contracts/fixtures/bridge-contract.json";
import type { Installation, Membership, Workspace } from "./contracts";
import type { PackageHostSnapshot, PackageInstanceKey } from "./package-contracts";
import { Shell, type ShellFrame, type ShellRoute } from "./shell";
import type { ShellModel } from "./view";

const installation = fixture.installation as Installation;
const workspace = fixture.workspace as Workspace;
const membership = fixture.membership as Membership;
const otherWorkspace: Workspace = { ...workspace, id: "ffeeddccbbaa99887766554433221100", name: "Other" };
const packageKey: PackageInstanceKey = { packageIdentity: "com.example.one", workspaceId: workspace.id };
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
  ],
  instances: [{ key: packageKey, lifecycle: "active", processGeneration: 414, mountState: "mounted" }],
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
  }],
};

class Region {
  public innerHTML = "";
  public hidden = false;
}

class TestControl {
  private listener: EventListener | null = null;

  public addEventListener(_type: string, listener: EventListener): void {
    this.listener = listener;
  }

  public click(): void {
    this.listener?.({} as Event);
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
    activeWorkspaceId,
    loading: false,
    error: null,
    notice: null,
  };
}

function harness(shellScreen: HTMLElement = new Region() as unknown as HTMLElement): {
  shell: Shell;
  frame: ShellFrame;
  calls: string[];
  routes: ShellRoute[];
} {
  const calls: string[] = [];
  const routes: ShellRoute[] = [];
  const frame: ShellFrame = {
    workspaceRail: new Region() as unknown as HTMLElement,
    contextualNavigation: new Region() as unknown as HTMLElement,
    shellScreen,
    packageMount: new Region() as unknown as HTMLElement,
  };
  const shell = new Shell(frame, {
    packageShell: {
      select: async (instance) => { calls.push(`mount:${instance.packageIdentity}`); },
      hide: async () => { calls.push("unmount"); },
    },
    theme: {
      getPreference: () => "system",
      selectPreference: (preference) => { calls.push(`theme:${preference}`); },
    },
    workspaceActions: {
      selectWorkspace: (workspaceId) => { calls.push(`workspace:${workspaceId}`); },
      createWorkspace: async () => undefined,
      joinWorkspace: async () => undefined,
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
    await test.shell.navigate({ kind: "workspace-invitations", workspaceId: workspace.id });
    test.shell.updatePackages({ ...packages, revision: 2 });

    expect(test.shell.frame()).toBe(frame);
    expect(test.shell.frame().packageMount).toBe(mount);
    expect(test.frame.shellScreen.innerHTML).toContain("Invitations");
  });

  it("selects overview at startup and whenever the active Workspace switches", async () => {
    const test = harness();
    await test.shell.updateWorkspace(model());
    expect(test.shell.route()).toEqual({ kind: "workspace-overview", workspaceId: workspace.id });

    await test.shell.updateWorkspace(model(otherWorkspace.id));
    expect(test.shell.route()).toEqual({ kind: "workspace-overview", workspaceId: otherWorkspace.id });
    expect(test.routes).toEqual([
      { kind: "workspace-overview", workspaceId: workspace.id },
      { kind: "workspace-overview", workspaceId: otherWorkspace.id },
    ]);
  });

  it("uses the chooser on first run and exposes discriminated Shell routes", async () => {
    const test = harness();
    await test.shell.updateWorkspace({ ...model(null), workspaces: [], memberships: {} });
    expect(test.shell.route()).toEqual({ kind: "workspace-chooser" });

    const routes: ShellRoute[] = [
      { kind: "workspace-chooser" },
      { kind: "workspace-overview", workspaceId: workspace.id },
      { kind: "workspace-invitations", workspaceId: workspace.id },
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

  it("renders fixed landmarks, grouped Package navigation, aria-current, and the sole theme selector", async () => {
    const test = harness();
    await test.shell.updateWorkspace(model());
    test.shell.updatePackages(packages);
    await test.shell.navigate({ kind: "workspace-overview", workspaceId: workspace.id });

    expect(test.frame.workspaceRail.innerHTML).toContain("Installation");
    expect(test.frame.workspaceRail.innerHTML).toContain('id="theme-preference"');
    expect(test.frame.contextualNavigation.innerHTML).toContain("Team Packages");
    expect(test.frame.contextualNavigation.innerHTML).toContain("Private Packages");
    expect(test.frame.contextualNavigation.innerHTML).toContain('aria-current="page"');
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
      { kind: "workspace-overview", workspaceId: workspace.id },
      { kind: "workspace-invitations", workspaceId: workspace.id },
      { kind: "workspace-settings", workspaceId: workspace.id },
    ];
    for (const route of routes) {
      await test.shell.navigate(route);
      const html = `${test.frame.workspaceRail.innerHTML}${test.frame.contextualNavigation.innerHTML}${test.frame.shellScreen.innerHTML}`;
      for (const forbidden of [
        installation.identity,
        workspace.id,
        workspace.createdBy,
        membership.workspaceId,
        membership.version,
        String(workspace.createdAtUnixMs),
        ...membership.members.map(({ identity }) => identity),
        packageKey.packageIdentity,
        "1.0.0-secret-version",
        "bundled",
        "application_bundled",
        "running",
        "active",
        "mounted",
        "414",
        "739217",
        "secret-contract-digest",
        "teamarness.package-host",
        "secret-diagnostic-code",
        "secret-diagnostic-stage",
        "secret-diagnostic-recovery",
        "secret-diagnostic-id",
      ]) expect(html).not.toContain(forbidden);
    }
  });
});
