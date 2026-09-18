import { describe, expect, it } from "vitest";
import { TEAMARNESS_THEME_TOKENS, type PackageMountContext, type PackageUiModule } from "@teamarness/package-ui-sdk";
import { PACKAGE_HOST_ELEMENT_ID, resolveEntrypointUrl, runPackageFrame, type FrameEnvironment } from "./runtime";
import { CONNECT_KIND, FRAME_BRIDGE_PROTOCOL, type ThemeSnapshot } from "./protocol";

const TOKEN = "A".repeat(32);
const RUNTIME_URL = `teamarness-package://localhost/${TOKEN}/__teamarness/frame-runtime.js`;

function themeSnapshot(accent = "#0b725d"): ThemeSnapshot {
  const values: Record<string, string> = {};
  for (const token of TEAMARNESS_THEME_TOKENS) values[token] = "#101010";
  values["--teamarness-color-accent"] = accent;
  return { colorScheme: "dark", values } as ThemeSnapshot;
}

class FakeSheet {
  public cssText = "";
  public replaceSync(text: string): void { this.cssText = text; }
}

type Harness = {
  env: FrameEnvironment;
  posted: Record<string, unknown>[];
  sheets: FakeSheet[];
  shadowRoots: object[];
  imported: string[];
  connect(overrides?: Readonly<{ source?: unknown; data?: unknown; ports?: unknown }>): void;
  toFrame(message: unknown): void;
  windowEvent(type: string): void;
  parent: object;
};

function harness(options: Readonly<{
  module?: PackageUiModule | null;
  host?: boolean;
  contexts?: PackageMountContext[];
}> = {}): Harness {
  const parent = { name: "parent" };
  const posted: Record<string, unknown>[] = [];
  const sheets: FakeSheet[] = [];
  const shadowRoots: object[] = [];
  const imported: string[] = [];
  const windowListeners = new Map<string, Array<(event: unknown) => void>>();
  const portListeners: Array<(event: { data: unknown }) => void> = [];

  const port = {
    postMessage: (message: unknown) => { posted.push(message as Record<string, unknown>); },
    addEventListener: (_type: "message", listener: (event: { data: unknown }) => void) => {
      portListeners.push(listener);
    },
    start: () => undefined,
  };

  const env: FrameEnvironment = {
    window: {
      parent,
      addEventListener: (type, listener) => {
        const listeners = windowListeners.get(type) ?? [];
        listeners.push(listener as (event: unknown) => void);
        windowListeners.set(type, listeners);
      },
      removeEventListener: (type, listener) => {
        const listeners = (windowListeners.get(type) ?? []).filter((entry) => entry !== listener);
        windowListeners.set(type, listeners);
      },
    },
    document: {
      getElementById: (id) => {
        if (options.host === false || id !== PACKAGE_HOST_ELEMENT_ID) return null;
        return {
          attachShadow: () => {
            const root = { id: `shadow-${shadowRoots.length}` };
            shadowRoots.push(root);
            return root as unknown as ShadowRoot;
          },
        };
      },
      adoptedStyleSheets: [],
    },
    runtimeUrl: RUNTIME_URL,
    importModule: async (url) => {
      imported.push(url);
      if (options.module === null) throw new Error("module unavailable");
      return options.module ?? { mount: () => () => undefined };
    },
    createStyleSheet: () => {
      const sheet = new FakeSheet();
      sheets.push(sheet);
      return sheet as unknown as CSSStyleSheet;
    },
  };

  runPackageFrame(env);

  return {
    env,
    posted,
    sheets,
    shadowRoots,
    imported,
    parent,
    connect(overrides = {}) {
      const event = {
        source: "source" in overrides ? overrides.source : parent,
        data: "data" in overrides ? overrides.data : { kind: CONNECT_KIND, protocol: FRAME_BRIDGE_PROTOCOL },
        ports: "ports" in overrides ? overrides.ports : [port],
      };
      for (const listener of [...(windowListeners.get("message") ?? [])]) listener(event);
    },
    toFrame(message) {
      for (const listener of [...portListeners]) listener({ data: message });
    },
    windowEvent(type) {
      for (const listener of [...(windowListeners.get(type) ?? [])]) listener({});
    },
  };
}

function mountMessage(entrypoint = "ui/index.js", theme = themeSnapshot()): unknown {
  return {
    kind: "mount",
    entrypoint,
    package: {
      identity: "dev.teamarness.reference",
      version: "1.0.0",
      displayName: "Reference Package",
      source: "bundled",
      trust: "application_bundled",
      lifecycle: "active",
      processGeneration: 1,
    },
    workspace: { id: "ff".repeat(16), name: "Workspace" },
    uiHostVersion: 1,
    theme,
  };
}

const settle = async (): Promise<void> => { await new Promise((resolve) => setTimeout(resolve, 0)); };

describe("Package UI frame runtime", () => {
  it("accepts the connect message only from the parent, only once and only with one port", () => {
    const test = harness();
    test.connect({ source: { name: "other" } });
    test.connect({ data: { kind: "teamarness.bridge.other", protocol: 1 } });
    test.connect({ data: { kind: CONNECT_KIND, protocol: 2 } });
    test.connect({ ports: [] });
    expect(test.posted).toEqual([]);

    test.connect();
    expect(test.posted).toEqual([{ kind: "hello", protocol: FRAME_BRIDGE_PROTOCOL }]);

    test.connect();
    expect(test.posted).toEqual([{ kind: "hello", protocol: FRAME_BRIDGE_PROTOCOL }]);
  });

  it("applies theme values through an adopted stylesheet before calling mount", async () => {
    const order: string[] = [];
    const test = harness({
      module: {
        mount: () => {
          order.push(`mount:${String(test.env.document.adoptedStyleSheets.length)}`);
          return () => undefined;
        },
      },
    });
    test.connect();
    test.toFrame(mountMessage());
    await settle();

    expect(order).toEqual(["mount:1"]);
    expect(test.sheets).toHaveLength(1);
    expect(test.sheets[0]?.cssText).toContain("color-scheme: dark");
    expect(test.sheets[0]?.cssText).toContain("--teamarness-color-accent: #0b725d;");
    expect(test.env.document.adoptedStyleSheets[0]).toBe(test.sheets[0]);
  });

  it("mounts into an open Shadow Root with the generic frozen context", async () => {
    const contexts: PackageMountContext[] = [];
    const test = harness({ module: { mount: (context) => { contexts.push(context); return () => undefined; } } });
    test.connect();
    test.toFrame(mountMessage());
    await settle();

    const context = contexts[0];
    expect(context).toBeDefined();
    expect(context?.root).toBe(test.shadowRoots[0]);
    expect(Object.isFrozen(context)).toBe(true);
    expect(context?.themeTokens).toBe(TEAMARNESS_THEME_TOKENS);
    expect(context?.uiHostVersion).toBe(1);
    expect(context?.package.displayName).toBe("Reference Package");
    expect(context?.workspace.name).toBe("Workspace");
    expect(JSON.stringify({ ...context, client: null, root: null })).not.toContain(TOKEN);
    expect(test.posted).toEqual([
      { kind: "hello", protocol: FRAME_BRIDGE_PROTOCOL },
      { kind: "mounted" },
    ]);
  });

  it("imports the entrypoint relative to its token path and refuses an escaping entrypoint", async () => {
    const test = harness();
    test.connect();
    test.toFrame(mountMessage("ui/nested/index.js"));
    await settle();
    expect(test.imported).toEqual([`teamarness-package://localhost/${TOKEN}/ui/nested/index.js`]);

    expect(resolveEntrypointUrl(RUNTIME_URL, "ui/index.js"))
      .toBe(`teamarness-package://localhost/${TOKEN}/ui/index.js`);
    for (const entrypoint of ["../other/index.js", "ui/index.js?pick=1", "ui/index.js#a"]) {
      expect(() => resolveEntrypointUrl(RUNTIME_URL, entrypoint)).toThrow();
    }

    const escaping = harness();
    escaping.connect();
    escaping.toFrame(mountMessage("../other-token/ui/index.js"));
    await settle();
    expect(escaping.imported).toEqual([]);
    expect(escaping.posted.at(-1)).toEqual({ kind: "fault", source: "load" });
  });

  it("reports load, mount and invalid-unmount failures as faults", async () => {
    const failedImport = harness({ module: null });
    failedImport.connect();
    failedImport.toFrame(mountMessage());
    await settle();
    expect(failedImport.posted.at(-1)).toEqual({ kind: "fault", source: "load" });

    const noMount = harness({ module: {} as PackageUiModule });
    noMount.connect();
    noMount.toFrame(mountMessage());
    await settle();
    expect(noMount.posted.at(-1)).toEqual({ kind: "fault", source: "load" });

    const noHost = harness({ host: false });
    noHost.connect();
    noHost.toFrame(mountMessage());
    await settle();
    expect(noHost.posted.at(-1)).toEqual({ kind: "fault", source: "load" });

    const failedMount = harness({ module: { mount: () => { throw new Error("mount failed"); } } });
    failedMount.connect();
    failedMount.toFrame(mountMessage());
    await settle();
    expect(failedMount.posted.at(-1)).toEqual({ kind: "fault", source: "mount" });

    const badUnmount = harness({ module: { mount: () => 7 as unknown as () => void } });
    badUnmount.connect();
    badUnmount.toFrame(mountMessage());
    await settle();
    expect(badUnmount.posted.at(-1)).toEqual({ kind: "fault", source: "mount" });
    expect(badUnmount.posted.filter((message) => message.kind === "mounted")).toEqual([]);
  });

  it("reports uncaught errors and unhandled rejections inside the frame once per source", async () => {
    const test = harness();
    test.connect();
    test.toFrame(mountMessage());
    await settle();

    test.windowEvent("error");
    test.windowEvent("error");
    test.windowEvent("unhandledrejection");
    test.windowEvent("unhandledrejection");

    expect(test.posted.filter((message) => message.kind === "fault")).toEqual([
      { kind: "fault", source: "error" },
      { kind: "fault", source: "unhandledrejection" },
    ]);
  });

  it("runs the Package unmount once and acknowledges it", async () => {
    let unmounts = 0;
    const test = harness({ module: { mount: () => () => { unmounts += 1; } } });
    test.connect();
    test.toFrame(mountMessage());
    await settle();

    test.toFrame({ kind: "unmount" });
    test.toFrame({ kind: "unmount" });
    await settle();

    expect(unmounts).toBe(1);
    expect(test.posted.filter((message) => message.kind === "unmounted")).toEqual([{ kind: "unmounted", ok: true }]);

    const failing = harness({ module: { mount: () => () => { throw new Error("unmount failed"); } } });
    failing.connect();
    failing.toFrame(mountMessage());
    await settle();
    failing.toFrame({ kind: "unmount" });
    await settle();
    expect(failing.posted.at(-1)).toEqual({ kind: "unmounted", ok: false });
  });

  it("replaces theme values without a Package-visible event", async () => {
    const events: string[] = [];
    const test = harness({
      module: {
        mount: (context) => {
          for (const key of Object.keys(context)) events.push(key);
          return () => undefined;
        },
      },
    });
    test.connect();
    test.toFrame(mountMessage());
    await settle();

    test.toFrame({ kind: "theme", theme: themeSnapshot("#7de2c4") });
    await settle();

    expect(test.sheets).toHaveLength(2);
    expect(test.env.document.adoptedStyleSheets).toEqual([test.sheets[1]]);
    expect(test.sheets[1]?.cssText).toContain("--teamarness-color-accent: #7de2c4;");
    expect(events).toEqual(["root", "package", "workspace", "uiHostVersion", "themeTokens", "client"]);
    expect(test.posted.filter((message) => message.kind === "fault")).toEqual([]);
  });
});
