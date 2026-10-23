import { typedPackageClient } from "@teamarness/package-ui-sdk";
import type { PackageClient, PackageValue } from "@teamarness/package-ui-sdk";
import { createPackageRequestError } from "@teamarness/package-ui-sdk/testing";
import { describe, expect, it } from "vitest";
import type { ConversationsContract } from "./contract.generated";
import { ConversationsController, type ConversationsClient } from "./controller";

const CHANNEL = "0".repeat(32);
const OTHER_CHANNEL = "1".repeat(32);
const epoch = "a".repeat(32);
const snapshot = () => ({ availability: "current" as const, channels: [
  { id: CHANNEL, slug: "general", canHide: false, unreadCount: 0 },
  { id: OTHER_CHANNEL, slug: "other", canHide: false, unreadCount: 0 },
] });
const message = (n: number) => ({ id: String(n), position: String(n).padStart(8, "0"),
  author: "alice", markdown: `message-${n}`, isLocal: false });

function packageRequestError(code: string, retryable: boolean): unknown {
  return createPackageRequestError({ stage: "ui", retryable, diagnosticId: "diag-1",
    packageError: { code, retryable } });
}
function classifiedClient(rejection: unknown, failOperation = "conversations.channel.open"): ConversationsClient {
  const raw: PackageClient = {
    request: async <T extends PackageValue>(operation: string, payload: PackageValue): Promise<T> => {
      if (operation === failOperation && (operation !== "conversations.channel.open"
        || (payload as { channelId: string }).channelId === OTHER_CHANNEL)) throw rejection;
      if (operation === "conversations.channel.open") return { messages: [], epoch, revision: "0" } as unknown as T;
      return snapshot() as unknown as T;
    },
    subscribe: async () => ({ close: async () => undefined }),
  };
  return typedPackageClient<ConversationsContract>(raw);
}
async function failedSwitch(rejection: unknown): Promise<ConversationsController> {
  const controller = new ConversationsController(classifiedClient(rejection), () => undefined);
  await controller.initialize();
  await controller.selectChannel(OTHER_CHANNEL);
  return controller;
}

describe("Workspace Principals", () => {
  it("assembles a bounded roster at one cursor before relabeling signed authors, then replaces it on rename", () => {
    const controller = new ConversationsController(classifiedClient(null), () => undefined);
    const alice = { identity: "alice", principalId: "p1", kind: "person" as const, stewardPrincipalId: null, name: "Alice", fingerprint: "alice" };
    const bob = { identity: "bob", principalId: "p2", kind: "person" as const, stewardPrincipalId: null, name: "Bob", fingerprint: "bob" };
    controller.applyPrincipals({ available: true, cursor: "1", principals: [alice], part: 0, parts: 2 });
    expect(controller.state.principals.available).toBe(false);
    controller.applyPrincipals({ available: true, cursor: "1", principals: [bob], part: 1, parts: 2 });
    expect(controller.state.principals.principals).toEqual([alice, bob]);
    controller.applyPrincipals({ available: true, cursor: "2", principals: [{ ...alice, name: "Renamed" }], part: 0, parts: 2 });
    expect(controller.state.principals.principals).toEqual([alice, bob]);
    controller.applyPrincipals({ available: true, cursor: "1", principals: [bob], part: 1, parts: 2 });
    expect(controller.state.principals.principals).toEqual([alice, bob]);
    controller.applyPrincipals({ available: true, cursor: "2", principals: [bob], part: 1, parts: 2 });
    expect(controller.state.principals.principals).toEqual([{ ...alice, name: "Renamed" }, bob]);
  });
  it("relabels an existing remote Message on rename without changing its signed author", async () => {
    const fake = delayedClient();
    const controller = new ConversationsController(fake.client, () => undefined);
    const initial = controller.initialize();
    await Promise.resolve();
    fake.pending[0]!.reply.resolve({ messages: [message(1)], epoch, revision: "0" });
    await initial;
    controller.applyPrincipals({ available: true, cursor: "1", principals: [{ identity: "alice", principalId: "p1", kind: "person" as const, stewardPrincipalId: null, name: "🌿 Alice", fingerprint: "alice" }] });
    expect(controller.state.messages[0]?.author).toBe("alice");
    expect(controller.state.principals.principals[0]?.name).toBe("🌿 Alice");
    controller.applyPrincipals({ available: true, cursor: "2", principals: [{ identity: "alice", principalId: "p1", kind: "person" as const, stewardPrincipalId: null, name: "Alice 🌻", fingerprint: "alice" }] });
    controller.applyPrincipals({ available: true, cursor: "1", principals: [] });
    expect(controller.state.principals.principals[0]?.name).toBe("Alice 🌻");
    expect(controller.state.messages[0]?.author).toBe("alice");
  });
});

describe("Conversations operation errors", () => {
  it("shows the Package code's hidden-Channel message", async () => {
    expect((await failedSwitch(packageRequestError("conversations.channel_hidden", false))).state.warning)
      .toBe("This Channel has been permanently hidden.");
  });
  it("uses retryability before a non-retryable Package code", async () => {
    expect((await failedSwitch(packageRequestError("conversations.channel_hidden", true))).state.warning)
      .toBe("Unable to complete the action. Please try again.");
  });
  it("falls back to a safe message for unknown host errors", async () => {
    expect((await failedSwitch({ code: "stale_mount", retryable: false })).state.warning)
      .toBe("Something went wrong. Please try again.");
  });
  it("does not promise to send an uncommitted Message while waiting for a key holder", async () => {
    const controller = await failedSwitch(packageRequestError("conversations.capability_unavailable", true));
    controller.applySnapshot({ ...snapshot(), availability: "waiting_for_history_access" });
    await controller.selectChannel(CHANNEL);
    await controller.selectChannel(OTHER_CHANNEL);
    expect(controller.state.warning).toBe("Posting is unavailable until a current key holder shares history access.");
  });
  it("preserves the committed offline Message wording", async () => {
    const controller = await failedSwitch(packageRequestError("conversations.capability_unavailable", true));
    controller.applySnapshot({ ...snapshot(), availability: "offline" });
    await controller.selectChannel(CHANNEL);
    await controller.selectChannel(OTHER_CHANNEL);
    expect(controller.state.warning).toBe("You are offline. Your message is saved and will sync when a member is reachable.");
  });
  it("rejects malformed slugs locally and maps Backend create failures", async () => {
    const controller = new ConversationsController(classifiedClient(packageRequestError("conversations.invalid_slug", false),
      "conversations.channel.create"), () => undefined);
    await controller.initialize();
    await controller.createChannel("Bad_Name");
    expect(controller.state.warning).toBe("Channel names use lowercase letters, numbers, and hyphens.");
    await controller.createChannel("design");
    expect(controller.state.warning).toBe("Channel names use lowercase letters, numbers, and hyphens.");
  });
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function delayedClient() {
  const pending: { operation: string; payload: PackageValue; reply: ReturnType<typeof deferred<PackageValue>> }[] = [];
  const raw: PackageClient = {
    request: <T extends PackageValue>(operation: string, payload: PackageValue): Promise<T> => {
      if (operation === "conversations.snapshot") return Promise.resolve(snapshot()) as unknown as Promise<T>;
      const reply = deferred<PackageValue>();
      pending.push({ operation, payload, reply });
      return reply.promise as Promise<T>;
    },
    subscribe: async () => ({ close: async () => undefined }),
  };
  return { client: typedPackageClient<ConversationsContract>(raw), pending };
}

describe("independent history request ownership", () => {
  it("crosses advancing empty pages, then blocks automatic retries after a failed page", async () => {
    const fake = delayedClient();
    const controller = new ConversationsController(fake.client, () => undefined);
    const initial = controller.initialize();
    await Promise.resolve();
    fake.pending[0]!.reply.resolve({ messages: [message(64)], nextBefore: "c64", epoch, revision: "0" });
    await initial;
    const older = controller.loadOlder();
    fake.pending[1]!.reply.resolve({ messages: [], nextBefore: "c32" });
    await Promise.resolve();
    expect(fake.pending[2]?.payload).toEqual({ channelId: CHANNEL, before: "c32", limit: 32 });
    fake.pending[2]!.reply.reject(new Error("temporary page error"));
    await older;
    expect(controller.state.olderError).toBe(true);
    await controller.loadOlder();
    expect(fake.pending).toHaveLength(3);
    const retry = controller.retryOlder();
    expect(fake.pending[3]?.payload).toEqual({ channelId: CHANNEL, before: "c32", limit: 32 });
    fake.pending[3]!.reply.resolve({ messages: [message(1)] });
    await retry;
    expect(controller.state.messages.map(m => m.id)).toEqual(["1", "64"]);
    expect(controller.state.hasMore).toBe(false);
  });

  it("discards a stale older reply after Channel switch and disposal", async () => {
    const fake = delayedClient();
    const controller = new ConversationsController(fake.client, () => undefined);
    const initial = controller.initialize();
    await Promise.resolve();
    fake.pending[0]!.reply.resolve({ messages: [message(64)], nextBefore: "older", epoch, revision: "0" });
    await initial;
    const older = controller.loadOlder();
    const switchRequest = controller.selectChannel(OTHER_CHANNEL);
    fake.pending[2]!.reply.resolve({ messages: [message(2)], epoch, revision: "0" });
    await switchRequest;
    fake.pending[1]!.reply.resolve({ messages: [message(1)] });
    await older;
    expect(controller.state.messages.map(m => m.id)).toEqual(["2"]);
    const back = controller.selectChannel(CHANNEL);
    controller.dispose();
    fake.pending[3]!.reply.resolve({ messages: [message(3)], epoch, revision: "0" });
    await back;
    expect(controller.state.messages).toEqual([]);
  });

  it("coalesces queued snapshots and awaits the second change batch", async () => {
    const fake = delayedClient();
    const controller = new ConversationsController(fake.client, () => undefined);
    const initial = controller.initialize();
    await Promise.resolve();
    fake.pending[0]!.reply.resolve({ messages: [message(1)], epoch, revision: "0" });
    await initial;
    const first = controller.applySnapshot(snapshot());
    const second = controller.applySnapshot(snapshot());
    expect(fake.pending.filter(call => call.operation === "conversations.channel.changes")).toHaveLength(1);
    fake.pending[1]!.reply.resolve({ events: [], continuation: "0", highWater: "0" });
    await Promise.resolve(); await Promise.resolve();
    expect(fake.pending[2]?.operation).toBe("conversations.channel.changes");
    fake.pending[2]!.reply.resolve({ events: [{ kind: "insert", id: "2", position: message(2).position,
      status: "ready", message: message(2), revision: "1" }], continuation: "1", highWater: "1" });
    await first; await second;
    expect(controller.state.messages.map(m => m.id)).toEqual(["1", "2"]);
  });

  it("keeps post acknowledgement owned by the Channel that sent it", async () => {
    const fake = delayedClient();
    const controller = new ConversationsController(fake.client, () => undefined);
    const initial = controller.initialize();
    await Promise.resolve();
    fake.pending[0]!.reply.resolve({ messages: [], epoch, revision: "0" });
    await initial;
    const post = controller.postMessage("draft");
    expect(controller.state.posting).toBe(true);
    expect(await controller.postMessage("draft")).toBe(false);
    const switched = controller.selectChannel(OTHER_CHANNEL);
    fake.pending[2]!.reply.resolve({ messages: [], epoch, revision: "0" }); await switched;
    fake.pending[1]!.reply.resolve({ ok: true });
    expect(await post).toBe(false);
    expect(controller.state.posting).toBe(false);
    expect(fake.pending.filter(call => call.operation === "conversations.channel.changes")).toHaveLength(0);
  });
});
