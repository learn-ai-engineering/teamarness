import { describe, expect, it } from "vitest";
import { createPortPackageClient, type PortPackageClient } from "./port-client";
import { parseShellMessage, type BridgeErrorEnvelope, type ShellToFrameMessage } from "./protocol";

const envelope: BridgeErrorEnvelope = Object.freeze({
  version: 1,
  namespace: "teamarness.package-host",
  code: "ui_contract_violation",
  stage: "ui",
  affectedPackage: "dev.teamarness.reference",
  affectedInstance: Object.freeze({ packageIdentity: "dev.teamarness.reference", workspaceId: "ff".repeat(16) }),
  retryable: false,
  recovery: "none",
  diagnosticId: "desktop-ui-bridge",
});

function harness(): {
  bridge: PortPackageClient;
  posted: Record<string, unknown>[];
  faults: number;
  deliver(message: unknown): boolean;
} {
  const posted: Record<string, unknown>[] = [];
  const state = { faults: 0 };
  const bridge = createPortPackageClient(
    { postMessage: (message) => { posted.push(message as Record<string, unknown>); } },
    { onListenerFault: () => { state.faults += 1; } },
  );
  return {
    bridge,
    posted,
    get faults() { return state.faults; },
    deliver(message) {
      const parsed = parseShellMessage(message);
      if (parsed === null) throw new Error("the test posted an invalid Shell message");
      return bridge.handle(parsed as ShellToFrameMessage);
    },
  };
}

const settle = async (): Promise<void> => { await new Promise((resolve) => setTimeout(resolve, 0)); };

describe("port-backed Package client", () => {
  it("sends a request and resolves the matching response", async () => {
    const test = harness();
    const answered = test.bridge.client.request("reference.read", { a: 1 });
    expect(test.posted).toEqual([{ kind: "request", id: 1, operation: "reference.read", payload: { a: 1 } }]);
    test.deliver({ kind: "response", id: 1, ok: true, value: { counter: 4 } });
    await expect(answered).resolves.toEqual({ counter: 4 });
    expect(test.deliver({ kind: "response", id: 1, ok: true, value: null })).toBe(true);
  });

  it("rejects with a frozen copy of the forwarded error envelope", async () => {
    const test = harness();
    const answered = test.bridge.client.request("reference.read", {});
    test.deliver({ kind: "response", id: 1, ok: false, error: envelope });
    const error = await answered.then(() => undefined, (reason: unknown) => reason);
    expect(error).toEqual(envelope);
    expect(Object.isFrozen(error)).toBe(true);
    expect(error).not.toBe(envelope);
  });

  it("cancels through the port and rejects with AbortError", async () => {
    const test = harness();
    const controller = new AbortController();
    const answered = test.bridge.client.request("reference.read", {}, { signal: controller.signal });
    controller.abort();
    await expect(answered).rejects.toMatchObject({ name: "AbortError" });
    expect(test.posted).toEqual([
      { kind: "request", id: 1, operation: "reference.read", payload: {} },
      { kind: "cancel", id: 1 },
    ]);
    test.deliver({ kind: "response", id: 1, ok: true, value: "late" });

    const aborted = new AbortController();
    aborted.abort();
    await expect(test.bridge.client.request("reference.read", {}, { signal: aborted.signal }))
      .rejects.toMatchObject({ name: "AbortError" });
    expect(test.posted).toHaveLength(2);
  });

  it("delivers events serially and acknowledges only after each listener resolves", async () => {
    const test = harness();
    const order: string[] = [];
    const releases: Array<() => void> = [];
    const subscribing = test.bridge.client.subscribe<string>("reference.changes", {}, async (event) => {
      order.push(`start:${event}`);
      await new Promise<void>((resolve) => releases.push(resolve));
      order.push(`end:${event}`);
    });
    test.deliver({ kind: "subscribed", id: 1 });
    await subscribing;

    test.deliver({ kind: "event", id: 1, sequence: "0", payload: "a" });
    test.deliver({ kind: "event", id: 1, sequence: "1", payload: "b" });
    await settle();
    expect(order).toEqual(["start:a"]);
    expect(test.posted.filter((message) => message.kind === "ack")).toEqual([]);

    releases[0]?.();
    await settle();
    expect(order).toEqual(["start:a", "end:a", "start:b"]);
    expect(test.posted.filter((message) => message.kind === "ack")).toEqual([{ kind: "ack", id: 1, sequence: "0" }]);

    releases[1]?.();
    await settle();
    expect(order).toEqual(["start:a", "end:a", "start:b", "end:b"]);
    expect(test.posted.filter((message) => message.kind === "ack")).toEqual([
      { kind: "ack", id: 1, sequence: "0" },
      { kind: "ack", id: 1, sequence: "1" },
    ]);
  });

  it("closes without acknowledgement and reports a rejected listener once", async () => {
    const test = harness();
    const subscribing = test.bridge.client.subscribe("reference.changes", {}, () => {
      throw new Error("listener failed");
    });
    test.deliver({ kind: "subscribed", id: 1 });
    await subscribing;

    test.deliver({ kind: "event", id: 1, sequence: "0", payload: "a" });
    test.deliver({ kind: "event", id: 1, sequence: "1", payload: "b" });
    await settle();

    expect(test.posted.filter((message) => message.kind === "ack")).toEqual([]);
    expect(test.posted.filter((message) => message.kind === "unsubscribe")).toEqual([{ kind: "unsubscribe", id: 1 }]);
    expect(test.faults).toBe(1);
  });

  it("treats stream termination as closed without acknowledgement", async () => {
    const test = harness();
    let delivered = 0;
    const subscribing = test.bridge.client.subscribe("reference.changes", {}, () => { delivered += 1; });
    test.deliver({ kind: "subscribed", id: 1 });
    const subscription = await subscribing;

    test.deliver({ kind: "stream_terminated", id: 1 });
    test.deliver({ kind: "event", id: 1, sequence: "0", payload: "a" });
    await settle();

    expect(delivered).toBe(0);
    expect(test.posted.map((message) => message.kind)).toEqual(["subscribe"]);
    expect(test.faults).toBe(0);
    await expect(subscription.close()).resolves.toBeUndefined();
    expect(test.posted.map((message) => message.kind)).toEqual(["subscribe"]);
  });

  it("makes subscription close and client dispose idempotent and rejects new work after dispose", async () => {
    const test = harness();
    const subscribing = test.bridge.client.subscribe("reference.changes", {}, () => undefined);
    test.deliver({ kind: "subscribed", id: 1 });
    const subscription = await subscribing;

    const closing = subscription.close();
    expect(test.posted.filter((message) => message.kind === "unsubscribe")).toHaveLength(1);
    test.deliver({ kind: "unsubscribed", id: 1 });
    await closing;
    await expect(subscription.close()).resolves.toBeUndefined();
    expect(test.posted.filter((message) => message.kind === "unsubscribe")).toHaveLength(1);

    const second = await (async () => {
      const opening = test.bridge.client.subscribe("reference.changes", {}, () => undefined);
      test.deliver({ kind: "subscribed", id: 2 });
      return opening;
    })();
    const pending = test.bridge.client.request("reference.read", {});
    await test.bridge.dispose();
    await test.bridge.dispose();
    await expect(pending).rejects.toThrow("Package UI mount is closed");
    expect(test.posted.filter((message) => message.kind === "unsubscribe")).toEqual([
      { kind: "unsubscribe", id: 1 },
      { kind: "unsubscribe", id: 2 },
    ]);
    await expect(second.close()).resolves.toBeUndefined();
    await expect(test.bridge.client.request("reference.read", {})).rejects.toThrow("Package UI mount is closed");
    await expect(test.bridge.client.subscribe("reference.changes", {}, () => undefined))
      .rejects.toThrow("Package UI mount is closed");
  });
});
