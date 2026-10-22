import { describe, expect, it, vi } from "vitest";
import fixture from "../contracts/fixtures/bridge-contract.json";
import type { TeamarnessBridge } from "./bridge";
import type { BridgeError, Installation, Invitation, Membership, PrincipalsSnapshot, Workspace, WorkspaceState } from "./contracts";
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
    renameWorkspacePrincipal: async () => undefined,
    addWorkspaceAgent: async () => undefined,
    removeWorkspaceAgent: async () => undefined,
    principalsSnapshot: async (workspaceId): Promise<PrincipalsSnapshot> => ({ ...fixture.principals, workspaceId, cursor: "0" } as PrincipalsSnapshot),
    subscribePrincipals: async () => undefined,
    ...overrides,
  };
}

function model(change: Partial<ShellModel> = {}): ShellModel {
  return {
    installation,
    workspaces: [workspace],
    memberships: { [workspace.id]: membership },
    principals: {},
    activeWorkspaceId: workspace.id,
    loading: false,
    error: null,
    notice: null,
    membershipRefreshUnavailable: null,
    ...change,
  };
}

async function rendered(source: ShellModel, route: ShellRoute): Promise<string> {
  return renderShell(await buildShellPresentation(source, route.kind === "workspace-chooser" ? null : workspace.id), route);
}

describe("Shell Workspace screens", () => {
  it("restarts a terminated name stream from the last cursor and cancels retries on disposal", async () => {
    vi.useFakeTimers();
    try {
      const cursors: string[] = [];
      const terminals: (() => void)[] = [];
      let listener: ((snapshot: PrincipalsSnapshot) => void) | undefined;
      const controller = new ShellController(fakeBridge({
        subscribePrincipals: async (_id, cursor, next, terminated) => {
          cursors.push(cursor);
          listener = next;
          terminals.push(terminated);
        },
      }), () => undefined);
      await controller.load();
      expect(cursors).toEqual(["0"]);
      listener?.({ workspaceId: workspace.id, cursor: "1", principals: [] });
      terminals[0]?.();
      await vi.advanceTimersByTimeAsync(1000);
      expect(cursors).toEqual(["0", "1"]);
      listener?.({ workspaceId: workspace.id, cursor: "2", principals: [
        { principalId: "d".repeat(64), kind: "person", stewardPrincipalId: null, keys: [installation.identity], name: "Renamed", fingerprint: "aaaa" },
      ] });
      expect(controller.snapshot().principals[workspace.id]?.principals[0]?.name).toBe("Renamed");
      terminals[1]?.();
      controller.dispose();
      await vi.advanceTimersByTimeAsync(60_000);
      expect(cursors).toHaveLength(2);
    } finally { vi.useRealTimers(); }
  });
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
    expect(controller.snapshot().principals[workspaceB.id]?.workspaceId).toBe(workspaceB.id);
    expect(controller.snapshot().principals[workspace.id]?.principals.find(({ keys }) => keys.includes(installation.identity))?.name).not.toBe("👩‍💻");
  });

  it("subscribes to changed names, scopes updates and escapes name text", async () => {
    let listener: ((snapshot: PrincipalsSnapshot) => void) | undefined;
    const controller = new ShellController(fakeBridge({
      principalsSnapshot: async (workspaceId) => ({ workspaceId, cursor: "0", principals: [] }),
      subscribePrincipals: async (_workspaceId, _cursor, next) => { listener = next; },
    }), () => undefined);
    await controller.load();
    listener?.({ workspaceId: workspace.id, cursor: "1", principals: [
      { principalId: "c".repeat(64), kind: "person", stewardPrincipalId: null, keys: [membership.members[0]!.identity], fingerprint: "deadbeef", name: "👩‍💻<script>" },
    ] });
    const html = await rendered(controller.snapshot(), { kind: "workspace-settings", workspaceId: workspace.id });
    expect(html).toContain("👩‍💻&lt;script&gt;");
    expect(html).not.toContain("<script>");
    const self = { principalId: "d".repeat(64), kind: "person" as const, stewardPrincipalId: null, keys: [installation.identity], fingerprint: "abcdef12", name: 'Alice "<script>' };
    listener?.({ workspaceId: workspace.id, cursor: "2", principals: [self] });
    const renamed = await rendered(controller.snapshot(), { kind: "workspace-settings", workspaceId: workspace.id });
    expect(renamed).toContain('value="Alice &quot;&lt;script&gt;"');
    expect(controller.snapshot().principals[workspace.id]?.cursor).toBe("2");
    listener?.({ workspaceId: workspace.id, cursor: "0", principals: [] });
    expect(controller.snapshot().principals[workspace.id]?.cursor).toBe("2");
  });

  it.each(["load", "refresh", "post-add"] as const)("keeps a newer Principal wake roster over a delayed %s result, then accepts a newer read", async (action) => {
    let listener: ((snapshot: PrincipalsSnapshot) => void) | undefined;
    let release: ((value: Membership) => void) | undefined;
    let pending: Promise<Membership> | undefined;
    let reads = 0;
    let current: Membership;
    const epoch2 = { ...membership, epoch: "2" };
    const epoch3 = { ...membership, epoch: "3" };
    const epoch4 = { ...membership, epoch: "4" };
    current = epoch2;
    const controller = new ShellController(fakeBridge({
      currentMembership: async () => {
        reads++;
        return pending ?? current;
      },
      subscribePrincipals: async (_id, _cursor, next) => { listener = next; },
    }), () => undefined);
    await controller.load();
    pending = new Promise<Membership>((resolve) => { release = resolve; });
    const inFlight = action === "load" ? controller.load() : action === "refresh"
      ? controller.refreshMembership(workspace.id) : controller.addAgent(workspace.id, "Helper");
    // Allow the delayed read to start before the wake triggers its own read.
    await vi.waitFor(() => expect(reads).toBe(2));
    pending = undefined;
    current = epoch3;
    listener?.({ workspaceId: workspace.id, cursor: "1", principals: [] });
    await vi.waitFor(() => expect(reads).toBe(3));
    expect(controller.snapshot().memberships[workspace.id]?.epoch).toBe("3");
    release?.(epoch2);
    await inFlight;
    expect(controller.snapshot().memberships[workspace.id]?.epoch).toBe("3");
    current = epoch4;
    await controller.refreshMembership(workspace.id);
    expect(controller.snapshot().memberships[workspace.id]?.epoch).toBe("4");
  });

  it("keeps a live rename when a delayed refresh or load returns an older snapshot", async () => {
    let listener: ((snapshot: PrincipalsSnapshot) => void) | undefined;
    let releaseSnapshot: ((snapshot: PrincipalsSnapshot) => void) | undefined;
    const pending = () => new Promise<PrincipalsSnapshot>((resolve) => { releaseSnapshot = resolve; });
    let nextSnapshot: Promise<PrincipalsSnapshot> | undefined;
    const controller = new ShellController(fakeBridge({
      principalsSnapshot: async (id) => nextSnapshot ?? { ...fixture.principals, workspaceId: id, cursor: "1" } as PrincipalsSnapshot,
      subscribePrincipals: async (_id, _cursor, next) => { listener = next; },
    }), () => undefined);
    await controller.load();
    const newer: PrincipalsSnapshot = { workspaceId: workspace.id, cursor: "2", principals: [
      { principalId: "d".repeat(64), kind: "person", stewardPrincipalId: null, keys: [installation.identity], fingerprint: "deadbeef", name: "Renamed 👩‍💻" },
    ] };
    for (const operation of [() => controller.refreshMembership(workspace.id), () => controller.load()]) {
      nextSnapshot = pending();
      const inFlight = operation();
      await Promise.resolve();
      listener?.(newer);
      releaseSnapshot?.({ workspaceId: workspace.id, cursor: "1", principals: [] });
      await inFlight;
      expect(controller.snapshot().principals[workspace.id]).toEqual(newer);
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
      principalsSnapshot: async () => { throw failure; },
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
      principalsSnapshot: async () => { throw failure; },
    }), () => undefined);
    await controller.load();
    await controller.refreshMembership(workspace.id);
    expect(controller.snapshot()).toMatchObject({
      loading: false, error: null, notice: "Membership refreshed",
      memberships: { [workspace.id]: refreshed },
    });
    expect(controller.snapshot().principals[workspace.id]).toBeUndefined();
  });

  it("keeps a durable self rename successful when its optional presentation snapshot fails", async () => {
    const failure = fixture.errors.find(({ code }) => code === "timeout") as BridgeError;
    const calls: string[] = [];
    const controller = new ShellController(fakeBridge({
      renameWorkspacePrincipal: async (id, _principal, name) => { calls.push(`${id}:${name}`); },
      principalsSnapshot: async () => { throw failure; },
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
      principalsSnapshot: async (id) => {
        if (id === workspaceB.id) throw failure;
        return { ...fixture.principals, workspaceId: id, cursor: "1" } as PrincipalsSnapshot;
      },
    }), () => undefined);
    await controller.load();
    expect(controller.snapshot()).toMatchObject({
      workspaces: [workspace, workspaceB], activeWorkspaceId: workspace.id, loading: false, error: null,
      memberships: { [workspace.id]: membership, [workspaceB.id]: membershipB },
    });
    expect(controller.snapshot().principals[workspace.id]?.cursor).toBe("1");
    expect(controller.snapshot().principals[workspaceB.id]).toBeUndefined();
    expect(await rendered(controller.snapshot(), { kind: "workspace-chooser" })).toContain(workspaceB.name);
  });

  it("passes the chosen name on join and persists self rename only in the selected Workspace", async () => {
    const calls: string[] = [];
    const controller = new ShellController(fakeBridge({
      listWorkspaces: async () => [],
      joinWorkspace: async (_invitation, name) => { calls.push(`join:${name}`); return { workspace, membership }; },
      renameWorkspacePrincipal: async (id, _principal, name) => { calls.push(`rename:${id}:${name}`); },
    }), () => undefined);
    await controller.load();
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
    expect(memberList.indexOf("Remote")).toBeLessThan(memberList.indexOf("Alice"));
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
      "uncertain_storage_outcome", "shutdown", "not_steward", "principal_not_bound",
    ]);
    expect(fixture.workspace.id).toMatch(/^[0-9a-f]{32}$/);
    expect(fixture.membership.version).toMatch(/^[0-9a-f]{64}$/);
    expect(fixture.agentMutationResponses).toEqual({ addWorkspaceAgent: null, removeWorkspaceAgent: null });
  });
});

describe("Workspace Agent settings", () => {
  const agentId = "e".repeat(64);
  const agentKey = "f".repeat(64);
  const agentMember = { identity: agentKey, role: "member" as const, principalId: agentId, kind: "agent" as const, stewardPrincipalId: "c".repeat(64) };
  const agentProfile = { principalId: agentId, kind: "agent" as const, stewardPrincipalId: "c".repeat(64), keys: [agentKey], name: "Helper <script>", fingerprint: agentKey.slice(0, 12) };

  it.each(["add", "remove"] as const)("reports committed %s success and a distinct refresh action when roster read fails", async (action) => {
    const calls: string[] = [];
    let fail = false;
    const controller = new ShellController(fakeBridge({
      addWorkspaceAgent: async () => { calls.push("add"); },
      removeWorkspaceAgent: async () => { calls.push("remove"); },
      currentMembership: async () => { calls.push("roster"); if (fail) throw { code: "timeout", retryable: true }; return membership; },
    }), () => undefined);
    await controller.load();
    fail = true;
    if (action === "add") await controller.addAgent(workspace.id, "Helper");
    else await controller.removeAgent(workspace.id, agentId);
    expect(controller.snapshot()).toMatchObject({ error: null, notice: action === "add" ? "Agent added" : "Agent removed", membershipRefreshUnavailable: workspace.id });
    expect(await rendered(controller.snapshot(), { kind: "workspace-settings", workspaceId: workspace.id })).toContain('id="retry-membership-refresh"');
    fail = false;
    await controller.refreshMembership(workspace.id);
    expect(controller.snapshot().membershipRefreshUnavailable).toBeNull();
    expect(calls.filter((call) => call === action)).toHaveLength(1);
  });

  it("adds by name and removes by opaque Agent ID, updating the roster even without optional profile reads", async () => {
    const calls: string[] = [];
    let roster: Membership = membership;
    const controller = new ShellController(fakeBridge({
      addWorkspaceAgent: async (id, name) => { calls.push(`add:${id}:${name}`); roster = { ...membership, members: [...membership.members, agentMember] }; },
      removeWorkspaceAgent: async (id, agent) => { calls.push(`remove:${id}:${agent}`); roster = membership; },
      currentMembership: async () => roster,
      principalsSnapshot: async () => { throw { code: "timeout", retryable: true }; },
    }), () => undefined);
    await controller.load();
    await controller.addAgent(workspace.id, "Helper");
    expect(controller.snapshot()).toMatchObject({ notice: "Agent added", error: null });
    expect(controller.snapshot().memberships[workspace.id]?.members).toContainEqual(agentMember);
    await controller.removeAgent(workspace.id, agentId);
    expect(controller.snapshot()).toMatchObject({ notice: "Agent removed", error: null });
    expect(controller.snapshot().memberships[workspace.id]?.members).toEqual(membership.members);
    expect(calls).toEqual([`add:${workspace.id}:Helper`, `remove:${workspace.id}:${agentId}`]);
  });

  it("renders Manager removal but not rename for a remote steward, with identity only inside details", async () => {
    const source = model({
      memberships: { [workspace.id]: { ...membership, members: [...membership.members, agentMember] } },
      principals: { [workspace.id]: { ...fixture.principals, principals: [...fixture.principals.principals, agentProfile] } as PrincipalsSnapshot },
    });
    const html = await rendered(source, { kind: "workspace-settings", workspaceId: workspace.id });
    expect(html).toContain('id="add-agent"');
    expect(html).toContain('Agent · stewarded by Remote');
    expect(html).toContain('data-remove-agent="2"');
    expect(html).not.toContain('data-rename-agent="2"');
    expect(html).toContain('Helper &lt;script&gt;');
    expect(html).not.toContain('<script>');
    expect(html).toContain(`<summary>View identity</summary>Identity: ${agentKey}`);
    expect(html.split('<summary>View identity</summary>')[0]).not.toContain(agentKey);
    expect(html).not.toMatch(/revoke|manager administration/i);
  });

  it("does not show Agent controls to an unrelated member", async () => {
    const source = model({
      installation: { identity: "9".repeat(64), displayName: "Other" },
      memberships: { [workspace.id]: { ...membership, members: [...membership.members, { ...membership.members[0]!, identity: "9".repeat(64), principalId: "8".repeat(64) }, agentMember] } },
      principals: { [workspace.id]: { ...fixture.principals, principals: [...fixture.principals.principals, agentProfile] } as PrincipalsSnapshot },
    });
    const html = await rendered(source, { kind: "workspace-settings", workspaceId: workspace.id });
    expect(html).not.toContain('data-remove-agent="2"');
    expect(html).not.toContain('data-rename-agent="2"');
  });
});

describe("remote Agent roster updates", () => {
  it("refreshes membership on a Principal wake and ignores an older in-flight roster", async () => {
    let listener: ((snapshot: PrincipalsSnapshot) => void) | undefined;
    let nextMembership = membership;
    const agent = { identity: "f".repeat(64), role: "member" as const, principalId: "e".repeat(64), kind: "agent" as const, stewardPrincipalId: "c".repeat(64) };
    const controller = new ShellController(fakeBridge({
      currentMembership: async () => nextMembership,
      subscribePrincipals: async (_id, _cursor, next) => { listener = next; },
    }), () => undefined);
    await controller.load();
    nextMembership = { ...membership, epoch: "3", members: [...membership.members, agent] };
    listener?.({ ...fixture.principals, cursor: "2" } as PrincipalsSnapshot);
    await vi.waitFor(() => expect(controller.snapshot().memberships[workspace.id]?.members).toContainEqual(agent));
    nextMembership = membership;
    listener?.({ ...fixture.principals, cursor: "3" } as PrincipalsSnapshot);
    await Promise.resolve();
    expect(controller.snapshot().memberships[workspace.id]?.epoch).toBe("3");
    controller.dispose();
  });
});

describe("Agent rename authorization feedback", () => {
  it("shows rename and removal to the Agent's steward, but reports a refused rename without leaking identifiers", async () => {
    const agentId = "e".repeat(64);
    const agentKey = "f".repeat(64);
    const agent = { identity: agentKey, role: "member" as const, principalId: agentId, kind: "agent" as const, stewardPrincipalId: "d".repeat(64) };
    const agentProfile = { principalId: agentId, kind: "agent" as const, stewardPrincipalId: "d".repeat(64), keys: [agentKey], name: "Helper", fingerprint: agentKey.slice(0, 12) };
    const source = model({
      memberships: { [workspace.id]: { ...membership, members: [...membership.members, agent] } },
      principals: { [workspace.id]: { ...fixture.principals, principals: [...fixture.principals.principals, agentProfile] } as PrincipalsSnapshot },
    });
    const html = await rendered(source, { kind: "workspace-settings", workspaceId: workspace.id });
    expect(html).toContain('data-rename-agent="2"');
    expect(html).toContain('data-remove-agent="2"');

    const controller = new ShellController(fakeBridge({
      renameWorkspacePrincipal: async () => { throw { code: "not_steward", retryable: false }; },
    }), () => undefined);
    await controller.load();
    await controller.renameAgent(workspace.id, agentId, "New helper");
    const failure = await rendered(controller.snapshot(), { kind: "workspace-settings", workspaceId: workspace.id });
    expect(failure).toContain("Only the steward can rename this Agent");
    expect(failure).not.toContain(agentId);
  });
});
