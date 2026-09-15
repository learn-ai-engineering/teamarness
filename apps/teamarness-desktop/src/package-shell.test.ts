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

it("targets lifecycle actions at the selected Package Instance and refreshes their results", async () => {
  const calls: string[] = [];
  const rendered: PackageHostSnapshot[] = [];
  let actions: Parameters<PackageShellView["render"]>[1] | undefined;
  const snapshots = [
    snapshot,
    { ...snapshot, revision: 2, instances: [{ ...snapshot.instances[0]!, lifecycle: "suspended" as const }, snapshot.instances[1]!] },
    { ...snapshot, revision: 3, instances: [{ ...snapshot.instances[0]!, processGeneration: 7 }, snapshot.instances[1]!] },
    { ...snapshot, revision: 4, instances: [{ ...snapshot.instances[0]!, lifecycle: "disposed" as const, processGeneration: null }, snapshot.instances[1]!] },
  ];
  const view: PackageShellView = {
    mountTarget: {},
    render: (value, nextActions) => { rendered.push(value); actions = nextActions; },
    showFailure: () => undefined,
  };
  const mounts = {
    mount: async (instance: PackageInstanceKey) => { calls.push(`mount:${instance.packageIdentity}`); return {}; },
    unmount: async () => undefined,
    suspendBackend: async (instance: PackageInstanceKey) => { calls.push(`suspend:${instance.packageIdentity}`); },
    recoverBackend: async (instance: PackageInstanceKey) => { calls.push(`recover:${instance.packageIdentity}`); },
    disposeBackend: async (instance: PackageInstanceKey) => { calls.push(`dispose:${instance.packageIdentity}`); },
  };
  const shell = new PackageShell({ snapshot: async () => snapshots.shift() ?? snapshot }, mounts, view);
  await shell.refresh();
  await actions?.select(first);
  await actions?.suspendBackend();
  await actions?.recoverBackend();
  await actions?.disposeBackend();

  expect(calls).toEqual([
    "mount:com.example.one",
    "suspend:com.example.one",
    "recover:com.example.one",
    "dispose:com.example.one",
  ]);
  expect(rendered.map(({ revision }) => revision)).toEqual([1, 2, 3, 4]);
  expect(rendered.map(({ instances }) => instances[0]?.processGeneration)).toEqual([7, 7, 7, null]);
  expect(rendered.at(-1)?.instances[0]).toMatchObject({ lifecycle: "disposed", processGeneration: null });
});

it("keeps Reload UI separate from Backend lifecycle", async () => {
  const calls: string[] = [];
  let actions: Parameters<PackageShellView["render"]>[1] | undefined;
  const view: PackageShellView = {
    mountTarget: {},
    render: (_snapshot, value) => { actions = value; },
    showFailure: () => undefined,
  };
  const mounts = {
    mount: async (instance: PackageInstanceKey) => { calls.push(`mount:${instance.packageIdentity}`); return {}; },
    unmount: async () => undefined,
    suspendBackend: async () => { calls.push("suspend"); },
    recoverBackend: async () => { calls.push("recover"); },
    disposeBackend: async () => { calls.push("dispose"); },
  };
  const shell = new PackageShell({ snapshot: async () => snapshot }, mounts, view);
  await shell.refresh();
  await actions?.select(second);
  await actions?.reloadUi();

  expect(calls).toEqual(["mount:com.example.two", "mount:com.example.two"]);
});

it("contains invalid post-disposal actions in Shell-owned recovery and refreshes the terminal state", async () => {
  let actions: Parameters<PackageShellView["render"]>[1] | undefined;
  let failures = 0;
  let snapshotCalls = 0;
  const disposed = {
    ...snapshot,
    revision: 2,
    instances: [{ ...snapshot.instances[0]!, lifecycle: "disposed" as const, processGeneration: null }, snapshot.instances[1]!],
  };
  const view: PackageShellView = {
    mountTarget: {},
    render: (_snapshot, value) => { actions = value; },
    showFailure: () => { failures += 1; },
  };
  const mounts = {
    mount: async () => ({}),
    unmount: async () => undefined,
    suspendBackend: async () => { throw new Error("disposed"); },
    recoverBackend: async () => undefined,
    disposeBackend: async () => { throw new Error("dispose returned Backend error"); },
  };
  const shell = new PackageShell({
    snapshot: async () => {
      snapshotCalls += 1;
      return snapshotCalls === 1 ? snapshot : disposed;
    },
  }, mounts, view);
  await shell.refresh();
  await actions?.select(first);
  await expect(actions?.disposeBackend()).resolves.toBeUndefined();
  expect(failures).toBe(1);
  await expect(actions?.suspendBackend()).resolves.toBeUndefined();

  expect(failures).toBe(2);
  expect(snapshotCalls).toBe(3);
});

it("shows Shell recovery when the selected mount reports a terminal stream failure", async () => {
  let actions: Parameters<PackageShellView["render"]>[1] | undefined;
  let failMount: (() => void) | undefined;
  let failures = 0;
  const view: PackageShellView = {
    mountTarget: {},
    render: (_snapshot, value) => { actions = value; },
    showFailure: () => { failures += 1; },
  };
  const mounts = {
    mount: async (
      _instance: PackageInstanceKey,
      _target: unknown,
      onFailure: () => void,
    ) => { failMount = onFailure; return {}; },
    unmount: async () => undefined,
    suspendBackend: async () => undefined,
    recoverBackend: async () => undefined,
    disposeBackend: async () => undefined,
  };
  const shell = new PackageShell({ snapshot: async () => snapshot }, mounts, view);
  await shell.refresh();
  await actions?.select(first);

  failMount?.();

  expect(failures).toBe(1);
  expect(snapshot.instances[0]).toMatchObject({ lifecycle: "active", processGeneration: 7 });
});

class TestElement {
  public type = "";
  public textContent = "";
  public className = "";
  public role: string | null = null;
  public readonly dataset: Record<string, string> = {};
  public children: TestElement[] = [];
  private readonly attributes: Record<string, string> = {};
  private onClick: (() => void) | undefined;

  public setAttribute(name: string, value: string): void { this.attributes[name] = value; }
  public addEventListener(type: string, listener: () => void): void {
    if (type === "click") this.onClick = listener;
  }
  public click(): void { this.onClick?.(); }
  public append(...children: TestElement[]): void { this.children.push(...children); }
  public replaceChildren(...children: TestElement[]): void { this.children = children; }
}

afterEach(() => { vi.unstubAllGlobals(); });

it("renders Package-neutral lifecycle controls with observable lifecycle and generation", () => {
  vi.stubGlobal("document", { createElement: () => new TestElement() });
  const navigation = new TestElement();
  const view = new BrowserPackageShellView(navigation as unknown as HTMLElement, new TestElement() as unknown as HTMLElement);
  const calls: string[] = [];
  const actions = {
    select: async () => undefined,
    reloadUi: async () => { calls.push("reload"); },
    suspendBackend: async () => { calls.push("suspend"); },
    recoverBackend: async () => { calls.push("recover"); },
    disposeBackend: async () => { calls.push("dispose"); },
  };

  view.render(snapshot, actions);

  expect(navigation.children[1]?.children[0]?.textContent).toContain("active · generation 7");
  expect(navigation.children[1]?.children[0]?.children[0]?.textContent).toBe("Bundled");
  expect(navigation.children[1]?.children[1]?.children[0]?.textContent).toBe("Local");
  const controls = navigation.children[2]?.children ?? [];
  expect(controls.map(({ textContent }) => textContent)).toEqual([
    "Reload UI",
    "Suspend",
    "Recover Backend",
    "Dispose",
  ]);
  for (const control of controls) control.click();
  expect(calls).toEqual(["reload", "suspend", "recover", "dispose"]);
});
