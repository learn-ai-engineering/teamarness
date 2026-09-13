import { describe, expect, it } from "vitest";
import fixture from "../contracts/fixtures/bridge-contract.json";
import type { ResonanceBridge } from "./bridge";
import type { BridgeError, Installation, Invitation, Membership, Workspace, WorkspaceState } from "./contracts";
import { ShellController, renderShell } from "./view";

const installation = fixture.installation as Installation;
const workspace = fixture.workspace as Workspace;
const membership = fixture.membership as Membership;
const invitation = fixture.invitation as Invitation;

function fakeBridge(overrides: Partial<ResonanceBridge> = {}): ResonanceBridge {
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

describe("Shell workspace flow", () => {
  it("loads the Installation, Workspace list, ordered members, and version", async () => {
    const controller = new ShellController(fakeBridge(), () => undefined);
    await controller.load();
    const html = renderShell(controller.snapshot());
    expect(html).toContain(installation.identity);
    expect(html).toContain(workspace.name);
    const memberList = html.slice(html.indexOf('<ol class="members">'));
    expect(memberList.indexOf(membership.members[0]!.identity)).toBeLessThan(memberList.indexOf(membership.members[1]!.identity));
    expect(html).toContain(membership.version);
  });

  it("creates a Workspace and exposes loading and success states", async () => {
    let release: ((value: WorkspaceState) => void) | undefined;
    const pending = new Promise<WorkspaceState>((resolve) => { release = resolve; });
    const updates: boolean[] = [];
    const controller = new ShellController(fakeBridge({ createWorkspace: async () => pending }),
      (model) => updates.push(model.loading));
    const operation = controller.createWorkspace(workspace.name);
    expect(controller.snapshot().loading).toBe(true);
    release?.({ workspace, membership });
    await operation;
    expect(updates).toContain(true);
    expect(controller.snapshot().notice).toBe(`Created ${workspace.name}`);
  });

  it("copies an invitation only through the public bridge result", async () => {
    const copied: string[] = [];
    const controller = new ShellController(fakeBridge(), () => undefined, async (value) => { copied.push(value); });
    await controller.copyInvitation(workspace.id);
    expect(copied).toEqual([invitation.value]);
    expect(controller.snapshot().notice).toContain("Sensitive bearer");
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
  });

  it("refreshes issuer membership after another Installation joins", async () => {
    const creatorOnly: Membership = { ...membership, epoch: "1", members: [membership.members[1]!] };
    let currentMembership = creatorOnly;
    const controller = new ShellController(fakeBridge({
      currentMembership: async () => currentMembership,
    }), () => undefined);

    await controller.load();
    expect(controller.snapshot().memberships[workspace.id]?.members).toEqual(creatorOnly.members);

    currentMembership = membership;
    await controller.refreshMembership(workspace.id);

    const html = renderShell(controller.snapshot());
    expect(html).toContain('data-refresh-membership="00112233445566778899aabbccddeeff"');
    expect(controller.snapshot().memberships[workspace.id]?.members).toEqual(membership.members);
    expect(html).toContain(membership.members[0]!.identity);
    expect(html).toContain(membership.members[1]!.identity);
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
    expect(renderShell(controller.snapshot())).toContain('role="alert"');
  });

  it("shows stable failure and retry states", async () => {
    const failure = fixture.errors.find(({ code }) => code === "timeout") as BridgeError;
    const controller = new ShellController(fakeBridge({ listWorkspaces: async () => { throw failure; } }), () => undefined);
    await controller.load();
    expect(controller.snapshot()).toMatchObject({ loading: false, error: "timeout" });
    expect(renderShell(controller.snapshot())).toContain('id="retry"');
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
