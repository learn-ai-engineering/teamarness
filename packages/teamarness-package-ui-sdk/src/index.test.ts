import type { Channel } from "@tauri-apps/api/core";
import { describe, expect, it } from "vitest";
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

function fakeShadowRoot(): ShadowRoot & { readonly children: HTMLElement[] } {
  const children: HTMLElement[] = [];
  const ownerDocument = {
    createElement: () => {
      const attributes = new Map<string, string>();
      const element = {
        textContent: "",
        setAttribute: (name: string, value: string) => { attributes.set(name, value); },
        hasAttribute: (name: string) => attributes.has(name),
        remove: () => {
          const index = children.indexOf(element as unknown as HTMLElement);
          if (index >= 0) children.splice(index, 1);
        },
      };
      return element as unknown as HTMLStyleElement;
    },
  };
  return {
    ownerDocument,
    children,
    append: (element: HTMLElement) => { children.push(element); },
  } as unknown as ShadowRoot & { readonly children: HTMLElement[] };
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

describe("Package presentation contract", () => {
  it("publishes the exact frozen theme-token authority", () => {
    expect(TEAMARNESS_THEME_TOKENS).toEqual(EXPECTED_THEME_TOKENS);
    expect(Object.isFrozen(TEAMARNESS_THEME_TOKENS)).toBe(true);
  });

  it("installs one marked, class-scoped style and cleans up overlapping installers safely", () => {
    const root = fakeShadowRoot();
    const cleanupFirst = installPackageUiKit(root);
    const cleanupSecond = installPackageUiKit(root);

    expect(root.children).toHaveLength(1);
    const style = root.children[0]!;
    expect(style.hasAttribute("data-teamarness-ui-kit")).toBe(true);
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
      expect(style.textContent).toContain(`.${className}`);
    }
    for (const token of TEAMARNESS_THEME_TOKENS) expect(style.textContent).toContain(`var(${token})`);

    cleanupFirst();
    cleanupFirst();
    expect(root.children).toHaveLength(1);
    cleanupSecond();
    cleanupSecond();
    expect(root.children).toHaveLength(0);
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
