import { afterEach, describe, expect, it } from "vitest";
import { TEAMARNESS_THEME_TOKENS } from "@teamarness/package-ui-sdk";
import type { OpenPackageMount, PackageHostTransport, PackageInstanceKey } from "./package-contracts";
import { CONNECT_KIND, type ThemeSnapshot } from "./package-frame/protocol";
import {
  browserFrameDom,
  FRAME_CONNECT_TIMEOUT_MS,
  PackageMountController,
  UNMOUNT_ACK_TIMEOUT_MS,
  type PackageFrameDom,
  type PackageFrameLease,
  type PackageFrameWindow,
  type PackageMountBridge,
  type PackageMountTimers,
  type PackageThemeSource,
} from "./package-mount";

const instance: PackageInstanceKey = {
  packageIdentity: "com.example.fixture",
  workspaceId: "00112233445566778899aabbccddeeff",
};

function transport(): PackageHostTransport {
  return {
    request: async () => null,
    cancelRequest: async () => undefined,
    openSubscription: async () => undefined,
    acknowledgeEvents: async () => undefined,
    closeSubscription: async () => undefined,
    reportUiFault: async () => undefined,
  };
}

function descriptor(number: number): OpenPackageMount {
  return {
    mount: `mount-${number}`,
    loader: {
      token: String(number).repeat(32),
      entrypoint: "ui/index.js",
      uiHostVersion: 1,
      contractDigest: "a".repeat(64),
      package: {
        identity: instance.packageIdentity,
        version: "1.0.0",
        displayName: "Fixture",
        source: "bundled",
        trust: "application_bundled",
        lifecycle: "active",
        processGeneration: 1,
      },
      workspace: { id: instance.workspaceId, name: "Fixture Workspace" },
    },
  };
}

function themeSnapshot(accent: string): ThemeSnapshot {
  const values: Record<string, string> = {};
  for (const token of TEAMARNESS_THEME_TOKENS) values[token] = "#101010";
  values["--teamarness-color-accent"] = accent;
  return { colorScheme: "light", values } as ThemeSnapshot;
}

type FakeFrame = {
  readonly source: Readonly<{ src: string; title: string }>;
  readonly posted: Array<Readonly<{ message: unknown; targetOrigin: string; transfer: unknown[] }>>;
  readonly lease: PackageFrameLease;
  detachWindow(): void;
  load(): void;
  port(): MessagePort;
};

function fakeFrame(source: Readonly<{ src: string; title: string }>, calls: string[]): FakeFrame {
  const posted: Array<Readonly<{ message: unknown; targetOrigin: string; transfer: unknown[] }>> = [];
  const listeners = new Set<() => void>();
  let windowAvailable = true;
  const lease: PackageFrameLease = Object.freeze({
    get contentWindow(): PackageFrameWindow | null {
      if (!windowAvailable) return null;
      return {
        postMessage: (message, targetOrigin, transfer) => {
          posted.push({ message, targetOrigin, transfer: [...transfer] });
        },
      };
    },
    onLoad(listener: () => void): () => void {
      listeners.add(listener);
      return () => { listeners.delete(listener); };
    },
    remove(): void { calls.push("remove-frame"); },
  });
  return {
    source,
    posted,
    lease,
    detachWindow: () => { windowAvailable = false; },
    load: () => { for (const listener of [...listeners]) listener(); },
    port: () => {
      const transferred = posted[0]?.transfer[0];
      if (transferred === undefined) throw new Error("the frame never received a port");
      return transferred as MessagePort;
    },
  };
}

type FakeTimers = {
  timers: PackageMountTimers;
  fire(ms: number): void;
  pending(): number[];
};

function fakeTimers(): FakeTimers {
  const scheduled = new Map<number, { ms: number; callback: () => void; cleared: boolean }>();
  let next = 1;
  return {
    timers: {
      setTimeout: (callback, ms) => {
        const handle = next++;
        scheduled.set(handle, { ms, callback, cleared: false });
        return handle;
      },
      clearTimeout: (handle) => {
        const entry = scheduled.get(handle as number);
        if (entry !== undefined) entry.cleared = true;
      },
    },
    fire(ms) {
      for (const entry of [...scheduled.values()]) {
        if (entry.ms !== ms || entry.cleared) continue;
        entry.cleared = true;
        entry.callback();
      }
    },
    pending: () => [...scheduled.values()].filter((entry) => !entry.cleared).map((entry) => entry.ms),
  };
}

type Harness = {
  bridge: PackageMountBridge;
  dom: PackageFrameDom;
  calls: string[];
  frames: FakeFrame[];
  timers: FakeTimers;
  theme: PackageThemeSource;
  pushTheme(theme: ThemeSnapshot): void;
  themeSubscribers(): number;
  controller(): PackageMountController;
};

const openPorts: MessagePort[] = [];

function harness(): Harness {
  const calls: string[] = [];
  const frames: FakeFrame[] = [];
  const timers = fakeTimers();
  const themeListeners = new Set<(theme: ThemeSnapshot) => void>();
  let opened = 0;
  const bridge: PackageMountBridge = {
    transport: transport(),
    openMount: async () => descriptor(++opened),
    closeMount: async ({ mount }) => { calls.push(`close:${mount}`); },
    reportUiFault: async ({ mount }) => { calls.push(`fault:${mount}`); },
    suspendBackend: async (selected) => { calls.push(`suspend:${selected.packageIdentity}`); },
    recoverBackend: async (selected) => { calls.push(`recover:${selected.packageIdentity}`); },
    disposeBackend: async (selected) => { calls.push(`dispose:${selected.packageIdentity}`); },
  };
  const dom: PackageFrameDom = {
    create: (_target, source) => {
      calls.push(`create-frame:${source.title}`);
      const frame = fakeFrame(source, calls);
      frames.push(frame);
      return frame.lease;
    },
  };
  const theme: PackageThemeSource = {
    current: () => themeSnapshot("#0b725d"),
    subscribe: (listener) => {
      themeListeners.add(listener);
      return () => { themeListeners.delete(listener); };
    },
  };
  const test: Harness = {
    bridge,
    dom,
    calls,
    frames,
    timers,
    theme,
    pushTheme: (snapshot) => { for (const listener of [...themeListeners]) listener(snapshot); },
    themeSubscribers: () => themeListeners.size,
    controller: () => new PackageMountController(bridge, dom, theme, timers.timers, () => {
      const channel = new MessageChannel();
      openPorts.push(channel.port1, channel.port2);
      return channel;
    }),
  };
  return test;
}

const settle = async (): Promise<void> => {
  for (let index = 0; index < 4; index += 1) await new Promise((resolve) => setTimeout(resolve, 0));
};

type ScriptedFrame = {
  received: Record<string, unknown>[];
  send(message: unknown): void;
  port: MessagePort;
};

function scriptFrame(frame: FakeFrame): ScriptedFrame {
  const port = frame.port();
  const received: Record<string, unknown>[] = [];
  port.onmessage = (event: MessageEvent) => { received.push(event.data as Record<string, unknown>); };
  port.start();
  return { received, port, send: (message) => { port.postMessage(message); } };
}

async function mountThrough(test: Harness, controller: PackageMountController, onFailure?: () => void): Promise<{
  mounted: Promise<{ unmount(): Promise<void> }>;
  frame: FakeFrame;
  scripted: ScriptedFrame;
}> {
  const mounted = controller.mount(instance, {}, onFailure);
  await settle();
  const frame = test.frames[test.frames.length - 1];
  if (frame === undefined) throw new Error("no frame was created");
  frame.load();
  const scripted = scriptFrame(frame);
  scripted.send({ kind: "hello", protocol: 1 });
  await settle();
  scripted.send({ kind: "mounted" });
  await settle();
  return { mounted, frame, scripted };
}

afterEach(() => {
  while (openPorts.length > 0) {
    const port = openPorts.pop();
    if (port === undefined) continue;
    port.onmessage = null;
    try {
      port.close();
    } catch {
      // A port the controller already closed needs no further close.
    }
  }
});

describe("generic Package UI mount", () => {
  it("creates one sandboxed frame per mount with allow-scripts set before its source", () => {
    const operations: string[] = [];
    class FakeElement {
      public replaceChildren(): void { operations.push("replaceChildren"); }
    }
    const iframe = {
      setAttribute: (name: string, value: string) => { operations.push(`attribute:${name}=${value}`); },
      addEventListener: (type: string) => { operations.push(`listen:${type}`); },
      removeEventListener: () => undefined,
      remove: () => undefined,
      contentWindow: null,
      set title(value: string) { operations.push(`title:${value}`); },
      set className(value: string) { operations.push(`class:${value}`); },
      set referrerPolicy(value: string) { operations.push(`referrer:${value}`); },
      set src(value: string) { operations.push(`src:${value}`); },
    };
    const globals = globalThis as unknown as Record<string, unknown>;
    const previousElement = globals.HTMLElement;
    const previousDocument = globals.document;
    globals.HTMLElement = FakeElement;
    globals.document = { createElement: () => iframe };
    try {
      const lease = browserFrameDom.create(new FakeElement(), {
        src: "teamarness-package://localhost/token/__teamarness/frame.html",
        title: "Fixture interface",
      });
      expect(lease).toBeDefined();
      expect(() => browserFrameDom.create({}, { src: "x", title: "y" })).toThrow("Package pane is unavailable");
    } finally {
      globals.HTMLElement = previousElement;
      globals.document = previousDocument;
    }

    expect(operations).toEqual([
      "attribute:sandbox=allow-scripts",
      "title:Fixture interface",
      "class:package-ui-frame",
      "referrer:no-referrer",
      "listen:load",
      "src:teamarness-package://localhost/token/__teamarness/frame.html",
      "replaceChildren",
    ]);
    expect(operations.indexOf("attribute:sandbox=allow-scripts"))
      .toBeLessThan(operations.findIndex((entry) => entry.startsWith("src:")));
  });

  it("transfers the port only after the first load and only to that frame's window", async () => {
    const test = harness();
    const controller = test.controller();
    const mounting = controller.mount(instance, {});
    await settle();

    const frame = test.frames[0];
    if (frame === undefined) throw new Error("no frame was created");
    expect(frame.source.src).toBe(`teamarness-package://localhost/${"1".repeat(32)}/__teamarness/frame.html`);
    expect(frame.source.title).toBe("Fixture interface");
    expect(frame.posted).toEqual([]);

    frame.load();
    expect(frame.posted).toHaveLength(1);
    expect(frame.posted[0]?.message).toEqual({ kind: CONNECT_KIND, protocol: 1 });
    expect(frame.posted[0]?.targetOrigin).toBe("*");
    expect(frame.posted[0]?.transfer).toHaveLength(1);

    const scripted = scriptFrame(frame);
    scripted.send({ kind: "hello", protocol: 1 });
    await settle();
    scripted.send({ kind: "mounted" });
    await mounting;
    expect(frame.posted).toHaveLength(1);
  });

  it("mounts with the generic context, entrypoint and current theme values", async () => {
    const test = harness();
    const controller = test.controller();
    const { mounted, scripted } = await mountThrough(test, controller);
    await mounted;

    expect(scripted.received).toHaveLength(1);
    expect(scripted.received[0]).toEqual({
      kind: "mount",
      entrypoint: "ui/index.js",
      package: {
        identity: instance.packageIdentity,
        version: "1.0.0",
        displayName: "Fixture",
        source: "bundled",
        trust: "application_bundled",
        lifecycle: "active",
        processGeneration: 1,
      },
      workspace: { id: instance.workspaceId, name: "Fixture Workspace" },
      uiHostVersion: 1,
      theme: themeSnapshot("#0b725d"),
    });
    expect(JSON.stringify(scripted.received)).not.toContain("1".repeat(32));
    expect(JSON.stringify(scripted.received)).not.toContain("mount-1");
  });

  it("pushes theme changes to the mounted frame and stops after unmount", async () => {
    const test = harness();
    const controller = test.controller();
    const { mounted, scripted } = await mountThrough(test, controller);
    const handle = await mounted;

    test.pushTheme(themeSnapshot("#7de2c4"));
    await settle();
    expect(scripted.received.at(-1)).toEqual({ kind: "theme", theme: themeSnapshot("#7de2c4") });

    const unmounting = handle.unmount();
    await settle();
    scripted.send({ kind: "unmounted", ok: true });
    await unmounting;
    expect(test.themeSubscribers()).toBe(0);

    test.pushTheme(themeSnapshot("#ffffff"));
    await settle();
    expect(scripted.received.filter((message) => message.kind === "theme")).toHaveLength(1);
  });

  it("treats a second frame load as a navigation fault without recovering the Backend", async () => {
    const test = harness();
    const controller = test.controller();
    let failures = 0;
    const { mounted, frame } = await mountThrough(test, controller, () => { failures += 1; });
    await mounted;

    frame.load();
    await settle();

    expect(failures).toBe(1);
    expect(test.calls).toContain("fault:mount-1");
    expect(test.calls).not.toContain("recover:com.example.fixture");
    expect(frame.posted).toHaveLength(1);
  });

  it("fails the mount when the frame does not say hello within the connect timeout", async () => {
    const test = harness();
    const controller = test.controller();
    let failures = 0;
    const mounting = controller.mount(instance, {}, () => { failures += 1; });
    await settle();
    expect(test.timers.pending()).toEqual([FRAME_CONNECT_TIMEOUT_MS]);

    test.frames[0]?.load();
    test.timers.fire(FRAME_CONNECT_TIMEOUT_MS);

    await expect(mounting).rejects.toThrow("connect_timeout");
    expect(failures).toBe(1);
    expect(test.calls).toEqual(["create-frame:Fixture interface", "fault:mount-1", "remove-frame", "close:mount-1"]);
  });

  it("unmounts through the frame, disposes the bridge, removes the frame, then closes the host mount", async () => {
    const test = harness();
    const controller = test.controller();
    const { mounted, scripted } = await mountThrough(test, controller);
    const handle = await mounted;

    const unmounting = handle.unmount();
    await settle();
    expect(scripted.received.at(-1)).toEqual({ kind: "unmount" });
    scripted.send({ kind: "unmounted", ok: true });
    await unmounting;

    expect(test.calls).toEqual(["create-frame:Fixture interface", "remove-frame", "close:mount-1"]);
    await expect(handle.unmount()).resolves.toBeUndefined();
    expect(test.calls.filter((entry) => entry.startsWith("close:"))).toEqual(["close:mount-1"]);
  });

  it("runs every local cleanup stage after unmount failures and retries only incomplete host close", async () => {
    const test = harness();
    let closeAttempts = 0;
    test.bridge.reportUiFault = async ({ mount }) => {
      test.calls.push(`fault:${mount}`);
      throw new Error("fault report failed");
    };
    test.bridge.closeMount = async ({ mount }) => {
      closeAttempts += 1;
      test.calls.push(`close:${mount}:${String(closeAttempts)}`);
      if (closeAttempts === 1) throw new Error("host close failed");
    };
    const controller = test.controller();
    const { mounted, scripted } = await mountThrough(test, controller);
    await mounted;

    const unmounting = controller.unmount();
    await settle();
    scripted.send({ kind: "unmounted", ok: false });
    await expect(unmounting).rejects.toThrow("Package UI did not acknowledge unmount");
    expect(test.calls).toEqual([
      "create-frame:Fixture interface",
      "fault:mount-1",
      "remove-frame",
      "close:mount-1:1",
    ]);

    await expect(controller.unmount()).resolves.toBeUndefined();
    expect(test.calls).toEqual([
      "create-frame:Fixture interface",
      "fault:mount-1",
      "remove-frame",
      "close:mount-1:1",
      "close:mount-1:2",
    ]);
  });

  it("reports an unmount acknowledgement timeout as a UI fault once", async () => {
    const test = harness();
    const controller = test.controller();
    let failures = 0;
    const { mounted } = await mountThrough(test, controller, () => { failures += 1; });
    const handle = await mounted;

    const unmounting = handle.unmount();
    await settle();
    expect(test.timers.pending()).toEqual([UNMOUNT_ACK_TIMEOUT_MS]);
    test.timers.fire(UNMOUNT_ACK_TIMEOUT_MS);

    await expect(unmounting).rejects.toThrow("Package UI did not acknowledge unmount");
    expect(failures).toBe(1);
    expect(test.calls.filter((entry) => entry === "fault:mount-1")).toHaveLength(1);
  });

  it("reports a frame fault once and surfaces it to the mount owner without recovering the Backend", async () => {
    const test = harness();
    const controller = test.controller();
    let failures = 0;
    const { mounted, scripted } = await mountThrough(test, controller, () => { failures += 1; });
    await mounted;

    scripted.send({ kind: "fault", source: "error" });
    scripted.send({ kind: "fault", source: "listener" });
    await settle();

    expect(failures).toBe(1);
    expect(test.calls.filter((entry) => entry === "fault:mount-1")).toHaveLength(1);
    expect(test.calls).not.toContain("recover:com.example.fixture");

    const broken = harness();
    const brokenController = broken.controller();
    const mounting = brokenController.mount(instance, {}, () => undefined);
    await settle();
    broken.frames[0]?.load();
    const brokenFrame = scriptFrame(broken.frames[0] as FakeFrame);
    brokenFrame.send({ kind: "hello", protocol: 1 });
    await settle();
    brokenFrame.send({ kind: "fault", source: "load" });
    await expect(mounting).rejects.toThrow("Package UI fault: load");
    expect(broken.calls).toContain("fault:mount-1");
  });

  it("routes generic lifecycle actions through the bridge for the supplied instance", async () => {
    const test = harness();
    const controller = test.controller();

    await controller.suspendBackend(instance);
    await controller.recoverBackend(instance);
    await controller.disposeBackend(instance);

    expect(test.calls).toEqual([
      "suspend:com.example.fixture",
      "recover:com.example.fixture",
      "dispose:com.example.fixture",
    ]);
  });

  it("replaces a mount without recovering its Backend", async () => {
    const test = harness();
    const controller = test.controller();
    const first = await mountThrough(test, controller);
    await first.mounted;

    const replacing = controller.mount(instance, {});
    await settle();
    first.scripted.send({ kind: "unmounted", ok: true });
    await settle();
    const second = test.frames[1];
    if (second === undefined) throw new Error("the replacement frame was never created");
    second.load();
    const secondFrame = scriptFrame(second);
    secondFrame.send({ kind: "hello", protocol: 1 });
    await settle();
    secondFrame.send({ kind: "mounted" });
    await replacing;

    expect(test.calls).toEqual([
      "create-frame:Fixture interface",
      "remove-frame",
      "close:mount-1",
      "create-frame:Fixture interface",
    ]);
    expect(test.calls).not.toContain("recover:com.example.fixture");

    const closing = controller.unmount();
    await settle();
    secondFrame.send({ kind: "unmounted", ok: true });
    await closing;
    expect(test.calls.filter((entry) => entry.startsWith("close:"))).toEqual(["close:mount-1", "close:mount-2"]);
  });

  it("installs no window-level listeners", async () => {
    const globals = globalThis as unknown as Record<string, unknown>;
    const previous = globals.addEventListener;
    const observed: string[] = [];
    globals.addEventListener = (type: string) => { observed.push(type); };
    try {
      const test = harness();
      const controller = test.controller();
      const { mounted, scripted } = await mountThrough(test, controller);
      const handle = await mounted;
      const unmounting = handle.unmount();
      await settle();
      scripted.send({ kind: "unmounted", ok: true });
      await unmounting;
    } finally {
      globals.addEventListener = previous;
    }
    expect(observed).toEqual([]);
  });

  it("faults the mount when the frame window is gone before the port is transferred", async () => {
    const test = harness();
    const controller = test.controller();
    let failures = 0;
    const mounting = controller.mount(instance, {}, () => { failures += 1; });
    await settle();
    test.frames[0]?.detachWindow();
    test.frames[0]?.load();

    await expect(mounting).rejects.toThrow("Package UI fault: navigation");
    expect(failures).toBe(1);
  });
});
