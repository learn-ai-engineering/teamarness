import { describe, expect, it } from "vitest";
import fixture from "../contracts/fixtures/bridge-contract.json";
import type { Installation, Membership, Workspace } from "./contracts";
import type { PackageHostSnapshot } from "./package-contracts";
import { buildPackageNavigationPresentation, buildShellPresentation } from "./presentation";
import type { ShellModel } from "./view";

const installation = fixture.installation as Installation;
const workspace = fixture.workspace as Workspace;
const membership = fixture.membership as Membership;
const packageIdentity = "com.example.secret-package-identity";
const contractDigest = "contract-digest-that-must-never-render";
const diagnosticId = "diagnostic-id-that-must-never-render";
const teamPackageIdentity = "com.example.team-package";
const snapshot: PackageHostSnapshot = {
  revision: 991,
  state: "running",
  packages: [{
    identity: packageIdentity,
    version: "9.8.7",
    displayName: "Planning",
    source: "local",
    trust: "trusted_native_code_v1",
    uiContractDigest: contractDigest,
  }, {
    identity: teamPackageIdentity,
    version: "1.0.0",
    displayName: "Notes",
    source: "bundled",
    trust: "application_bundled",
    uiContractDigest: "another-secret-digest",
  }],
  instances: [{
    key: { packageIdentity, workspaceId: workspace.id },
    lifecycle: "failed",
    processGeneration: 812,
    mountState: "opening",
  }, {
    key: { packageIdentity: teamPackageIdentity, workspaceId: workspace.id },
    lifecycle: "active",
    processGeneration: 5,
    mountState: "mounted",
  }, {
    key: { packageIdentity: teamPackageIdentity, workspaceId: "another-workspace" },
    lifecycle: "suspended",
    processGeneration: 2,
    mountState: null,
  }],
  diagnostics: [{
    version: 1,
    namespace: "teamarness.package-host",
    code: "secret_protocol_code",
    stage: "secret_protocol_stage",
    affectedPackage: packageIdentity,
    affectedInstance: { packageIdentity, workspaceId: workspace.id },
    retryable: true,
    recovery: "secret_protocol_recovery",
    diagnosticId,
  }],
};

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

describe("display-safe Shell presentation", () => {
  it("builds chooser, active Workspace, member, feedback, and error models without operational identifiers", async () => {
    const unsafeMembership = { ...membership, epoch: "secret-membership-epoch" };
    const presented = await buildShellPresentation(model({
      memberships: { [workspace.id]: unsafeMembership },
      error: "timeout",
      notice: "Membership refreshed",
    }), workspace.id);
    expect(presented).toMatchObject({
      installationName: installation.displayName,
      workspaceChoices: [{ name: workspace.name }],
      activeWorkspaceIndex: 0,
      workspace: { name: workspace.name, memberCount: membership.members.length },
      notice: "Membership refreshed",
      error: { message: "Workspace onboarding timed out. Try again.", canRetry: true },
    });
    const serialized = JSON.stringify(presented);
    for (const forbidden of [
      installation.identity,
      workspace.id,
      workspace.createdBy,
      membership.workspaceId,
      unsafeMembership.epoch,
      membership.version,
      String(workspace.createdAtUnixMs),
      ...membership.members.map(({ identity }) => identity),
    ]) expect(serialized).not.toContain(forbidden);
  });

  it("represents loading, no-Workspace, and empty-membership states deliberately", async () => {
    await expect(buildShellPresentation(model({
      installation: null,
      workspaces: [],
      memberships: {},
      activeWorkspaceId: null,
      loading: true,
    }), null)).resolves.toMatchObject({
      installationName: null,
      workspaceChoices: [],
      workspace: null,
      loading: true,
    });
    const empty = await buildShellPresentation(model({
      memberships: { [workspace.id]: { ...membership, members: [] } },
    }), workspace.id);
    expect(empty.workspace?.members).toEqual([]);
  });

  it("joins active-Workspace instances with catalog display data, source groups, and lifecycle capabilities", () => {
    expect(buildPackageNavigationPresentation(null, workspace.id)).toEqual({ teamPackages: [], privatePackages: [] });
    const presented = buildPackageNavigationPresentation(snapshot, workspace.id);
    expect(presented).toEqual({
      teamPackages: [{
        name: "Notes",
        availability: "Available",
        actionIndex: 1,
        installationLocal: false,
        sourceLabel: "Team",
        canSuspend: true,
        canRecover: false,
        canDispose: true,
      }],
      privatePackages: [{
        name: "Planning",
        availability: "Needs recovery",
        actionIndex: 0,
        installationLocal: true,
        sourceLabel: "Personal",
        canSuspend: false,
        canRecover: true,
        canDispose: true,
      }],
    });
    const serialized = JSON.stringify(presented);
    for (const forbidden of [
      workspace.id,
      "another-workspace",
      packageIdentity,
      teamPackageIdentity,
      contractDigest,
      diagnosticId,
      "local",
      "bundled",
      "trusted_native_code_v1",
      "application_bundled",
      "running",
      "failed",
      "active",
      "suspended",
      "opening",
      "mounted",
      "812",
      "991",
    ]) expect(serialized).not.toContain(forbidden);
  });

  it("maps every lifecycle to plain availability and gates only valid stable-state actions", () => {
    const lifecycles = [
      "admitted", "activating", "active", "suspending", "suspended", "recovering",
      "failed", "crash_loop", "disposing", "disposed",
    ] as const;
    const lifecycleSnapshot: PackageHostSnapshot = {
      ...snapshot,
      packages: lifecycles.map((lifecycle, index) => ({
        identity: `package-${index}`,
        version: "1.0.0",
        displayName: `Package ${index}`,
        source: "bundled" as const,
        trust: "application_bundled" as const,
        uiContractDigest: "digest",
      })),
      instances: lifecycles.map((lifecycle, index) => ({
        key: { packageIdentity: `package-${index}`, workspaceId: workspace.id },
        lifecycle,
        processGeneration: null,
        mountState: null,
      })),
    };
    const rows = buildPackageNavigationPresentation(lifecycleSnapshot, workspace.id).teamPackages;
    expect(rows.map(({ availability }) => availability)).toEqual([
      "Getting ready", "Getting ready", "Available", "Pausing", "Paused",
      "Getting ready", "Needs recovery", "Needs recovery", "Removing", "Unavailable",
    ]);
    expect(rows.map(({ canSuspend, canRecover, canDispose }) => [canSuspend, canRecover, canDispose])).toEqual([
      [false, false, true], [false, false, false], [true, false, true], [false, false, false],
      [false, true, true], [false, false, false], [false, true, true], [false, true, true],
      [false, false, false], [false, false, false],
    ]);
  });
});
