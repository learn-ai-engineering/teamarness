import { Channel, invoke } from "@tauri-apps/api/core";
import type { PackageValue } from "@teamarness/package-ui-sdk";
import type {
  HostUiEvent,
  InspectedPackage,
  OpenPackageMount,
  PackageAuthoringEntry,
  PackageChange,
  PackageManagementBridge,
  PackageHostSnapshot,
  PackageHostStateNotification,
  PackageHostTransport,
  PackageInstanceKey,
  PackageUiView,
} from "./package-contracts";

export interface PackageShellBridge {
  readonly transport: PackageHostTransport;
  snapshot(): Promise<PackageHostSnapshot>;
  subscribeHostState(afterRevision: number, listener: (event: PackageHostStateNotification) => void): Promise<void>;
  openMount(instance: PackageInstanceKey, view: PackageUiView): Promise<OpenPackageMount>;
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
        | Readonly<{ kind: "failure"; value: unknown }>
        | Readonly<{ kind: "completed"; value: Readonly<{ mount: string; subscription: string }> }>;
      const onEvent = makeChannel<StreamMessage>((message) => {
        if (message.kind === "event") deliver(message.value);
        else if (message.kind === "completed") terminate({ error: null });
        else terminate({ error: message.value });
      });
      await call("package_open_stream", { ...input, onEvent });
    },
    acknowledgeEvents: async (input) => { await call("package_acknowledge_events", input); },
    closeSubscription: async (input) => { await call("package_close_stream", input); },
    reportUiFault: async (input) => { await call("package_report_ui_fault", input); },
  };
  return Object.freeze(transport);
}

/** The Shell's Package Host commands for snapshots, mounts, and Instance lifecycle. */
export function createTauriPackageShellBridge(call: typeof invoke = invoke): PackageShellBridge {
  const shellBridge: PackageShellBridge = {
    transport: createTauriPackageHostTransport(call),
    snapshot: () => call<PackageHostSnapshot>("package_catalog_snapshot"),
    subscribeHostState: async (afterRevision, listener) => {
      const onEvent = new Channel(listener);
      await call("package_subscribe_host_state", { afterRevision, onEvent });
    },
    openMount: (instance, view) => call<OpenPackageMount>("package_open_mount", { instance, view }),
    closeMount: async ({ mount }) => { await call("package_close_mount", { mount }); },
    reportUiFault: async ({ mount, code }) => { await call("package_report_ui_fault", { mount, code }); },
    suspendBackend: async (instance) => { await call("package_suspend", { instance }); },
    recoverBackend: async (instance) => { await call("package_recover", { instance }); },
    disposeBackend: async (instance) => { await call("package_dispose", { instance }); },
  };
  return Object.freeze(shellBridge);
}

export const packageBridge = createTauriPackageShellBridge();

/** The Shell's own Package management commands. No Package UI Frame can reach them. */
export function createTauriPackageManagementBridge(call: typeof invoke = invoke): PackageManagementBridge {
  const management: PackageManagementBridge = {
    chooseDirectory: () => call<string | null>("package_choose_directory"),
    inspectDirectory: (directory: string) => call<InspectedPackage>("package_inspect_directory", { directory }),
    install: ({ directory, expectedIdentity, acceptedRisks }) =>
      call<PackageChange>("package_install", { directory, expectedIdentity, acceptedRisks: [...acceptedRisks] }),
    reload: (identity: string) => call<PackageChange>("package_reload", { identity }),
    remove: (identity: string) => call<PackageChange>("package_remove", { identity }),
    writeOverride: ({ directory, expectedIdentity, acceptedRisks }) =>
      call<PackageChange>("package_write_override", { directory, expectedIdentity, acceptedRisks: [...acceptedRisks] }),
    restart: async () => { await call("app_restart"); },
    restartPackage: async (instance) => { await call("package_restart", { instance }); },
    authoringCatalog: () => call<readonly PackageAuthoringEntry[]>("package_authoring_catalog"),
  };
  return Object.freeze(management);
}

export type { PackageValue };
