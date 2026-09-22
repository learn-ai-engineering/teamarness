import { afterEach, describe, expect, it } from "vitest";
import {
  TEAMARNESS_THEME_TOKENS,
  describePackageError,
  isPackageRequestError,
  packageErrorCode,
  type PackageClient,
  type PackageValue,
} from "@teamarness/package-ui-sdk";
import type { HostUiEvent, PackageHostTransport, PackageInstanceKey } from "../package-contracts";
import { PackagePortHost, type PackageUiFaultSource } from "../package-port-host";
import { PACKAGE_HOST_ELEMENT_ID, runPackageFrame, type FrameEnvironment } from "./runtime";
import { CONNECT_KIND, FRAME_BRIDGE_PROTOCOL, type ThemeSnapshot } from "./protocol";

const instance: PackageInstanceKey = {
  packageIdentity: "dev.teamarness.reference",
  workspaceId: "ff".repeat(16),
};

function themeSnapshot(): ThemeSnapshot {
  const values: Record<string, string> = {};
  for (const token of TEAMARNESS_THEME_TOKENS) values[token] = "#101010";
  return { colorScheme: "light", values } as ThemeSnapshot;
}

type HostCall = Readonly<{ method: string; mount: string; input: Record<string, unknown> }>;

function sharedTransport(): {
  transport: PackageHostTransport;
  calls: HostCall[];
  answer(request: string, value: PackageValue): void;
  reject(request: string, error: unknown): void;
  deliver(event: HostUiEvent): void;
} {
  const calls: HostCall[] = [];
  const pending = new Map<string, (value: PackageValue) => void>();
  const rejections = new Map<string, (error: unknown) => void>();
  const streams = new Map<string, (event: HostUiEvent) => void>();
  const record = (method: string, input: { mount: string }): void => {
    calls.push({ method, mount: input.mount, input: input as unknown as Record<string, unknown> });
  };
  return {
    calls,
    answer: (request, value) => { pending.get(request)?.(value); },
    reject: (request, error) => { rejections.get(request)?.(error); },
    deliver: (event) => { streams.get(`${event.mount}/${event.subscription}`)?.(event); },
    transport: {
      request: (input) => new Promise((resolve, rejectRequest) => {
        record("request", input);
        pending.set(input.request, resolve);
        rejections.set(input.request, rejectRequest);
      }),
      cancelRequest: async (input) => { record("cancelRequest", input); },
      openSubscription: async (input, deliver) => {
        record("openSubscription", input);
        streams.set(`${input.mount}/${input.subscription}`, deliver);
      },
      acknowledgeEvents: async (input) => { record("acknowledgeEvents", input); },
      closeSubscription: async (input) => { record("closeSubscription", input); },
      reportUiFault: async (input) => { record("reportUiFault", input); },
    },
  };
}

type Mount = {
  host: PackagePortHost;
  faults: PackageUiFaultSource[];
  client: Promise<PackageClient>;
  forge(message: unknown): void;
  close(): Promise<void>;
};

const mounts: Mount[] = [];

function mountFrame(
  mount: string,
  transport: PackageHostTransport,
  packageMount: (client: PackageClient) => void | Promise<void> = () => undefined,
): Mount {
  const channel = new MessageChannel();
  const faults: PackageUiFaultSource[] = [];
  const parent = { name: `parent-${mount}` };
  let resolveClient: (client: PackageClient) => void = () => undefined;
  const client = new Promise<PackageClient>((resolve) => { resolveClient = resolve; });
  const connectListeners: Array<(event: unknown) => void> = [];

  const env: FrameEnvironment = {
    window: {
      parent,
      addEventListener: (type, listener) => {
        if (type === "message") connectListeners.push(listener as (event: unknown) => void);
      },
      removeEventListener: () => undefined,
    },
    document: {
      getElementById: (id) => (id === PACKAGE_HOST_ELEMENT_ID
        ? { attachShadow: () => ({}) as unknown as ShadowRoot }
        : null),
      adoptedStyleSheets: [],
    },
    runtimeUrl: `teamarness-package://localhost/${"A".repeat(32)}/__teamarness/frame-runtime.js`,
    importModule: async () => ({
      mount: async (context: { client: PackageClient }) => {
        resolveClient(context.client);
        await packageMount(context.client);
        return () => undefined;
      },
    }),
    createStyleSheet: () => ({ replaceSync: () => undefined }) as unknown as CSSStyleSheet,
  };

  const host = new PackagePortHost({
    mount,
    instance,
    packageIdentity: instance.packageIdentity,
    port: channel.port1,
    transport,
    onFault: (source) => faults.push(source),
  });
  host.start();
  runPackageFrame(env);
  for (const listener of connectListeners) {
    listener({ source: parent, data: { kind: CONNECT_KIND, protocol: FRAME_BRIDGE_PROTOCOL }, ports: [channel.port2] });
  }

  const record: Mount = {
    host,
    faults,
    client,
    forge: (message) => { channel.port2.postMessage(message); },
    close: async () => {
      await host.dispose();
      channel.port2.close();
    },
  };
  mounts.push(record);
  return record;
}

const settle = async (): Promise<void> => {
  for (let index = 0; index < 6; index += 1) await new Promise((resolve) => setTimeout(resolve, 0));
};

async function sendMount(mount: Mount): Promise<void> {
  await mount.host.ready;
  mount.host.sendMount({
    entrypoint: "ui/index.js",
    package: {
      identity: instance.packageIdentity,
      version: "1.0.0",
      displayName: "Reference Package",
      source: "bundled",
      trust: "application_bundled",
      lifecycle: "active",
      processGeneration: "1",
    },
    workspace: { id: instance.workspaceId, name: "Workspace" },
    uiHostVersion: 1,
    theme: themeSnapshot(),
  });
  await mount.host.mounted;
}

afterEach(async () => {
  while (mounts.length > 0) await mounts.pop()?.close();
});

describe("Package UI frame and Shell host over a real channel", () => {
  it("round-trips a request, a cancelled request and an ordered acknowledged stream end to end", async () => {
    const host = sharedTransport();
    const mount = mountFrame("mount-a", host.transport);
    await sendMount(mount);
    const client = await mount.client;

    const answered = client.request("reference.read", { a: 1 });
    await settle();
    host.answer("r1", { counter: 9 });
    await expect(answered).resolves.toEqual({ counter: 9 });

    const controller = new AbortController();
    const cancelling = client.request("reference.read", {}, { signal: controller.signal });
    await settle();
    controller.abort();
    await expect(cancelling).rejects.toMatchObject({ name: "AbortError" });
    await settle();
    expect(host.calls.filter(({ method }) => method === "cancelRequest")).toEqual([
      { method: "cancelRequest", mount: "mount-a", input: { mount: "mount-a", request: "r2" } },
    ]);

    const seen: PackageValue[] = [];
    const subscription = await client.subscribe("reference.changes", {}, (event) => { seen.push(event); });
    await settle();
    host.deliver({ mount: "mount-a", subscription: "s1", sequence: "0", payload: { step: 1 } });
    host.deliver({ mount: "mount-a", subscription: "s1", sequence: "1", payload: { step: 2 } });
    await settle();

    expect(seen).toEqual([{ step: 1 }, { step: 2 }]);
    expect(host.calls.filter(({ method }) => method === "acknowledgeEvents").map(({ input }) => input.throughSequence))
      .toEqual(["0", "1"]);
    await subscription.close();
    await settle();
    expect(host.calls.at(-1)?.method).toBe("closeSubscription");
    expect(mount.faults).toEqual([]);
  });

  it("a Package error from the transport reaches the Package module as a typed error", async () => {
    const host = sharedTransport();
    const mount = mountFrame("mount-a", host.transport);
    await sendMount(mount);
    const client = await mount.client;

    const answered = client.request("reference.read", {});
    await settle();
    host.reject("r1", {
      version: 2,
      namespace: "teamarness.package-host",
      code: "backend_error",
      stage: "ui",
      affectedPackage: instance.packageIdentity,
      affectedInstance: instance,
      retryable: false,
      recovery: "none",
      diagnosticId: "diag-1",
      packageError: { code: "fixture.rejected", retryable: false, details: { field: "value" } },
    });

    const error = await answered.then(() => undefined, (reason: unknown) => reason);
    expect(isPackageRequestError(error)).toBe(true);
    expect(packageErrorCode(error)).toBe("fixture.rejected");
    expect(describePackageError(error)).toBe("fixture.rejected");
    expect(mount.faults).toEqual([]);
  });

  it("a Package module cannot reach another mount through forged messages", async () => {
    const host = sharedTransport();
    const first = mountFrame("mount-a", host.transport);
    const second = mountFrame("mount-b", host.transport);
    await sendMount(first);
    await sendMount(second);
    const client = await first.client;

    const answered = client.request("reference.read", {});
    await settle();
    first.forge({ kind: "request", id: 500, operation: "reference.read", payload: {}, mount: "mount-b" });
    first.forge({ kind: "subscribe", id: 501, operation: "reference.changes", payload: {}, mount: "mount-b" });
    await settle();
    host.answer("r1", null);
    await expect(answered).resolves.toBeNull();
    await settle();

    expect(new Set(host.calls.map(({ mount }) => mount))).toEqual(new Set(["mount-a"]));
    expect(host.calls.filter(({ mount }) => mount === "mount-b")).toEqual([]);
    expect(first.faults).toEqual(["protocol"]);
    expect(second.faults).toEqual([]);
  });
});
