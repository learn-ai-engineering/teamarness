import { afterEach, expect, it, vi } from "vitest";
import type { PackageHostSnapshot, PackageInstanceKey } from "./package-contracts";
import { BrowserPackageShellView, PackageShell, type PackageShellView } from "./package-shell";

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

function view(failures: { value: number } = { value: 0 }): PackageShellView {
  return {
    mountTarget: {},
    showFailure: () => { failures.value += 1; },
  };
}

it("targets lifecycle actions at the selected Package Instance and refreshes their results", async () => {
  const calls: string[] = [];
  let snapshotCalls = 0;
  const mounts = {
    mount: async (instance: PackageInstanceKey) => { calls.push(`mount:${instance.packageIdentity}`); return {}; },
    unmount: async () => undefined,
    suspendBackend: async (instance: PackageInstanceKey) => { calls.push(`suspend:${instance.packageIdentity}`); },
    recoverBackend: async (instance: PackageInstanceKey) => { calls.push(`recover:${instance.packageIdentity}`); },
    disposeBackend: async (instance: PackageInstanceKey) => { calls.push(`dispose:${instance.packageIdentity}`); },
  };
  const shell = new PackageShell({ snapshot: async () => { snapshotCalls += 1; return snapshot; } }, mounts, view());
  await shell.refresh();
  await shell.select(first);
  await shell.suspendBackend();
  await shell.recoverBackend();
  await shell.disposeBackend();

  expect(calls).toEqual([
    "mount:com.example.one",
    "suspend:com.example.one",
    "recover:com.example.one",
    "dispose:com.example.one",
  ]);
  expect(snapshotCalls).toBe(4);
});

it("keeps Reload UI separate from Backend lifecycle", async () => {
  const calls: string[] = [];
  const mounts = {
    mount: async (instance: PackageInstanceKey) => { calls.push(`mount:${instance.packageIdentity}`); return {}; },
    unmount: async () => undefined,
    suspendBackend: async () => { calls.push("suspend"); },
    recoverBackend: async () => { calls.push("recover"); },
    disposeBackend: async () => { calls.push("dispose"); },
  };
  const shell = new PackageShell({ snapshot: async () => snapshot }, mounts, view());
  await shell.select(second);
  await shell.reloadUi();

  expect(calls).toEqual(["mount:com.example.two", "mount:com.example.two"]);
});

it("unmounts a hidden Package UI without a Backend lifecycle call", async () => {
  const calls: string[] = [];
  const mounts = {
    mount: async () => ({}),
    unmount: async () => { calls.push("unmount"); },
    suspendBackend: async () => { calls.push("suspend"); },
    recoverBackend: async () => { calls.push("recover"); },
    disposeBackend: async () => { calls.push("dispose"); },
  };
  const shell = new PackageShell({ snapshot: async () => snapshot }, mounts, view());

  await shell.select(first);
  await shell.hide();

  expect(calls).toEqual(["unmount"]);
});

it("contains invalid post-disposal actions in Shell-owned recovery", async () => {
  const failures = { value: 0 };
  let snapshotCalls = 0;
  const mounts = {
    mount: async () => ({}),
    unmount: async () => undefined,
    suspendBackend: async () => { throw new Error("disposed"); },
    recoverBackend: async () => undefined,
    disposeBackend: async () => { throw new Error("dispose returned Backend error"); },
  };
  const shell = new PackageShell({ snapshot: async () => { snapshotCalls += 1; return snapshot; } }, mounts, view(failures));
  await shell.select(first);
  await shell.disposeBackend();
  await shell.suspendBackend();

  expect(failures.value).toBe(2);
  expect(snapshotCalls).toBe(2);
});

it("shows Shell recovery when the selected mount reports a terminal stream failure", async () => {
  let failMount: (() => void) | undefined;
  const failures = { value: 0 };
  const mounts = {
    mount: async (_instance: PackageInstanceKey, _target: unknown, onFailure: () => void) => { failMount = onFailure; return {}; },
    unmount: async () => undefined,
    suspendBackend: async () => undefined,
    recoverBackend: async () => undefined,
    disposeBackend: async () => undefined,
  };
  const shell = new PackageShell({ snapshot: async () => snapshot }, mounts, view(failures));
  await shell.select(first);

  failMount?.();

  expect(failures.value).toBe(1);
});

class TestElement {
  public type = "";
  public textContent = "";
  public className = "";
  public role: string | null = null;
  public children: TestElement[] = [];
  private onClick: (() => void) | undefined;

  public addEventListener(type: string, listener: () => void): void {
    if (type === "click") this.onClick = listener;
  }
  public click(): void { this.onClick?.(); }
  public replaceChildren(...children: TestElement[]): void { this.children = children; }
}

afterEach(() => { vi.unstubAllGlobals(); });

it("narrows the browser view to mount recovery actions", () => {
  vi.stubGlobal("document", { createElement: () => new TestElement() });
  const target = new TestElement();
  const view = new BrowserPackageShellView(target as unknown as HTMLElement);
  const calls: string[] = [];

  view.showFailure({
    reloadUi: async () => { calls.push("reload"); },
    recoverBackend: async () => { calls.push("recover"); },
  });

  expect(target.children.map(({ textContent }) => textContent)).toEqual([
    "This Package interface failed.",
    "Reload UI",
    "Recover Backend",
  ]);
  target.children[1]?.click();
  target.children[2]?.click();
  expect(calls).toEqual(["reload", "recover"]);
});
