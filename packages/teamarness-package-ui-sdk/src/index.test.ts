import type { Channel } from "@tauri-apps/api/core";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  createPackageClient,
  createTauriPackageTransport,
  installPackageUiKit,
  TEAMARNESS_THEME_TOKENS,
  type PackageTransport,
  type TeamarnessThemeToken,
  type UiEvent,
} from "./index";

const EXPECTED_THEME_TOKENS = [
  "--teamarness-color-background",
  "--teamarness-color-panel",
  "--teamarness-color-text",
  "--teamarness-color-text-muted",
  "--teamarness-color-border",
  "--teamarness-color-accent",
  "--teamarness-color-danger",
  "--teamarness-color-focus",
  "--teamarness-font-body",
  "--teamarness-font-mono",
  "--teamarness-radius-control",
  "--teamarness-radius-panel",
] as const satisfies readonly TeamarnessThemeToken[];

class FakeCSSStyleSheet {
  cssText = "";

  replaceSync(css: string): void {
    this.cssText = css;
  }
}

function fakeShadowRoot(initialSheets: CSSStyleSheet[] = []): ShadowRoot {
  return { adoptedStyleSheets: initialSheets } as unknown as ShadowRoot;
}

function deferred(): { promise: Promise<void>; resolve: () => void; reject: (reason: Error) => void } {
  let resolve: () => void = () => {};
  let reject: (reason: Error) => void = () => {};
  const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function fakeTransport(log: string[]): PackageTransport & {
  deliver?: (event: UiEvent) => void;
  terminate?: () => void;
} {
  const transport: PackageTransport & {
    deliver?: (event: UiEvent) => void;
    terminate?: () => void;
  } = {
    request: async () => ({ ok: true }),
    cancelRequest: async () => { log.push("cancel"); },
    openSubscription: async (_input, deliver, terminate) => {
      transport.deliver = deliver;
      transport.terminate = terminate;
    },
    acknowledgeEvents: async ({ throughSequence }) => { log.push(`ack:${throughSequence}`); },
    closeSubscription: async () => { log.push("close"); },
    reportUiFault: async () => { log.push("fault"); },
  };
  return transport;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("Package presentation contract", () => {
  it("publishes the exact frozen theme-token authority", () => {
    expect(TEAMARNESS_THEME_TOKENS).toEqual(EXPECTED_THEME_TOKENS);
    expect(Object.isFrozen(TEAMARNESS_THEME_TOKENS)).toBe(true);
  });

  it("adopts one class-scoped kit sheet and cleans up overlapping installers safely", () => {
    vi.stubGlobal("CSSStyleSheet", FakeCSSStyleSheet);
    const packageSheetBefore = new FakeCSSStyleSheet() as unknown as CSSStyleSheet;
    const packageSheetAfter = new FakeCSSStyleSheet() as unknown as CSSStyleSheet;
    const root = fakeShadowRoot([packageSheetBefore]);
    const cleanupFirst = installPackageUiKit(root);
    const cleanupSecond = installPackageUiKit(root);

    expect(root.adoptedStyleSheets).toHaveLength(2);
    expect(root.adoptedStyleSheets[0]).toBe(packageSheetBefore);
    const kitSheet = root.adoptedStyleSheets[1] as unknown as FakeCSSStyleSheet;
    expect(kitSheet.cssText).toMatch(/\.teamarness-control\s*\{[^}]*font:\s*inherit/);
    for (const className of [
      "teamarness-control",
      "teamarness-control--primary",
      "teamarness-control--secondary",
      "teamarness-field",
      "teamarness-input",
      "teamarness-help",
      "teamarness-validation",
      "teamarness-notice",
      "teamarness-notice--danger",
      "teamarness-panel",
      "teamarness-stack",
      "teamarness-inline",
      "teamarness-focus",
    ]) {
      expect(kitSheet.cssText).toContain(`.${className}`);
    }
    for (const className of [
      "teamarness-page",
      "teamarness-page--readable",
      "teamarness-page-header",
      "teamarness-eyebrow",
      "teamarness-page-title",
      "teamarness-page-subtitle",
      "teamarness-section",
      "teamarness-section-title",
    ]) {
      expect(kitSheet.cssText).toMatch(new RegExp(`\\.${className}\\s*\\{`));
    }
    for (const token of TEAMARNESS_THEME_TOKENS) expect(kitSheet.cssText).toContain(`var(${token})`);

    root.adoptedStyleSheets = [...root.adoptedStyleSheets, packageSheetAfter];
    cleanupFirst();
    cleanupFirst();
    expect(root.adoptedStyleSheets).toEqual([packageSheetBefore, kitSheet, packageSheetAfter]);
    cleanupSecond();
    cleanupSecond();
    expect(root.adoptedStyleSheets).toEqual([packageSheetBefore, packageSheetAfter]);
  });

  it("fails rather than adding an inline fallback when constructed sheets are unsupported", () => {
    vi.stubGlobal("CSSStyleSheet", undefined);
    expect(() => installPackageUiKit(fakeShadowRoot())).toThrow(/constructed CSSStyleSheet support/);

    vi.stubGlobal("CSSStyleSheet", class {});
    expect(() => installPackageUiKit(fakeShadowRoot())).toThrow(/replaceSync support/);

    vi.stubGlobal("CSSStyleSheet", FakeCSSStyleSheet);
    expect(() => installPackageUiKit({} as ShadowRoot)).toThrow(/adoptedStyleSheets support/);
    const immutableRoot = Object.defineProperty({}, "adoptedStyleSheets", {
      get: () => [],
      set: () => { throw new Error("read only"); },
    }) as ShadowRoot;
    expect(() => installPackageUiKit(immutableRoot)).toThrow(/mutable ShadowRoot\.adoptedStyleSheets support/);
  });
});

describe("mount-scoped Package client", () => {
  it("cancels a request through its mount without exposing another selector", async () => {
    const log: string[] = [];
    const transport = fakeTransport(log);
    transport.request = async () => new Promise(() => undefined);
    const { client } = createPackageClient("mount-1", transport);
    const abort = new AbortController();
    const request = client.request("fixture.read", {}, { signal: abort.signal });
    abort.abort();
    await expect(request).rejects.toMatchObject({ name: "AbortError" });
    expect(log).toEqual(["cancel"]);
  });

  it("delivers events serially and acknowledges only after each listener resolves", async () => {
    const log: string[] = [];
    const first = deferred();
    const transport = fakeTransport(log);
    const { client } = createPackageClient("mount-1", transport);
    await client.subscribe("fixture.changes", {}, async (payload) => {
      log.push(`listen:${String((payload as { order: number }).order)}`);
      if ((payload as { order: number }).order === 1) await first.promise;
      log.push(`done:${String((payload as { order: number }).order)}`);
    });

    transport.deliver?.({ mount: "mount-1", subscription: "subscription-1", sequence: "1", payload: { order: 1 } });
    transport.deliver?.({ mount: "mount-1", subscription: "subscription-1", sequence: "2", payload: { order: 2 } });
    await Promise.resolve();
    expect(log).toEqual(["listen:1"]);
    first.resolve();
    await first.promise;
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(log).toEqual(["listen:1", "done:1", "ack:1", "listen:2", "done:2", "ack:2"]);
  });

  it("closes without acknowledgement and reports a rejected listener once", async () => {
    const log: string[] = [];
    const transport = fakeTransport(log);
    const { client } = createPackageClient("mount-1", transport);
    await client.subscribe("fixture.changes", {}, async () => { throw new Error("listener failed"); });
    transport.deliver?.({ mount: "mount-1", subscription: "subscription-1", sequence: "1", payload: {} });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(log).toEqual(["close", "fault"]);
  });

  it("turns a terminal Backend stream failure into mount failure without acknowledgement", async () => {
    const log: string[] = [];
    const transport = fakeTransport(log);
    const { client } = createPackageClient("mount-1", transport);
    await client.subscribe("fixture.changes", {}, async () => { log.push("listen"); });
    transport.terminate?.();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(log).toEqual(["close", "fault"]);
  });

  it("maps a terminal Tauri stream message to termination rather than delivery", async () => {
    let receive: ((message: unknown) => void) | undefined;
    const transport = createTauriPackageTransport(
      async () => undefined as never,
      <T>(handler: (message: T) => void): Channel<T> => {
        receive = (message) => { handler(message as T); };
        return {} as Channel<T>;
      },
    );
    const log: string[] = [];

    await transport.openSubscription(
      { mount: "mount-1", subscription: "subscription-1", operation: "fixture.changes", payload: {} },
      () => { log.push("deliver"); },
      () => { log.push("terminate"); },
    );
    receive?.({ kind: "failure", value: { code: "event_lagged" } });

    expect(log).toEqual(["terminate"]);
  });

  it("makes subscription and mount cleanup idempotent", async () => {
    const log: string[] = [];
    const transport = fakeTransport(log);
    const { client, dispose } = createPackageClient("mount-1", transport);
    const subscription = await client.subscribe("fixture.changes", {}, async () => undefined);
    await subscription.close();
    await subscription.close();
    await dispose();
    await dispose();
    expect(log).toEqual(["close"]);
  });
});
