import { TEAMARNESS_THEME_TOKENS, type TeamarnessThemeToken } from "@teamarness/package-ui-sdk";
import type { OpenPackageMount, PackageHostTransport, PackageInstanceKey } from "./package-contracts";
import { CONNECT_KIND, FRAME_BRIDGE_PROTOCOL, type ThemeSnapshot } from "./package-frame/protocol";
import { PackagePortHost, type PackageUiFaultSource } from "./package-port-host";
import { browserThemeSource, type PackageThemeSource } from "./package-theme";

export type { PackageThemeSource } from "./package-theme";

export const FRAME_CONNECT_TIMEOUT_MS = 10_000;
export const UNMOUNT_ACK_TIMEOUT_MS = 5_000;

export interface PackageMountBridge {
  readonly transport: PackageHostTransport;
  openMount(instance: PackageInstanceKey): Promise<OpenPackageMount>;
  closeMount(input: Readonly<{ mount: string }>): Promise<void>;
  reportUiFault(input: Readonly<{ mount: string; code: "ui_exception" }>): Promise<void>;
  suspendBackend(instance: PackageInstanceKey): Promise<void>;
  recoverBackend(instance: PackageInstanceKey): Promise<void>;
  disposeBackend(instance: PackageInstanceKey): Promise<void>;
}

export type PackageFrameWindow = Readonly<{
  postMessage(message: unknown, targetOrigin: string, transfer: Transferable[]): void;
}>;

export type PackageFrameLease = Readonly<{
  readonly contentWindow: PackageFrameWindow | null;
  onLoad(listener: () => void): () => void;
  remove(): void;
}>;

export interface PackageFrameDom {
  create(target: unknown, frame: Readonly<{ src: string; title: string }>): PackageFrameLease;
}

export type PackageMountTimers = Readonly<{
  setTimeout(callback: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}>;

export interface MountedPackageUi {
  unmount(): Promise<void>;
}

export const browserFrameDom: PackageFrameDom = {
  create(target, frame) {
    if (!(target instanceof HTMLElement)) throw new Error("Package pane is unavailable");
    const iframe = document.createElement("iframe");
    iframe.setAttribute("sandbox", "allow-scripts");
    iframe.title = frame.title;
    iframe.className = "package-ui-frame";
    iframe.referrerPolicy = "no-referrer";
    const listeners = new Set<() => void>();
    const onLoad = (): void => { for (const listener of [...listeners]) listener(); };
    iframe.addEventListener("load", onLoad);
    iframe.src = frame.src;
    target.replaceChildren(iframe);
    return Object.freeze({
      get contentWindow(): PackageFrameWindow | null {
        return iframe.contentWindow as PackageFrameWindow | null;
      },
      onLoad(listener: () => void): () => void {
        listeners.add(listener);
        return () => { listeners.delete(listener); };
      },
      remove(): void {
        iframe.removeEventListener("load", onLoad);
        iframe.remove();
      },
    });
  },
};

const globalTimers: PackageMountTimers = Object.freeze({
  setTimeout: (callback: () => void, ms: number) => setTimeout(callback, ms),
  clearTimeout: (handle: unknown) => { clearTimeout(handle as ReturnType<typeof setTimeout>); },
});

const emptyThemeValues = Object.freeze(
  Object.fromEntries(TEAMARNESS_THEME_TOKENS.map((token) => [token, ""])),
) as Readonly<Record<TeamarnessThemeToken, string>>;

const staticThemeSource: PackageThemeSource = Object.freeze({
  current: (): ThemeSnapshot => Object.freeze({ colorScheme: "light", values: emptyThemeValues }),
  subscribe: (): (() => void) => () => undefined,
});

export class PackageMountController {
  private current: MountedPackageUi | null = null;
  private transition: Promise<void> = Promise.resolve();

  public constructor(
    private readonly bridge: PackageMountBridge,
    private readonly dom: PackageFrameDom = browserFrameDom,
    private readonly theme: PackageThemeSource = staticThemeSource,
    private readonly timers: PackageMountTimers = globalTimers,
    private readonly createChannel: () => MessageChannel = () => new MessageChannel(),
  ) {}

  public mount(
    instance: PackageInstanceKey,
    target: unknown,
    onFailure: () => void = () => undefined,
  ): Promise<MountedPackageUi> {
    const operation = this.transition.then(() => this.mountNow(instance, target, onFailure));
    this.transition = operation.then(() => undefined, () => undefined);
    return operation;
  }

  private async mountNow(
    instance: PackageInstanceKey,
    target: unknown,
    onFailure: () => void,
  ): Promise<MountedPackageUi> {
    await this.current?.unmount();
    const opened = await this.bridge.openMount(instance);
    const loader = opened.loader;

    let host: PackagePortHost | null = null;
    let frameLoaded = false;
    let connectTimer: unknown = null;
    let stopTheme: (() => void) | null = null;
    let faultReported = false;
    let hostClosed = false;
    let unmountRequested = false;
    let themeStopped = false;
    let hostDisposed = false;
    let frameRemoved = false;
    let settleMounted: ((error?: unknown) => void) | null = null;
    const mounting = new Promise<void>((resolve, reject) => {
      settleMounted = (error) => { if (error === undefined) resolve(); else reject(error); };
    });
    mounting.catch(() => undefined);

    const clearConnectTimer = (): void => {
      if (connectTimer === null) return;
      this.timers.clearTimeout(connectTimer);
      connectTimer = null;
    };
    const recordFault = async (): Promise<void> => {
      if (faultReported || hostClosed) return;
      faultReported = true;
      try {
        await this.bridge.reportUiFault({ mount: opened.mount, code: "ui_exception" });
      } finally {
        onFailure();
      }
    };
    const reportFault = (source: PackageUiFaultSource): void => {
      console.warn("Package UI fault", { source });
      clearConnectTimer();
      settleMounted?.(new Error(`Package UI fault: ${source}`));
      settleMounted = null;
      if (!hostDisposed) {
        hostDisposed = true;
        void host?.dispose().catch(() => undefined);
      }
      void recordFault().catch(() => undefined);
    };

    const frame = this.dom.create(target, {
      src: `teamarness-package://localhost/${loader.token}/__teamarness/frame.html`,
      title: `${loader.package.displayName} interface`,
    });
    const stopWatchingLoad = frame.onLoad(() => {
      if (frameLoaded) {
        reportFault("navigation");
        return;
      }
      frameLoaded = true;
      if (hostDisposed) return;
      const frameWindow = frame.contentWindow;
      if (frameWindow === null) {
        reportFault("navigation");
        return;
      }
      const channel = this.createChannel();
      const started = new PackagePortHost({
        mount: opened.mount,
        instance,
        packageIdentity: loader.package.identity,
        port: channel.port1,
        transport: this.bridge.transport,
        onFault: reportFault,
      });
      host = started;
      started.start();
      started.mounted.then(() => { settleMounted?.(); settleMounted = null; }, (error: unknown) => {
        settleMounted?.(error);
        settleMounted = null;
      });
      frameWindow.postMessage({ kind: CONNECT_KIND, protocol: FRAME_BRIDGE_PROTOCOL }, "*", [channel.port2]);
      void started.ready.then(() => {
        clearConnectTimer();
        if (hostDisposed) return;
        started.sendMount({
          entrypoint: loader.entrypoint,
          package: loader.package,
          workspace: loader.workspace,
          uiHostVersion: loader.uiHostVersion,
          theme: this.theme.current(),
        });
        stopTheme = this.theme.subscribe((theme) => { started.sendTheme(theme); });
      }, () => undefined);
    });
    connectTimer = this.timers.setTimeout(() => {
      connectTimer = null;
      reportFault("connect_timeout");
    }, FRAME_CONNECT_TIMEOUT_MS);

    const mounted: MountedPackageUi = Object.freeze({
      unmount: async (): Promise<void> => {
        if (hostClosed) return;
        let firstError: unknown;
        const capture = async (stage: () => void | Promise<void>): Promise<void> => {
          try {
            await stage();
          } catch (error) {
            firstError ??= error;
          }
        };

        if (!unmountRequested) {
          unmountRequested = true;
          clearConnectTimer();
          const current = host;
          if (current !== null && !hostDisposed) {
            const acknowledged = await this.awaitUnmount(current);
            if (!acknowledged) {
              firstError ??= new Error("Package UI did not acknowledge unmount");
              await capture(recordFault);
            }
          }
        }
        if (!themeStopped) {
          themeStopped = true;
          await capture(() => { stopTheme?.(); stopTheme = null; });
        }
        if (!hostDisposed) {
          hostDisposed = true;
          await capture(async () => { await host?.dispose(); });
        }
        if (!frameRemoved) {
          frameRemoved = true;
          await capture(() => { stopWatchingLoad(); frame.remove(); });
        }
        await capture(async () => {
          await this.bridge.closeMount({ mount: opened.mount });
          hostClosed = true;
        });
        if (hostClosed && this.current === mounted) this.current = null;
        if (firstError !== undefined) throw firstError;
      },
    });
    this.current = mounted;

    try {
      await mounting;
      return mounted;
    } catch (error) {
      try {
        await recordFault();
      } catch {
        // Preserve the mount failure while cleanup continues.
      }
      try {
        await mounted.unmount();
      } catch {
        // Preserve the mount failure after all cleanup stages have run.
      }
      throw error;
    }
  }

  private awaitUnmount(host: PackagePortHost): Promise<boolean> {
    return new Promise<boolean>((resolve) => {
      let settled = false;
      const handle = this.timers.setTimeout(() => {
        if (settled) return;
        settled = true;
        resolve(false);
      }, UNMOUNT_ACK_TIMEOUT_MS);
      const finish = (acknowledged: boolean): void => {
        if (settled) return;
        settled = true;
        this.timers.clearTimeout(handle);
        resolve(acknowledged);
      };
      void host.requestUnmount().then(finish, () => { finish(false); });
    });
  }

  public unmount(): Promise<void> {
    const operation = this.transition.then(async () => { await this.current?.unmount(); });
    this.transition = operation.then(() => undefined, () => undefined);
    return operation;
  }

  public async suspendBackend(instance: PackageInstanceKey): Promise<void> {
    await this.bridge.suspendBackend(instance);
  }

  public async recoverBackend(instance: PackageInstanceKey): Promise<void> {
    await this.bridge.recoverBackend(instance);
  }

  public async disposeBackend(instance: PackageInstanceKey): Promise<void> {
    await this.bridge.disposeBackend(instance);
  }
}

export { browserThemeSource };
