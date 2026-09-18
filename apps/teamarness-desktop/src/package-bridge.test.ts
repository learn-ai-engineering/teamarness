import type { Channel } from "@tauri-apps/api/core";
import { describe, expect, it } from "vitest";
import { createTauriPackageHostTransport } from "./package-bridge";

describe("desktop Tauri Package transport", () => {
  it("maps a terminal Tauri stream message to termination rather than delivery", async () => {
    let receive: ((message: unknown) => void) | undefined;
    const transport = createTauriPackageHostTransport(
      async () => undefined as never,
      <T>(handler: (message: T) => void): Channel<T> => {
        receive = (message) => { handler(message as T); };
        return {} as Channel<T>;
      },
    );
    const log: string[] = [];

    await transport.openSubscription(
      { mount: "mount-1", subscription: "s1", operation: "fixture.changes", payload: {} },
      () => { log.push("deliver"); },
      () => { log.push("terminate"); },
    );
    receive?.({ kind: "failure", value: { code: "event_lagged" } });
    receive?.({
      kind: "event",
      value: { mount: "mount-1", subscription: "s1", sequence: "0", payload: {} },
    });

    expect(log).toEqual(["terminate", "deliver"]);
  });

  it("sends every package command with the mount-scoped input unchanged", async () => {
    const calls: Array<readonly [string, unknown]> = [];
    const transport = createTauriPackageHostTransport(
      (async (command: string, input: unknown) => {
        calls.push([command, input]);
        return null;
      }) as never,
      <T>(): Channel<T> => ({ marker: "channel" }) as unknown as Channel<T>,
    );

    await transport.request({ mount: "mount-1", request: "r1", operation: "fixture.read", payload: { a: 1 } });
    await transport.cancelRequest({ mount: "mount-1", request: "r1" });
    await transport.openSubscription(
      { mount: "mount-1", subscription: "s1", operation: "fixture.changes", payload: {} },
      () => undefined,
      () => undefined,
    );
    await transport.acknowledgeEvents({ mount: "mount-1", subscription: "s1", throughSequence: "3" });
    await transport.closeSubscription({ mount: "mount-1", subscription: "s1" });
    await transport.reportUiFault({ mount: "mount-1", code: "ui_exception" });

    expect(calls.map(([command]) => command)).toEqual([
      "package_request",
      "package_cancel_request",
      "package_open_stream",
      "package_acknowledge_events",
      "package_close_stream",
      "package_report_ui_fault",
    ]);
    expect(calls[0]?.[1]).toEqual({ mount: "mount-1", request: "r1", operation: "fixture.read", payload: { a: 1 } });
    expect(calls[2]?.[1]).toEqual({
      mount: "mount-1",
      subscription: "s1",
      operation: "fixture.changes",
      payload: {},
      onEvent: { marker: "channel" },
    });
    expect(calls[5]?.[1]).toEqual({ mount: "mount-1", code: "ui_exception" });
  });
});
