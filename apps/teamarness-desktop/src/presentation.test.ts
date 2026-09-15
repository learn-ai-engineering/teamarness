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
  }],
  instances: [{
    key: { packageIdentity, workspaceId: workspace.id },
    lifecycle: "recovering",
    processGeneration: 812,
    mountState: "opening",
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

  it("maps no-Package and populated navigation to display names and private action indexes only", () => {
    expect(buildPackageNavigationPresentation(null, workspace.id)).toEqual({ teamPackages: [], privatePackages: [] });
    const presented = buildPackageNavigationPresentation(snapshot, workspace.id);
    expect(presented).toEqual({
      teamPackages: [],
      privatePackages: [{ name: "Planning", actionIndex: 0 }],
    });
    const serialized = JSON.stringify(presented);
    for (const forbidden of [
      workspace.id,
      packageIdentity,
      contractDigest,
      diagnosticId,
      "local",
      "trusted_native_code_v1",
      "running",
      "recovering",
      "opening",
      "812",
      "991",
    ]) expect(serialized).not.toContain(forbidden);
  });
});
