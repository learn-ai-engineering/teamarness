import { afterEach, describe, expect, it } from "vitest";
import type { PackageValue } from "@teamarness/package-ui-sdk";
import type { HostUiEvent, PackageHostTransport, PackageInstanceKey } from "./package-contracts";
import { PackagePortHost, type PackageUiFaultSource } from "./package-port-host";
import {
  FRAME_BRIDGE_PROTOCOL,
  MAX_OPEN_SUBSCRIPTIONS,
  MAX_PAYLOAD_BYTES,
  MAX_PENDING_REQUESTS,
  type BridgeErrorEnvelope,
} from "./package-frame/protocol";
import bridgeFixture from "../contracts/fixtures/package-bridge.json";

const MOUNT = "mount-token-1";

/** The `packageErrorEnvelope` example in the shared desktop bridge fixture. */
const packageErrorEnvelope = bridgeFixture.packageErrorEnvelope as unknown as BridgeErrorEnvelope;
const TOKEN = "T".repeat(32);

/** A host envelope the Shell forwards unchanged, as the Tauri layer builds it. */
function hostEnvelope(code: string): BridgeErrorEnvelope {
  return {
    version: 2,
    namespace: "teamarness.package-host",
    code,
    stage: "ui",
    affectedPackage: instance.packageIdentity,
    affectedInstance: instance,
    retryable: false,
    recovery: "none",
    diagnosticId: "host-9",
    packageError: null,
  };
}
const instance: PackageInstanceKey = {
  packageIdentity: "dev.teamarness.reference",
  workspaceId: "ff".repeat(16),
};

type TransportCall = Readonly<{ method: string; input: Record<string, unknown> }>;

type Recorder = {
  transport: PackageHostTransport;
  calls: TransportCall[];
  answerRequest(request: string, value: PackageValue): void;
  failRequest(request: string, error: unknown): void;
  deliver(event: HostUiEvent): void;
  terminate(subscription: string, error?: unknown): void;
  failOpen(subscription: string, error: unknown): void;
  openFailures: Map<string, (error: unknown) => void>;
  /** Makes every later `acknowledgeEvents` reject, as a dead generation does. */
  refuseAcknowledgements(error: unknown): void;
};

function recorder(): Recorder {
  const calls: TransportCall[] = [];
  const pending = new Map<string, Readonly<{ resolve(value: PackageValue): void; reject(error: unknown): void }>>();
  const streams = new Map<
    string,
    Readonly<{ deliver(event: HostUiEvent): void; terminate(outcome: Readonly<{ error: unknown }>): void }>
  >();
  const openFailures = new Map<string, (error: unknown) => void>();
  let acknowledgementFailure: unknown;
  const record = (method: string, input: unknown): void => {
    calls.push({ method, input: input as Record<string, unknown> });
  };
  const transport: PackageHostTransport = {
    request: (input) => new Promise((resolve, reject) => {
      record("request", input);
      pending.set(input.request, Object.freeze({ resolve, reject }));
    }),
    cancelRequest: async (input) => { record("cancelRequest", input); },
    openSubscription: (input, deliver, terminate) => new Promise((resolve, reject) => {
      record("openSubscription", input);
      streams.set(input.subscription, Object.freeze({ deliver, terminate }));
      openFailures.set(input.subscription, reject);
      resolve();
    }),
    acknowledgeEvents: async (input) => {
      record("acknowledgeEvents", input);
      if (acknowledgementFailure !== undefined) throw acknowledgementFailure;
    },
    closeSubscription: async (input) => { record("closeSubscription", input); },
    reportUiFault: async (input) => { record("reportUiFault", input); },
  };
  return {
    transport,
    calls,
    answerRequest: (request, value) => { pending.get(request)?.resolve(value); },
    failRequest: (request, error) => { pending.get(request)?.reject(error); },
    deliver: (event) => { streams.get(event.subscription)?.deliver(event); },
    terminate: (subscription, error = null) => { streams.get(subscription)?.terminate({ error }); },
    failOpen: (subscription, error) => { openFailures.get(subscription)?.(error); },
    openFailures,
    refuseAcknowledgements: (error) => { acknowledgementFailure = error; },
  };
}

type Harness = {
  host: PackagePortHost;
  transport: Recorder;
  faults: PackageUiFaultSource[];
  received: Record<string, unknown>[];
  fromFrame(message: unknown): void;
  greet(): Promise<void>;
  dispose(): Promise<void>;
};

const open: Harness[] = [];

function harness(mount = MOUNT): Harness {
  const channel = new MessageChannel();
  const transport = recorder();
  const faults: PackageUiFaultSource[] = [];
  const received: Record<string, unknown>[] = [];
  const host = new PackagePortHost({
    mount,
    instance,
    packageIdentity: instance.packageIdentity,
    port: channel.port1,
    transport: transport.transport,
    onFault: (source) => faults.push(source),
  });
  channel.port2.onmessage = (event: MessageEvent) => { received.push(event.data as Record<string, unknown>); };
  channel.port2.start();
  host.start();
  const test: Harness = {
    host,
    transport,
    faults,
    received,
    fromFrame: (message) => { channel.port2.postMessage(message); },
    async greet() {
      channel.port2.postMessage({ kind: "hello", protocol: FRAME_BRIDGE_PROTOCOL });
      await host.ready;
      await settle();
    },
    async dispose() {
      await host.dispose();
      channel.port2.onmessage = null;
      channel.port2.close();
    },
  };
  open.push(test);
  return test;
}

const settle = async (): Promise<void> => {
  for (let index = 0; index < 4; index += 1) await new Promise((resolve) => setTimeout(resolve, 0));
};

afterEach(async () => {
  while (open.length > 0) await open.pop()?.dispose();
});

function scan(value: unknown, visit: (entry: unknown) => void): void {
  visit(value);
  if (Array.isArray(value)) {
    for (const entry of value) scan(entry, visit);
    return;
  }
  if (typeof value === "object" && value !== null) {
    for (const [key, entry] of Object.entries(value)) {
      visit(key);
      scan(entry, visit);
    }
  }
}

describe("Shell-side Package UI port host", () => {
  it("binds every host call to its own mount and treats messages carrying identity fields as protocol faults", async () => {
    const a = harness("mount-a");
    const b = harness("mount-b");
    await a.greet();
    await b.greet();

    a.fromFrame({ kind: "request", id: 1, operation: "reference.read", payload: {} });
    a.fromFrame({ kind: "request", id: 2, operation: "reference.read", payload: {}, mount: "mount-b" });
    await settle();

    expect(a.transport.calls.map(({ method, input }) => [method, input.mount])).toEqual([["request", "mount-a"]]);
    expect(b.transport.calls).toEqual([]);
    expect(a.faults).toEqual(["protocol"]);
  });

  it("forwards a request and returns the response to the frame", async () => {
    const test = harness();
    await test.greet();
    test.fromFrame({ kind: "request", id: 1, operation: "reference.read", payload: { a: 1 } });
    await settle();

    expect(test.transport.calls).toEqual([{
      method: "request",
      input: { mount: MOUNT, request: "r1", operation: "reference.read", payload: { a: 1 } },
    }]);
    test.transport.answerRequest("r1", { counter: 3 });
    await settle();
    expect(test.received).toEqual([{ kind: "response", id: 1, ok: true, value: { counter: 3 } }]);
    expect(test.faults).toEqual([]);
  });

  it("forwards host error envelopes unchanged and maps other rejections to ui_bridge_unavailable", async () => {
    const test = harness();
    await test.greet();
    const envelope: BridgeErrorEnvelope = {
      version: 1,
      namespace: "teamarness.package-host",
      code: "request_rejected",
      stage: "request",
      affectedPackage: instance.packageIdentity,
      affectedInstance: instance,
      retryable: false,
      recovery: "restart_package",
      diagnosticId: "host-7",
      packageError: null,
    };
    test.fromFrame({ kind: "request", id: 1, operation: "reference.read", payload: {} });
    test.fromFrame({ kind: "request", id: 2, operation: "reference.read", payload: {} });
    await settle();
    test.transport.failRequest("r1", envelope);
    test.transport.failRequest("r2", new Error("the Shell lost the command channel"));
    await settle();

    expect(test.received[0]).toEqual({ kind: "response", id: 1, ok: false, error: envelope });
    expect(test.received[1]).toEqual({
      kind: "response",
      id: 2,
      ok: false,
      error: {
        version: 2,
        namespace: "teamarness.desktop-ui-bridge",
        code: "ui_bridge_unavailable",
        stage: "ui",
        affectedPackage: instance.packageIdentity,
        affectedInstance: instance,
        retryable: true,
        recovery: "none",
        diagnosticId: "desktop-ui-bridge",
        packageError: null,
      },
    });
    expect(JSON.stringify(test.received)).not.toContain("lost the command channel");
  });

  it("answers invalid payloads and operations with ui_contract_violation without calling the host", async () => {
    const test = harness();
    await test.greet();
    test.fromFrame({ kind: "request", id: 1, operation: "", payload: {} });
    test.fromFrame({ kind: "request", id: 2, operation: "x".repeat(257), payload: {} });
    test.fromFrame({ kind: "request", id: 3, operation: "reference.read", payload: { a: "a".repeat(MAX_PAYLOAD_BYTES) } });
    test.fromFrame({ kind: "subscribe", id: 4, operation: "", payload: {} });
    await settle();

    expect(test.transport.calls).toEqual([]);
    expect(test.received.map((message) => [message.kind, message.id])).toEqual([
      ["response", 1], ["response", 2], ["response", 3], ["subscribe_failed", 4],
    ]);
    for (const message of test.received) {
      expect((message.error as BridgeErrorEnvelope).code).toBe("ui_contract_violation");
      expect((message.error as BridgeErrorEnvelope).retryable).toBe(false);
      expect((message.error as BridgeErrorEnvelope).namespace).toBe("teamarness.package-host");
    }
    expect(test.faults).toEqual([]);
  });

  it("answers with overloaded at the pending-request bound and refuses subscriptions past the open bound", async () => {
    const test = harness();
    await test.greet();
    for (let id = 1; id <= MAX_PENDING_REQUESTS; id += 1) {
      test.fromFrame({ kind: "request", id, operation: "reference.read", payload: {} });
    }
    test.fromFrame({ kind: "request", id: MAX_PENDING_REQUESTS + 1, operation: "reference.read", payload: {} });
    await settle();

    expect(test.transport.calls.filter(({ method }) => method === "request")).toHaveLength(MAX_PENDING_REQUESTS);
    expect(test.received).toHaveLength(1);
    expect(test.received[0]).toMatchObject({
      kind: "response",
      id: MAX_PENDING_REQUESTS + 1,
      ok: false,
      error: { code: "overloaded", retryable: true },
    });

    const streams = harness();
    await streams.greet();
    for (let index = 0; index < MAX_OPEN_SUBSCRIPTIONS; index += 1) {
      streams.fromFrame({ kind: "subscribe", id: 1_000 + index, operation: "reference.changes", payload: {} });
    }
    streams.fromFrame({ kind: "subscribe", id: 9_999, operation: "reference.changes", payload: {} });
    await settle();
    expect(streams.transport.calls.filter(({ method }) => method === "openSubscription"))
      .toHaveLength(MAX_OPEN_SUBSCRIPTIONS);
    expect(streams.received.at(-1)).toMatchObject({
      kind: "subscribe_failed",
      id: 9_999,
      error: { code: "overloaded", retryable: true },
    });
  });

  it("cancels a pending request on the host and drops its late response", async () => {
    const test = harness();
    await test.greet();
    test.fromFrame({ kind: "request", id: 1, operation: "reference.read", payload: {} });
    await settle();
    test.fromFrame({ kind: "cancel", id: 1 });
    await settle();

    expect(test.transport.calls.at(-1)).toEqual({
      method: "cancelRequest",
      input: { mount: MOUNT, request: "r1" },
    });
    test.transport.answerRequest("r1", { late: true });
    await settle();
    expect(test.received).toEqual([]);
    expect(test.faults).toEqual([]);
  });

  it("forwards only id, sequence and payload for stream events and acknowledges only delivered sequences", async () => {
    const test = harness();
    await test.greet();
    test.fromFrame({ kind: "subscribe", id: 1, operation: "reference.changes", payload: {} });
    await settle();
    expect(test.received).toEqual([{ kind: "subscribed", id: 1 }]);

    test.transport.deliver({ mount: MOUNT, subscription: "s1", sequence: "0", payload: { a: 1 } });
    test.transport.deliver({ mount: "mount-other", subscription: "s1", sequence: "1", payload: { b: 2 } });
    test.transport.deliver({ mount: MOUNT, subscription: "s9", sequence: "2", payload: { c: 3 } });
    await settle();
    expect(test.received.slice(1)).toEqual([{ kind: "event", id: 1, sequence: "0", payload: { a: 1 } }]);

    test.fromFrame({ kind: "ack", id: 1, sequence: "0" });
    await settle();
    expect(test.transport.calls.at(-1)).toEqual({
      method: "acknowledgeEvents",
      input: { mount: MOUNT, subscription: "s1", throughSequence: "0" },
    });
    expect(test.faults).toEqual([]);

    test.fromFrame({ kind: "ack", id: 1, sequence: "5" });
    await settle();
    expect(test.faults).toEqual(["protocol"]);
    expect(test.transport.calls.filter(({ method }) => method === "acknowledgeEvents")).toHaveLength(1);
  });

  it("keeps a refused acknowledgement owed and ends the stream instead of stalling it", async () => {
    const test = harness();
    await test.greet();
    test.fromFrame({ kind: "subscribe", id: 1, operation: "reference.changes", payload: {} });
    await settle();
    test.transport.deliver({ mount: MOUNT, subscription: "s1", sequence: "0", payload: { a: 1 } });
    await settle();

    // The Package Host could not take the acknowledgement. Recording it anyway
    // and discarding the rejection is what lost the credit permanently: this
    // host would refuse to ask again, and nothing anywhere said so.
    test.transport.refuseAcknowledgements(hostEnvelope("package.process_exited"));
    test.fromFrame({ kind: "ack", id: 1, sequence: "0" });
    await settle();
    expect(test.received.at(-1)).toMatchObject({ kind: "stream_terminated", id: 1 });
    expect(test.faults).toEqual(["stream_terminated"]);
  });

  it("drops the in-flight acknowledgement for a stream that has already completed", async () => {
    const test = harness();
    await test.greet();
    test.fromFrame({ kind: "subscribe", id: 1, operation: "reference.changes", payload: {} });
    await settle();
    test.transport.deliver({ mount: MOUNT, subscription: "s1", sequence: "0", payload: { a: 1 } });
    await settle();
    // An ordinary completion. The frame's acknowledgement for the last event
    // it received is already queued and arrives after the entry is gone.
    test.transport.terminate("s1");
    await settle();
    expect(test.received.at(-1)).toEqual({ kind: "stream_terminated", id: 1, error: null });

    test.fromFrame({ kind: "ack", id: 1, sequence: "0" });
    await settle();
    expect(test.faults).toEqual([]);
    expect(test.transport.calls.filter(({ method }) => method === "acknowledgeEvents")).toEqual([]);
  });

  it("drops the in-flight unsubscribe for a stream that has already completed", async () => {
    const test = harness();
    await test.greet();
    test.fromFrame({ kind: "subscribe", id: 1, operation: "reference.changes", payload: {} });
    await settle();
    // The documented teardown: the frame calls `subscription.close()` while the
    // `stream_terminated` for an ordinary completion is still in its queue, so
    // the `unsubscribe` names an entry this side has already released.
    test.transport.terminate("s1");
    await settle();
    expect(test.received.at(-1)).toEqual({ kind: "stream_terminated", id: 1, error: null });

    test.fromFrame({ kind: "unsubscribe", id: 1 });
    await settle();
    expect(test.faults).toEqual([]);
    expect(test.transport.calls.filter(({ method }) => method === "closeSubscription")).toHaveLength(1);
  });

  it("drops the in-flight cancel for a request that has already been answered", async () => {
    const test = harness();
    await test.greet();
    test.fromFrame({ kind: "request", id: 1, operation: "reference.read", payload: {} });
    await settle();
    // The frame's abort handler fires while the queued `response` is still
    // unprocessed, so its `cancel` names a request this side has retired.
    test.transport.answerRequest("r1", { ok: true });
    await settle();
    expect(test.received.at(-1)).toMatchObject({ kind: "response", id: 1, ok: true });

    test.fromFrame({ kind: "cancel", id: 1 });
    await settle();
    expect(test.faults).toEqual([]);
    expect(test.transport.calls.filter(({ method }) => method === "cancelRequest")).toEqual([]);
  });

  it("still faults the mount for a message naming an identifier it never issued", async () => {
    const test = harness();
    await test.greet();
    test.fromFrame({ kind: "unsubscribe", id: 7 });
    await settle();
    expect(test.faults).toEqual(["protocol"]);

    const requests = harness();
    await requests.greet();
    requests.fromFrame({ kind: "cancel", id: 7 });
    await settle();
    expect(requests.faults).toEqual(["protocol"]);
  });

  it("caps open subscriptions at the bound the Package Host enforces per mount", () => {
    // The Host refuses the ninth subscription on one mount as `overloaded`.
    expect(MAX_OPEN_SUBSCRIPTIONS).toBe(8);
  });

  it("still faults the mount for a host-owned stream failure", async () => {
    const test = harness();
    await test.greet();
    test.fromFrame({ kind: "subscribe", id: 1, operation: "reference.changes", payload: {} });
    await settle();

    test.transport.terminate("s1", hostEnvelope("event_lagged"));
    await settle();

    expect(test.transport.calls.at(-1)).toEqual({
      method: "closeSubscription",
      input: { mount: MOUNT, subscription: "s1" },
    });
    expect(test.received.at(-1)).toEqual({
      kind: "stream_terminated",
      id: 1,
      error: hostEnvelope("event_lagged"),
    });
    expect(test.faults).toEqual(["stream_terminated"]);
  });

  it("ends a subscription gracefully on completion without a fault", async () => {
    const test = harness();
    await test.greet();
    test.fromFrame({ kind: "subscribe", id: 1, operation: "reference.changes", payload: {} });
    await settle();

    test.transport.terminate("s1");
    await settle();

    expect(test.transport.calls.at(-1)).toEqual({
      method: "closeSubscription",
      input: { mount: MOUNT, subscription: "s1" },
    });
    expect(test.received.at(-1)).toEqual({ kind: "stream_terminated", id: 1, error: null });
    expect(test.faults).toEqual([]);
  });

  it("delivers a Package-owned stream failure to the frame without a fault and closes the host stream once", async () => {
    const test = harness();
    await test.greet();
    test.fromFrame({ kind: "subscribe", id: 1, operation: "reference.changes", payload: {} });
    await settle();

    const failure = {
      ...hostEnvelope("backend_error"),
      packageError: { code: "fixture.stream_rejected", retryable: false, details: {} },
    };
    test.transport.terminate("s1", failure);
    await settle();

    expect(test.transport.calls.filter(({ method }) => method === "closeSubscription")).toHaveLength(1);
    expect(test.received.at(-1)).toEqual({ kind: "stream_terminated", id: 1, error: failure });
    expect(test.faults).toEqual([]);
  });

  it("forwards the fixture Package error envelope to the frame unchanged", async () => {
    const test = harness();
    await test.greet();
    test.fromFrame({ kind: "request", id: 1, operation: "reference.read", payload: {} });
    await settle();
    test.transport.failRequest("r1", packageErrorEnvelope);
    await settle();

    expect(test.received.at(-1)).toEqual({
      kind: "response",
      id: 1,
      ok: false,
      error: packageErrorEnvelope,
    });
    expect(test.faults).toEqual([]);
  });

  it("reports protocol faults for unknown kinds, duplicate ids, messages before hello and bad acknowledgements", async () => {
    const early = harness();
    early.fromFrame({ kind: "request", id: 1, operation: "reference.read", payload: {} });
    await settle();
    expect(early.faults).toEqual(["protocol"]);
    expect(early.transport.calls).toEqual([]);

    const unknown = harness();
    await unknown.greet();
    unknown.fromFrame({ kind: "teamarness.bridge.connect", protocol: FRAME_BRIDGE_PROTOCOL });
    await settle();
    expect(unknown.faults).toEqual(["protocol"]);

    const duplicate = harness();
    await duplicate.greet();
    duplicate.fromFrame({ kind: "request", id: 1, operation: "reference.read", payload: {} });
    await settle();
    duplicate.fromFrame({ kind: "subscribe", id: 1, operation: "reference.changes", payload: {} });
    await settle();
    expect(duplicate.faults).toEqual(["protocol"]);
    expect(duplicate.transport.calls.filter(({ method }) => method === "openSubscription")).toEqual([]);

    const acknowledgement = harness();
    await acknowledgement.greet();
    acknowledgement.fromFrame({ kind: "ack", id: 7, sequence: "0" });
    await settle();
    expect(acknowledgement.faults).toEqual(["protocol"]);

    const greetedTwice = harness();
    await greetedTwice.greet();
    greetedTwice.fromFrame({ kind: "hello", protocol: FRAME_BRIDGE_PROTOCOL });
    await settle();
    expect(greetedTwice.faults).toEqual(["protocol"]);
  });

  it("sends and processes nothing after dispose and closes each open host stream once", async () => {
    const test = harness();
    await test.greet();
    test.fromFrame({ kind: "subscribe", id: 1, operation: "reference.changes", payload: {} });
    test.fromFrame({ kind: "subscribe", id: 2, operation: "reference.changes", payload: {} });
    test.fromFrame({ kind: "request", id: 3, operation: "reference.read", payload: {} });
    await settle();
    const before = test.received.length;

    await test.host.dispose();
    await test.host.dispose();
    await settle();

    expect(test.transport.calls.filter(({ method }) => method === "closeSubscription").map(({ input }) => input))
      .toEqual([{ mount: MOUNT, subscription: "s1" }, { mount: MOUNT, subscription: "s2" }]);
    test.transport.answerRequest("r3", { late: true });
    test.transport.deliver({ mount: MOUNT, subscription: "s1", sequence: "0", payload: {} });
    await settle();
    expect(test.received).toHaveLength(before);
    await expect(test.host.mounted).rejects.toThrow("Package UI mount closed before it mounted");
    await expect(test.host.requestUnmount()).resolves.toBe(false);
  });

  it("never posts channel objects, mount ids, tokens or host request ids to the frame", async () => {
    const test = harness();
    await test.greet();
    test.host.sendMount({
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
      view: "main",
      uiHostVersion: 2,
      theme: { colorScheme: "light", values: {} } as never,
    });
    test.fromFrame({ kind: "request", id: 1, operation: "reference.read", payload: {} });
    test.fromFrame({ kind: "subscribe", id: 2, operation: "reference.changes", payload: {} });
    await settle();
    test.transport.answerRequest("r1", { ok: true });
    test.transport.deliver({ mount: MOUNT, subscription: "s1", sequence: "0", payload: { a: 1 } });
    test.transport.terminate("s1");
    await settle();

    expect(test.received.length).toBeGreaterThan(3);
    for (const message of test.received) {
      scan(message, (entry) => {
        if (typeof entry === "string") {
          expect(entry).not.toContain(MOUNT);
          expect(entry).not.toContain(TOKEN);
          expect(entry).not.toMatch(/^[rs][0-9]+$/);
          return;
        }
        if (entry === null || Array.isArray(entry)) return;
        if (typeof entry === "object") {
          expect(Object.getPrototypeOf(entry)).toBe(Object.prototype);
          return;
        }
        expect(["boolean", "number"]).toContain(typeof entry);
      });
    }
  });
});
