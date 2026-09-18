import { Channel, invoke } from "@tauri-apps/api/core";
import type { PackageValue } from "@teamarness/package-ui-sdk";
import type {
  HostUiEvent,
  OpenPackageMount,
  PackageHostSnapshot,
  PackageHostStateNotification,
  PackageHostTransport,
  PackageInstanceKey,
} from "./package-contracts";

export interface PackageShellBridge {
  readonly transport: PackageHostTransport;
  snapshot(): Promise<PackageHostSnapshot>;
  subscribeHostState(afterRevision: number, listener: (event: PackageHostStateNotification) => void): Promise<void>;
  openMount(instance: PackageInstanceKey): Promise<OpenPackageMount>;
  closeMount(input: Readonly<{ mount: string }>): Promise<void>;
  reportUiFault(input: Readonly<{ mount: string; code: "ui_exception" }>): Promise<void>;
  suspendBackend(instance: PackageInstanceKey): Promise<void>;
  recoverBackend(instance: PackageInstanceKey): Promise<void>;
  disposeBackend(instance: PackageInstanceKey): Promise<void>;
}

/**
 * The Shell's own Tauri command surface for one Package UI Mount. Only the
 * Shell realm holds it; a Package UI Frame reaches it through the port bridge.
 */
export function createTauriPackageHostTransport(
  call: typeof invoke = invoke,
  makeChannel: <T>(handler: (message: T) => void) => Channel<T> = (handler) => new Channel(handler),
): PackageHostTransport {
  const transport: PackageHostTransport = {
    request: async (input) => call<PackageValue>("package_request", input),
    cancelRequest: async (input) => { await call("package_cancel_request", input); },
    openSubscription: async (input, deliver, terminate) => {
      type StreamMessage =
        | Readonly<{ kind: "event"; value: HostUiEvent }>
        | Readonly<{ kind: "failure"; value: unknown }>;
      const onEvent = makeChannel<StreamMessage>((message) => {
        if (message.kind === "event") deliver(message.value);
        else terminate();
      });
      await call("package_open_stream", { ...input, onEvent });
    },
    acknowledgeEvents: async (input) => { await call("package_acknowledge_events", input); },
    closeSubscription: async (input) => { await call("package_close_stream", input); },
    reportUiFault: async (input) => { await call("package_report_ui_fault", input); },
  };
  return Object.freeze(transport);
}

const tauriPackageBridge: PackageShellBridge = {
  transport: createTauriPackageHostTransport(),
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
