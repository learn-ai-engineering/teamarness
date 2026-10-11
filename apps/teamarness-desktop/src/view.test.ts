import { describe, expect, it } from "vitest";
import fixture from "../contracts/fixtures/bridge-contract.json";
import type { TeamarnessBridge } from "./bridge";
import type { BridgeError, Installation, Invitation, Membership, PersonNameSnapshot, Workspace, WorkspaceState } from "./contracts";
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
    setOwnPersonName: async () => undefined,
    personNameSnapshot: async (workspaceId): Promise<PersonNameSnapshot> => ({ workspaceId, cursor: "0", persons: [] }),
    subscribePersonNames: async () => undefined,
    ...overrides,
  };
}

function model(change: Partial<ShellModel> = {}): ShellModel {
  return {
    installation,
    workspaces: [workspace],
    memberships: { [workspace.id]: membership },
    personNames: {},
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
  it("uses Workspace-specific Unicode names and a distinct profile cursor for live rename", async () => {
    const createdNames: string[] = [];
    const workspaceB = { ...workspace, id: "f".repeat(32), name: "Second" };
    const controller = new ShellController(fakeBridge({
      createWorkspace: async (_workspaceName, personName) => {
        createdNames.push(personName);
        return { workspace: workspaceB, membership: { ...membership, workspaceId: workspaceB.id } };
      },
    }), () => undefined);
    await controller.load();
    await controller.createWorkspace("Second", "👩‍💻");
    expect(createdNames).toEqual(["👩‍💻"]);
    expect(controller.snapshot().personNames[workspaceB.id]?.workspaceId).toBe(workspaceB.id);
    expect(controller.snapshot().personNames[workspace.id]?.persons.find(({ identity }) => identity === installation.identity)?.name).not.toBe("👩‍💻");
  });

  it("subscribes to changed names, scopes updates and escapes name text", async () => {
    let listener: ((snapshot: PersonNameSnapshot) => void) | undefined;
    const controller = new ShellController(fakeBridge({
      personNameSnapshot: async (workspaceId) => ({ workspaceId, cursor: "0", persons: [] }),
      subscribePersonNames: async (_workspaceId, _cursor, next) => { listener = next; },
    }), () => undefined);
    await controller.load();
    listener?.({ workspaceId: workspace.id, cursor: "1", persons: [
      { identity: membership.members[0]!.identity, personId: "c".repeat(64), fingerprint: "deadbeef", name: "👩‍💻<script>" },
    ] });
    const html = await rendered(controller.snapshot(), { kind: "workspace-settings", workspaceId: workspace.id });
    expect(html).toContain("👩‍💻&lt;script&gt;");
    expect(html).not.toContain("<script>");
    const self = { identity: installation.identity, personId: "d".repeat(64), fingerprint: "abcdef12", name: 'Alice "<script>' };
    listener?.({ workspaceId: workspace.id, cursor: "2", persons: [self] });
    const renamed = await rendered(controller.snapshot(), { kind: "workspace-settings", workspaceId: workspace.id });
    expect(renamed).toContain('value="Alice &quot;&lt;script&gt;"');
    expect(controller.snapshot().personNames[workspace.id]?.cursor).toBe("2");
    listener?.({ workspaceId: workspace.id, cursor: "0", persons: [] });
    expect(controller.snapshot().personNames[workspace.id]?.cursor).toBe("2");
  });

  it("keeps a live rename when a delayed refresh or load returns an older snapshot", async () => {
    let listener: ((snapshot: PersonNameSnapshot) => void) | undefined;
    let releaseSnapshot: ((snapshot: PersonNameSnapshot) => void) | undefined;
    const pending = () => new Promise<PersonNameSnapshot>((resolve) => { releaseSnapshot = resolve; });
    let nextSnapshot: Promise<PersonNameSnapshot> | undefined;
    const controller = new ShellController(fakeBridge({
      personNameSnapshot: async (id) => nextSnapshot ?? { workspaceId: id, cursor: "1", persons: [] },
      subscribePersonNames: async (_id, _cursor, next) => { listener = next; },
    }), () => undefined);
    await controller.load();
    const newer: PersonNameSnapshot = { workspaceId: workspace.id, cursor: "2", persons: [
      { identity: installation.identity, personId: "c".repeat(64), fingerprint: "deadbeef", name: "Renamed 👩‍💻" },
    ] };
    for (const operation of [() => controller.refreshMembership(workspace.id), () => controller.load()]) {
      nextSnapshot = pending();
      const inFlight = operation();
      await Promise.resolve();
      listener?.(newer);
      releaseSnapshot?.({ workspaceId: workspace.id, cursor: "1", persons: [] });
      await inFlight;
      expect(controller.snapshot().personNames[workspace.id]).toEqual(newer);
      expect(await rendered(controller.snapshot(), { kind: "workspace-settings", workspaceId: workspace.id }))
        .toContain("Renamed 👩‍💻");
    }
  });

  it.each(["create", "join"] as const)("keeps a successful %s visible when its presentation snapshot fails", async (action) => {
    const failure = fixture.errors.find(({ code }) => code === "timeout") as BridgeError;
    const calls: string[] = [];
    const controller = new ShellController(fakeBridge({
      listWorkspaces: async () => [],
      createWorkspace: async () => { calls.push("create"); return { workspace, membership }; },
      joinWorkspace: async () => { calls.push("join"); return { workspace, membership }; },
      personNameSnapshot: async () => { throw failure; },
    }), () => undefined);
    if (action === "create") await controller.createWorkspace(workspace.name, "Alice");
    else await controller.joinWorkspace(invitation.value, "Alice");
    expect(calls).toEqual([action]);
    expect(controller.snapshot()).toMatchObject({
      workspaces: [workspace], activeWorkspaceId: workspace.id, loading: false, error: null,
      notice: `${action === "create" ? "Created" : "Joined"} ${workspace.name}`,
    });
    expect(controller.snapshot().memberships[workspace.id]).toEqual(membership);
    expect(await rendered(controller.snapshot(), { kind: "workspace-chooser" })).not.toContain('id="retry"');
  });

  it("keeps a successful membership refresh when its optional name snapshot fails", async () => {
    const failure = fixture.errors.find(({ code }) => code === "timeout") as BridgeError;
    const refreshed: Membership = { ...membership, version: "c".repeat(64) };
    const controller = new ShellController(fakeBridge({
      currentMembership: async () => refreshed,
      personNameSnapshot: async () => { throw failure; },
    }), () => undefined);
    await controller.load();
    await controller.refreshMembership(workspace.id);
    expect(controller.snapshot()).toMatchObject({
      loading: false, error: null, notice: "Membership refreshed",
      memberships: { [workspace.id]: refreshed },
    });
    expect(controller.snapshot().personNames[workspace.id]).toBeUndefined();
  });

  it("keeps a durable self rename successful when its optional presentation snapshot fails", async () => {
    const failure = fixture.errors.find(({ code }) => code === "timeout") as BridgeError;
    const calls: string[] = [];
    const controller = new ShellController(fakeBridge({
      setOwnPersonName: async (id, name) => { calls.push(`${id}:${name}`); },
      personNameSnapshot: async () => { throw failure; },
    }), () => undefined);
    await controller.load();
    await controller.rename(workspace.id, "Alice");
    expect(calls).toEqual([`${workspace.id}:Alice`]);
    expect(controller.snapshot()).toMatchObject({ loading: false, error: null, notice: "Name updated" });
    expect(controller.snapshot().workspaces).toEqual([workspace]);
  });

  it("loads authoritative Workspaces and memberships when one optional name snapshot fails", async () => {
    const workspaceB: Workspace = { ...workspace, id: "f".repeat(32), name: "Second" };
    const membershipB: Membership = { ...membership, workspaceId: workspaceB.id };
    const failure = fixture.errors.find(({ code }) => code === "timeout") as BridgeError;
    const controller = new ShellController(fakeBridge({
      listWorkspaces: async () => [workspace, workspaceB],
      currentMembership: async (id) => id === workspace.id ? membership : membershipB,
      personNameSnapshot: async (id) => {
        if (id === workspaceB.id) throw failure;
        return { workspaceId: id, cursor: "1", persons: [] };
      },
    }), () => undefined);
    await controller.load();
    expect(controller.snapshot()).toMatchObject({
      workspaces: [workspace, workspaceB], activeWorkspaceId: workspace.id, loading: false, error: null,
      memberships: { [workspace.id]: membership, [workspaceB.id]: membershipB },
    });
    expect(controller.snapshot().personNames[workspace.id]?.cursor).toBe("1");
    expect(controller.snapshot().personNames[workspaceB.id]).toBeUndefined();
    expect(await rendered(controller.snapshot(), { kind: "workspace-chooser" })).toContain(workspaceB.name);
  });

  it("passes the chosen name on join and persists self rename only in the selected Workspace", async () => {
    const calls: string[] = [];
    const controller = new ShellController(fakeBridge({
      joinWorkspace: async (_invitation, name) => { calls.push(`join:${name}`); return { workspace, membership }; },
      setOwnPersonName: async (id, name) => { calls.push(`rename:${id}:${name}`); },
    }), () => undefined);
    await controller.joinWorkspace(invitation.value, "Alice 👩‍💻");
    await controller.rename(workspace.id, "Alice & Bob");
    expect(calls).toEqual(["join:Alice 👩‍💻", `rename:${workspace.id}:Alice & Bob`]);
  });
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
    expect(memberList.indexOf(`Member ${membership.members[0]!.identity.slice(0, 12)}`))
      .toBeLessThan(memberList.indexOf(`Member ${membership.members[1]!.identity.slice(0, 12)}`));
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
    expect(settings).toContain("Your name");
    expect(settings).not.toMatch(/leave Workspace|revoke|manager administration/i);
  });

  it("creates a Workspace and exposes loading and success feedback", async () => {
    let release: ((value: WorkspaceState) => void) | undefined;
    const pending = new Promise<WorkspaceState>((resolve) => { release = resolve; });
    const updates: boolean[] = [];
    const controller = new ShellController(fakeBridge({ createWorkspace: async () => pending }),
      (nextModel) => updates.push(nextModel.loading));
    const operation = controller.createWorkspace(workspace.name, "Alice");
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
    await controller.joinWorkspace(invitation.value, "Alice");
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
    for (const member of membership.members) expect(html).toContain(member.identity);
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
