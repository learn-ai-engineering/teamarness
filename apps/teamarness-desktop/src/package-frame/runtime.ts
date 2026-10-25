import {
  TEAMARNESS_THEME_TOKENS,
  type PackageMountContext,
  type PackageUiModule,
  type PackageUnmount,
} from "@teamarness/package-ui-sdk";
import { createPortPackageClient } from "./port-client";
import {
  CONNECT_KIND,
  FRAME_BRIDGE_PROTOCOL,
  parseShellMessage,
  type FrameFaultSource,
  type ShellToFrameMessage,
  type ThemeSnapshot,
} from "./protocol";

export const PACKAGE_HOST_ELEMENT_ID = "teamarness-package-host";

type FrameEventListener = (event: never) => void;

export type FrameWindow = Readonly<{
  parent: unknown;
  addEventListener(type: string, listener: FrameEventListener): void;
  removeEventListener(type: string, listener: FrameEventListener): void;
}>;

export type FrameHostElement = Readonly<{ attachShadow(init: Readonly<{ mode: "open" }>): ShadowRoot }>;

export type FrameDocument = {
  getElementById(id: string): FrameHostElement | null;
  adoptedStyleSheets: CSSStyleSheet[];
};

export type FrameEnvironment = Readonly<{
  window: FrameWindow;
  document: FrameDocument;
  runtimeUrl: string;
  importModule(url: string): Promise<unknown>;
  createStyleSheet(): CSSStyleSheet;
}>;

type PortLike = Readonly<{
  postMessage(message: unknown): void;
  addEventListener(type: "message", listener: (event: { data: unknown }) => void): void;
  start(): void;
}>;

type ConnectEvent = Readonly<{ source: unknown; data: unknown; ports: readonly PortLike[] }>;

function isConnect(data: unknown): boolean {
  if (typeof data !== "object" || data === null) return false;
  const message = data as Record<string, unknown>;
  return message.kind === CONNECT_KIND && message.protocol === FRAME_BRIDGE_PROTOCOL;
}

/**
 * Resolves the Package entrypoint against this runtime's own location and
 * refuses anything that leaves the mount's token directory.
 */
export function resolveEntrypointUrl(runtimeUrl: string, entrypoint: string): string {
  const base = new URL("../", runtimeUrl).href;
  const resolved = new URL(`../${entrypoint}`, runtimeUrl);
  if (resolved.search !== "" || resolved.hash !== "") throw new Error("Package entrypoint carries a selector");
  if (!resolved.href.startsWith(base)) throw new Error("Package entrypoint leaves its mount");
  return resolved.href;
}

function themeCss(theme: ThemeSnapshot): string {
  const declarations = TEAMARNESS_THEME_TOKENS.map((token) => `${token}: ${theme.values[token]};`);
  return `:root { color-scheme: ${theme.colorScheme}; ${declarations.join(" ")} }`;
}

/** Starts the Shell-owned frame runtime. It is the only script the Shell puts in a Package frame. */
export function runPackageFrame(env: FrameEnvironment): void {
  let connected = false;
  const onConnect = (event: ConnectEvent): void => {
    if (connected) return;
    if (event.source !== env.window.parent) return;
    if (!isConnect(event.data)) return;
    const ports = event.ports;
    if (!Array.isArray(ports) || ports.length !== 1) return;
    const port = ports[0];
    if (port === undefined) return;
    connected = true;
    env.window.removeEventListener("message", onConnect as FrameEventListener);
    startBridge(env, port);
  };
  env.window.addEventListener("message", onConnect as FrameEventListener);
}

function startBridge(env: FrameEnvironment, port: PortLike): void {
  let themeSheet: CSSStyleSheet | null = null;
  let mountStarted = false;
  let unmountStarted = false;
  let packageUnmount: PackageUnmount = () => undefined;
  const reported = new Set<FrameFaultSource>();

  const postFault = (source: FrameFaultSource): void => {
    if (reported.has(source)) return;
    reported.add(source);
    port.postMessage({ kind: "fault", source });
  };

  const bridge = createPortPackageClient(port, { onListenerFault: () => { postFault("listener"); } });

  const applyTheme = (theme: ThemeSnapshot): void => {
    const sheet = env.createStyleSheet();
    sheet.replaceSync(themeCss(theme));
    const previous = themeSheet;
    themeSheet = sheet;
    env.document.adoptedStyleSheets = [sheet, ...env.document.adoptedStyleSheets.filter((entry) => entry !== previous)];
  };

  const runMount = async (message: Extract<ShellToFrameMessage, { kind: "mount" }>): Promise<void> => {
    let root: ShadowRoot;
    let module: PackageUiModule;
    try {
      applyTheme(message.theme);
      const host = env.document.getElementById(PACKAGE_HOST_ELEMENT_ID);
      if (host === null) throw new Error("The Package frame document has no host element");
      root = host.attachShadow({ mode: "open" });
      const loaded = await env.importModule(resolveEntrypointUrl(env.runtimeUrl, message.entrypoint));
      if (typeof (loaded as PackageUiModule | null)?.mount !== "function") {
        throw new Error("Package UI module has no mount function");
      }
      module = loaded as PackageUiModule;
    } catch {
      postFault("load");
      return;
    }
    try {
      const context: PackageMountContext = Object.freeze({
        root,
        package: message.package,
        workspace: message.workspace,
        view: message.view,
        uiHostVersion: message.uiHostVersion,
        themeTokens: TEAMARNESS_THEME_TOKENS,
        client: bridge.client,
      });
      const unmount = await module.mount(context);
      if (typeof unmount !== "function") throw new Error("Package UI mount did not return an unmount function");
      packageUnmount = unmount;
      port.postMessage({ kind: "mounted" });
    } catch {
      postFault("mount");
    }
  };

  const runUnmount = async (): Promise<void> => {
    if (unmountStarted) return;
    unmountStarted = true;
    let ok = true;
    try {
      await packageUnmount();
    } catch {
      ok = false;
    }
    port.postMessage({ kind: "unmounted", ok });
    await bridge.dispose();
  };

  port.addEventListener("message", (event) => {
    const message = parseShellMessage(event.data);
    if (message === null) return;
    if (bridge.handle(message)) return;
    switch (message.kind) {
      case "mount":
        if (mountStarted) return;
        mountStarted = true;
        void runMount(message);
        return;
      case "theme":
        applyTheme(message.theme);
        return;
      case "unmount":
        void runUnmount();
        return;
      default:
        return;
    }
  });
  port.start();

  env.window.addEventListener("error", (() => { postFault("error"); }) as FrameEventListener);
  env.window.addEventListener("unhandledrejection", (() => { postFault("unhandledrejection"); }) as FrameEventListener);

  port.postMessage({ kind: "hello", protocol: FRAME_BRIDGE_PROTOCOL });
}
