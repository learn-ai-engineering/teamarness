import { Channel, invoke } from "@tauri-apps/api/core";
import { createTauriPackageTransport, type PackageTransport, type PackageValue } from "@teamarness/package-ui-sdk";
import type {
  OpenPackageMount,
  PackageHostSnapshot,
  PackageHostStateNotification,
  PackageInstanceKey,
} from "./package-contracts";

export interface PackageShellBridge {
  readonly transport: PackageTransport;
  snapshot(): Promise<PackageHostSnapshot>;
  subscribeHostState(afterRevision: number, listener: (event: PackageHostStateNotification) => void): Promise<void>;
  openMount(instance: PackageInstanceKey): Promise<OpenPackageMount>;
  closeMount(input: Readonly<{ mount: string }>): Promise<void>;
  reportUiFault(input: Readonly<{ mount: string; code: "ui_exception" }>): Promise<void>;
  suspendBackend(instance: PackageInstanceKey): Promise<void>;
  recoverBackend(instance: PackageInstanceKey): Promise<void>;
  disposeBackend(instance: PackageInstanceKey): Promise<void>;
}

const tauriPackageBridge: PackageShellBridge = {
  transport: createTauriPackageTransport(),
  snapshot: () => invoke<PackageHostSnapshot>("package_catalog_snapshot"),
  subscribeHostState: async (afterRevision, listener) => {
    const onEvent = new Channel(listener);
    await invoke("package_subscribe_host_state", { afterRevision, onEvent });
  },
  openMount: (instance) => invoke<OpenPackageMount>("package_open_mount", { instance }),
  closeMount: async ({ mount }) => { await invoke("package_close_mount", { mount }); },
  reportUiFault: async ({ mount, code }) => { await invoke("package_report_ui_fault", { mount, code }); },
  suspendBackend: async (instance) => { await invoke("package_suspend", { instance }); },
  recoverBackend: async (instance) => { await invoke("package_recover", { instance }); },
  disposeBackend: async (instance) => { await invoke("package_dispose", { instance }); },
};

export const packageBridge = Object.freeze(tauriPackageBridge);

export type { PackageValue };
