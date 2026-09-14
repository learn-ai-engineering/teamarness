import { expect, it } from "vitest";
import type { PackageHostSnapshot, PackageInstanceKey } from "./package-contracts";
import { PackageShell, type PackageShellView } from "./package-shell";

const first: PackageInstanceKey = { packageIdentity: "com.example.one", workspaceId: "workspace-one" };
const second: PackageInstanceKey = { packageIdentity: "com.example.two", workspaceId: "workspace-two" };
const snapshot: PackageHostSnapshot = {
  revision: 1,
  state: "running",
  packages: [
    { identity: first.packageIdentity, version: "1.0.0", displayName: "One", source: "bundled", trust: "application_bundled", uiContractDigest: "a" },
    { identity: second.packageIdentity, version: "1.0.0", displayName: "Two", source: "local", trust: "trusted_native_code_v1", uiContractDigest: "b" },
  ],
  instances: [
    { key: first, lifecycle: "active", processGeneration: 7, mountState: null },
    { key: second, lifecycle: "active", processGeneration: 9, mountState: null },
  ],
  diagnostics: [],
};

it("keeps navigation, UI reload, and Backend recovery as separate operations", async () => {
  const calls: string[] = [];
  let actions: Parameters<PackageShellView["render"]>[1] | undefined;
  const view: PackageShellView = {
    mountTarget: {},
    render: (_snapshot, value) => { actions = value; },
    showFailure: () => undefined,
  };
  const mounts = {
    mount: async (instance: PackageInstanceKey) => { calls.push(`mount:${instance.packageIdentity}`); return { unmount: async () => undefined }; },
    unmount: async () => { calls.push("unmount"); },
    recoverBackend: async (instance: PackageInstanceKey) => { calls.push(`recover:${instance.packageIdentity}`); },
  };
  const shell = new PackageShell({ snapshot: async () => snapshot }, mounts, view);
  await shell.refresh();
  await actions?.select(first);
  await actions?.select(second);
  await actions?.reloadUi();
  await actions?.recoverBackend();
  expect(calls).toEqual([
    "mount:com.example.one",
    "mount:com.example.two",
    "mount:com.example.two",
    "recover:com.example.two",
  ]);
});
