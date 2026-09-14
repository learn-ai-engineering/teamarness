import { describe, expect, it } from "vitest";
import { createPackageClient, type PackageTransport, type UiEvent } from "./index";

function deferred(): { promise: Promise<void>; resolve: () => void; reject: (reason: Error) => void } {
  let resolve: () => void = () => {};
  let reject: (reason: Error) => void = () => {};
  const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function fakeTransport(log: string[]): PackageTransport & { deliver?: (event: UiEvent) => void } {
  const transport: PackageTransport & { deliver?: (event: UiEvent) => void } = {
    request: async () => ({ ok: true }),
    cancelRequest: async () => { log.push("cancel"); },
    openSubscription: async (_input, deliver) => { transport.deliver = deliver; },
    acknowledgeEvents: async ({ throughSequence }) => { log.push(`ack:${throughSequence}`); },
    closeSubscription: async () => { log.push("close"); },
    reportUiFault: async () => { log.push("fault"); },
  };
  return transport;
}

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
