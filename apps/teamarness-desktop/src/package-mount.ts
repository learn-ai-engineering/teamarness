import {
  createPackageClient,
  type PackageMountContext,
  type PackageTransport,
  type PackageUiModule,
  type PackageUnmount,
} from "@teamarness/package-ui-sdk";
import type { OpenPackageMount, PackageInstanceKey } from "./package-contracts";

export const THEME_TOKENS = Object.freeze([
  "--teamarness-color-background",
  "--teamarness-color-panel",
  "--teamarness-color-text",
  "--teamarness-color-accent",
  "--teamarness-color-danger",
  "--teamarness-font-body",
  "--teamarness-radius-control",
] as const);

export interface PackageMountBridge {
  readonly transport: PackageTransport;
  openMount(instance: PackageInstanceKey): Promise<OpenPackageMount>;
  closeMount(input: Readonly<{ mount: string }>): Promise<void>;
  reportUiFault(input: Readonly<{ mount: string; code: "ui_exception" }>): Promise<void>;
  suspendBackend(instance: PackageInstanceKey): Promise<void>;
  recoverBackend(instance: PackageInstanceKey): Promise<void>;
  disposeBackend(instance: PackageInstanceKey): Promise<void>;
}

export type PackageMountDomLease = Readonly<{
  root: ShadowRoot;
  setThemeTokens(names: readonly string[]): void;
  remove(): void;
}>;

export interface PackageMountDom {
  create(target: unknown): PackageMountDomLease;
  watchErrors(moduleUrl: string, report: () => void): () => void;
}

export type PackageModuleLoader = (url: string) => Promise<PackageUiModule>;

export interface MountedPackageUi {
  unmount(): Promise<void>;
}

const browserDom: PackageMountDom = {
  create(target) {
    if (!(target instanceof HTMLElement)) throw new Error("Package pane is unavailable");
    const host = document.createElement("div");
    host.className = "package-ui-host";
    target.replaceChildren(host);
    const root = host.attachShadow({ mode: "open" });
    return {
      root,
      setThemeTokens(names) { host.dataset.themeTokens = names.join(" "); },
      remove() { host.remove(); },
    };
  },
  watchErrors(moduleUrl, report) {
    const onError = (event: ErrorEvent): void => {
      if (event.filename.startsWith(moduleUrl)) report();
    };
    const onRejection = (event: PromiseRejectionEvent): void => {
      const stack = event.reason instanceof Error ? event.reason.stack ?? "" : "";
      if (stack.includes(moduleUrl)) report();
    };
    window.addEventListener("error", onError);
    window.addEventListener("unhandledrejection", onRejection);
    return () => {
      window.removeEventListener("error", onError);
      window.removeEventListener("unhandledrejection", onRejection);
    };
  },
};

const loadModule: PackageModuleLoader = async (url) => import(/* @vite-ignore */ url) as Promise<PackageUiModule>;

export class PackageMountController {
  private current: MountedPackageUi | null = null;
  private transition: Promise<void> = Promise.resolve();

  public constructor(
    private readonly bridge: PackageMountBridge,
    private readonly dom: PackageMountDom = browserDom,
    private readonly load: PackageModuleLoader = loadModule,
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
    const domLease = this.dom.create(target);
    domLease.setThemeTokens(THEME_TOKENS);
    const moduleUrl = resourceUrl(opened);
    let packageUnmount: PackageUnmount = () => undefined;
    let closed = false;
    let faultReported = false;

    const recordFault = async (): Promise<void> => {
      if (faultReported || closed) return;
      faultReported = true;
      try {
        await this.bridge.reportUiFault({ mount: opened.mount, code: "ui_exception" });
      } finally {
        onFailure();
      }
    };
    const reportFault = (): void => { void recordFault().catch(() => undefined); };
    const scopedClient = createPackageClient(opened.mount, Object.freeze({
      ...this.bridge.transport,
      reportUiFault: async () => { await recordFault(); },
    }));
    const stopWatching = this.dom.watchErrors(moduleUrl, reportFault);

    const mounted: MountedPackageUi = Object.freeze({
      unmount: async (): Promise<void> => {
        if (closed) return;
        closed = true;
        try {
          await packageUnmount();
        } catch {
          if (!faultReported) {
            faultReported = true;
            await this.bridge.reportUiFault({ mount: opened.mount, code: "ui_exception" });
          }
        } finally {
          stopWatching();
          await scopedClient.dispose();
          await this.bridge.closeMount({ mount: opened.mount });
          domLease.remove();
          if (this.current === mounted) this.current = null;
        }
      },
    });
    this.current = mounted;

    try {
      const module = await this.load(moduleUrl);
      if (typeof module.mount !== "function") throw new Error("Package UI module has no mount function");
      const context: PackageMountContext = Object.freeze({
        root: domLease.root,
        package: Object.freeze({ ...opened.loader.package }),
        workspace: Object.freeze({ ...opened.loader.workspace }),
        uiHostVersion: opened.loader.uiHostVersion,
        themeTokens: THEME_TOKENS,
        client: scopedClient.client,
      });
      packageUnmount = await module.mount(context);
      if (typeof packageUnmount !== "function") throw new Error("Package UI mount did not return an unmount function");
      return mounted;
    } catch (error) {
      await recordFault();
      await mounted.unmount();
      throw error;
    }
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

function resourceUrl(opened: OpenPackageMount): string {
  return `teamarness-package://localhost/${opened.loader.token}/${opened.loader.entrypoint}`;
}
