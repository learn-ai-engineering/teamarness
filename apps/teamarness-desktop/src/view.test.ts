import { describe, expect, it } from "vitest";
import fixture from "../contracts/fixtures/bridge-contract.json";
import type { TeamarnessBridge } from "./bridge";
import type { BridgeError, Installation, Invitation, Membership, Workspace, WorkspaceState } from "./contracts";
import { buildShellPresentation } from "./presentation";
import type { ShellRoute } from "./shell";
import { ShellController, ShellRenderer, renderShell, type ShellModel } from "./view";

const installation = fixture.installation as Installation;
const workspace = fixture.workspace as Workspace;
const membership = fixture.membership as Membership;
const invitation = fixture.invitation as Invitation;

function fakeBridge(overrides: Partial<TeamarnessBridge> = {}): TeamarnessBridge {
  const state: WorkspaceState = { workspace, membership };
  return {
    installationDetails: async () => installation,
    listWorkspaces: async () => [workspace],
    createWorkspace: async () => state,
    createWorkspaceInvitation: async () => invitation,
    joinWorkspace: async () => state,
    currentMembership: async () => membership,
    ...overrides,
  };
}

function model(change: Partial<ShellModel> = {}): ShellModel {
  return {
    installation,
    workspaces: [workspace],
    memberships: { [workspace.id]: membership },
    activeWorkspaceId: workspace.id,
    loading: false,
    error: null,
    notice: null,
    ...change,
  };
}

async function rendered(source: ShellModel, route: ShellRoute): Promise<string> {
  return renderShell(await buildShellPresentation(source, route.kind === "workspace-chooser" ? null : workspace.id), route);
}

describe("Shell Workspace screens", () => {
  it("updates Workspace controls without replacing the persistent Package pane", async () => {
    const controls = { innerHTML: "" } as HTMLElement;
    const pane = { mountedUi: "mount-1" };
    const renderer = new ShellRenderer(controls, pane);
    await renderer.render(model(), { kind: "workspace-settings", workspaceId: workspace.id });
    await renderer.render(model({ workspaces: [], memberships: {}, activeWorkspaceId: null, notice: "Updated" }));
    expect(renderer.packagePane()).toBe(pane);
    expect(pane.mountedUi).toBe("mount-1");
    expect(controls.innerHTML).toContain("Updated");
    expect(controls.innerHTML).not.toContain("package-pane");
  });

  it("loads the Installation and renders ordered display-safe Member names", async () => {
    const controller = new ShellController(fakeBridge(), () => undefined);
    await controller.load();
    const route: ShellRoute = { kind: "workspace-settings", workspaceId: workspace.id };
    const html = await rendered(controller.snapshot(), route);
    expect(controller.snapshot().installation).toEqual(installation);
    expect(controller.snapshot().activeWorkspaceId).toBe(workspace.id);
    expect(html).toContain(workspace.name);
    const memberList = html.slice(html.indexOf('<ol class="members">'));
    expect(memberList.indexOf("Vivid Whale")).toBeLessThan(memberList.indexOf(installation.displayName));
  });

  it("renders chooser listing, create, join, loading, and no-Workspace states", async () => {
    const chooser: ShellRoute = { kind: "workspace-chooser" };
    const normal = await rendered(model(), chooser);
    expect(normal).toContain(workspace.name);
    expect(normal).toContain('id="create-workspace"');
    expect(normal).toContain('id="join-workspace"');
    expect(normal).toContain('<label for="invitation-value">Workspace Invitation</label>');
    expect(normal.replaceAll("Workspace Invitation", "")).not.toMatch(/\bInvitations?\b/);

    const loading = await rendered(model({ workspaces: [], memberships: {}, activeWorkspaceId: null, loading: true }), chooser);
    expect(loading).toContain("Loading Workspaces…");
    expect(loading).toContain("disabled");

    expect(normal).toContain("Create or join a Workspace");
    expect(normal).toContain("Your Workspaces");
    expect(normal).not.toMatch(/Choose a Workspace/);

    const empty = await rendered(model({ workspaces: [], memberships: {}, activeWorkspaceId: null }), chooser);
    expect(empty).toContain("Create or join a Workspace");
    expect(empty).not.toContain("Your Workspaces");
  });

  it("renders supported Workspace Invitation and read-only settings screens only", async () => {
    const invitations = await rendered(model(), { kind: "workspace-settings", workspaceId: workspace.id });
    expect(invitations).toContain('id="copy-invitation"');
    expect(invitations).toContain("Workspace Invitations are sensitive bearer admission material");
    expect(invitations.replaceAll("Workspace Invitation", "")).not.toMatch(/\bInvitations?\b/);

    const settings = await rendered(model(), { kind: "workspace-settings", workspaceId: workspace.id });
    expect(settings).toContain("Workspace details");
    expect(settings).toContain("Package Instances");
    expect(settings).toContain("read-only");
    expect(settings).not.toMatch(/rename|leave Workspace|revoke|manager administration/i);
  });

  it("creates a Workspace and exposes loading and success feedback", async () => {
    let release: ((value: WorkspaceState) => void) | undefined;
    const pending = new Promise<WorkspaceState>((resolve) => { release = resolve; });
    const updates: boolean[] = [];
    const controller = new ShellController(fakeBridge({ createWorkspace: async () => pending }),
      (nextModel) => updates.push(nextModel.loading));
    const operation = controller.createWorkspace(workspace.name);
    expect(controller.snapshot().loading).toBe(true);
    release?.({ workspace, membership });
    await operation;
    expect(updates).toContain(true);
    expect(controller.snapshot().notice).toBe(`Created ${workspace.name}`);
  });

  it("copies an invitation without rendering its bearer value", async () => {
    const copied: string[] = [];
    const controller = new ShellController(fakeBridge(), () => undefined, async (value) => { copied.push(value); });
    await controller.copyInvitation(workspace.id);
    expect(copied).toEqual([invitation.value]);
    expect(controller.snapshot().notice).toBe("Workspace Invitation copied. Keep it private.");
    expect(await rendered(controller.snapshot(), { kind: "workspace-settings", workspaceId: workspace.id }))
      .not.toContain(invitation.value);
  });

  it("pastes and joins a Workspace", async () => {
    let pasted = "";
    const controller = new ShellController(fakeBridge({
      joinWorkspace: async (value) => { pasted = value; return { workspace, membership }; },
      listWorkspaces: async () => [],
    }), () => undefined);
    await controller.joinWorkspace(invitation.value);
    expect(pasted).toBe(invitation.value);
    expect(controller.snapshot().workspaces).toEqual([workspace]);
    expect(controller.snapshot().notice).toBe(`Joined ${workspace.name}`);
  });

  it("refreshes membership and renders success without identities or versions", async () => {
    const creatorOnly: Membership = { ...membership, epoch: "1", members: [membership.members[1]!] };
    let currentMembership = creatorOnly;
    const controller = new ShellController(fakeBridge({ currentMembership: async () => currentMembership }), () => undefined);
    await controller.load();
    currentMembership = membership;
    await controller.refreshMembership(workspace.id);

    const html = await rendered(controller.snapshot(), { kind: "workspace-settings", workspaceId: workspace.id });
    expect(html).toContain('id="refresh-membership"');
    expect(html).toContain("Membership refreshed");
    expect(controller.snapshot().memberships[workspace.id]?.members).toEqual(membership.members);
    expect(html).not.toContain(membership.version);
    for (const member of membership.members) expect(html).not.toContain(member.identity);
  });

  it("uses Workspace Invitation in every visible onboarding error", async () => {
    const onboardingErrors = ["authentication", "expired", "already_used", "readiness", "local_conflict"] as const;
    for (const code of onboardingErrors) {
      const presentation = await buildShellPresentation(model({ error: code }), workspace.id);
      expect(presentation.error?.message).toContain("Workspace Invitation");
      expect(presentation.error?.message.replaceAll("Workspace Invitation", "")).not.toMatch(/\bInvitation\b/);
    }
  });

  it("accepts Package settings content as an explicit renderer input", async () => {
    const presentation = await buildShellPresentation(model(), workspace.id);
    const html = renderShell(
      presentation,
      { kind: "workspace-settings", workspaceId: workspace.id },
      { packageSettings: '<p class="package-test-state">Package state</p>' },
    );
    expect(html).toContain('<p class="package-test-state">Package state</p>');
    expect(html).not.toContain("No Package management actions are available on this screen yet.");
  });

  it("shows bridge failure and retry states", async () => {
    const failure = fixture.errors.find(({ code }) => code === "timeout") as BridgeError;
    const controller = new ShellController(fakeBridge({ listWorkspaces: async () => { throw failure; } }), () => undefined);
    await controller.load();
    expect(controller.snapshot()).toMatchObject({ loading: false, error: "timeout" });
    const html = await rendered(controller.snapshot(), { kind: "workspace-chooser" });
    expect(html).toContain('role="alert"');
    expect(html).toContain('id="retry"');
    expect(html).not.toContain("timeout");
  });

  it("keeps membership refresh failures observable", async () => {
    const failure = fixture.errors.find(({ code }) => code === "timeout") as BridgeError;
    let shouldFail = false;
    const controller = new ShellController(fakeBridge({
      currentMembership: async () => {
        if (shouldFail) throw failure;
        return membership;
      },
    }), () => undefined);
    await controller.load();
    shouldFail = true;
    await controller.refreshMembership(workspace.id);
    expect(controller.snapshot()).toMatchObject({ loading: false, error: "timeout" });
  });
});

describe("shared bridge fixture", () => {
  it("contains every stable public error and typed success shape", () => {
    expect(fixture.errors.map(({ code }) => code)).toEqual([
      "invalid_input", "authentication", "expired", "already_used", "readiness", "timeout",
      "overload", "invalid_bootstrap", "local_conflict", "admission_no_longer_current",
      "uncertain_storage_outcome", "shutdown",
    ]);
    expect(fixture.workspace.id).toMatch(/^[0-9a-f]{32}$/);
    expect(fixture.membership.version).toMatch(/^[0-9a-f]{64}$/);
  });
});
