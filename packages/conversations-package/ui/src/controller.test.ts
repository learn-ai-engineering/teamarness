import { typedPackageClient } from "@teamarness/package-ui-sdk";
import type { PackageClient, PackageValue } from "@teamarness/package-ui-sdk";
import { createPackageRequestError } from "@teamarness/package-ui-sdk/testing";
import { describe, expect, it } from "vitest";
import type { ConversationsContract } from "./contract.generated";
import { ConversationsController, type ConversationsClient } from "./controller";

/** A rejected Package request as the frame bridge brands it. */
function packageRequestError(code: string, retryable: boolean): unknown {
  return createPackageRequestError({
    stage: "ui",
    retryable,
    diagnosticId: "diag-1",
    packageError: { code, retryable },
  });
}

const CHANNEL = "00000000000000000000000000000000";
const OTHER_CHANNEL = "11111111111111111111111111111111";

function client(rejection: unknown): ConversationsClient {
  const untyped: PackageClient = {
    request: async <TResponse extends PackageValue>(
      operation: string,
      payload: PackageValue,
    ): Promise<TResponse> => {
      if (operation === "conversations.channel.open") {
        const channelId = (payload as { channelId?: string }).channelId;
        if (channelId === OTHER_CHANNEL) throw rejection;
        return { messages: [] } as unknown as TResponse;
      }
      return {
        availability: "current",
        channels: [{ id: CHANNEL, slug: "general", canHide: false, unreadCount: 0 }],
      } as unknown as TResponse;
    },
    subscribe: async () => ({ close: async () => undefined }),
  };
  return typedPackageClient<ConversationsContract>(untyped);
}

/** A client that fails `channel.create` with `rejection` and records every operation it served. */
function creatingClient(rejection: unknown): { instance: ConversationsClient; calls: string[] } {
  const calls: string[] = [];
  const untyped: PackageClient = {
    request: async <TResponse extends PackageValue>(
      operation: string,
      payload: PackageValue,
    ): Promise<TResponse> => {
      calls.push(operation, JSON.stringify(payload));
      if (operation === "conversations.channel.create") throw rejection;
      return {
        availability: "current",
        channels: [{ id: CHANNEL, slug: "general", canHide: false, unreadCount: 0 }],
      } as unknown as TResponse;
    },
    subscribe: async () => ({ close: async () => undefined }),
  };
  return { instance: typedPackageClient<ConversationsContract>(untyped), calls };
}

/** Reaches an active, `current` session, then fails one channel open. */
async function failingOpen(rejection: unknown): Promise<ConversationsController> {
  const controller = new ConversationsController(client(rejection), () => undefined);
  await controller.initialize();
  await controller.selectChannel(OTHER_CHANNEL);
  return controller;
}

describe("Conversations error classification", () => {
  it("uses the Package code and retryability from a typed error", async () => {
    const controller = await failingOpen(packageRequestError("conversations.channel_hidden", false));
    expect(controller.state.warning).toBe("This Channel has been permanently hidden.");
  });

  it("keeps the retryable message for a retryable Package error", async () => {
    const controller = await failingOpen(packageRequestError("conversations.channel_hidden", true));
    expect(controller.state.warning).toBe("Unable to complete the action. Please try again.");
  });

  it("falls back to the host code when there is no Package error", async () => {
    const controller = await failingOpen({ code: "stale_mount", retryable: false });
    expect(controller.state.warning).toBe("Something went wrong. Please try again.");
  });
});

describe("history access failure wording", () => {
  // Verifies: TEAM.SYS.TRANS-R08, TEAM.SYS.CONV-R14
  it("does not promise to send an uncommitted Message while waiting for a key holder", async () => {
    const controller = await failingOpen(packageRequestError("conversations.capability_unavailable", true));
    controller.applySnapshot({
      availability: "waiting_for_history_access",
      channels: [{ id: CHANNEL, slug: "general", canHide: false, unreadCount: 0 }],
    });
    await controller.selectChannel(OTHER_CHANNEL);
    expect(controller.state.warning).toBe("Posting is unavailable until a current key holder shares history access.");
    expect(controller.state.warning).not.toMatch(/will be sent|sent soon/i);
  });

  it("preserves the committed offline Message wording", async () => {
    const controller = await failingOpen(packageRequestError("conversations.capability_unavailable", true));
    controller.applySnapshot({
      availability: "offline",
      channels: [{ id: CHANNEL, slug: "general", canHide: false, unreadCount: 0 }],
    });
    await controller.selectChannel(OTHER_CHANNEL);
    expect(controller.state.warning).toBe("You are offline. Your message is saved and will sync when a member is reachable.");
  });
});

describe("createChannel error mapping", () => {
  it("fails an invalid slug locally without a round trip", async () => {
    const { instance, calls } = creatingClient(new Error("must not be reached"));
    const controller = new ConversationsController(instance, () => undefined);
    await controller.initialize();
    await controller.createChannel("Bad_Name");
    expect(controller.state.warning).toBe("Channel names use lowercase letters, numbers, and hyphens.");
    expect(calls).not.toContain("conversations.channel.create");
  });

  it("shows the real failure for a valid name instead of the slug message", async () => {
    const controller = new ConversationsController(
      creatingClient(packageRequestError("conversations.capability_unavailable", false)).instance,
      () => undefined,
    );
    await controller.initialize();
    await controller.createChannel("design");
    expect(controller.state.warning).toBe("Something went wrong. Please try again.");
  });

  it("keeps the slug message when the Backend refuses a well-formed name as invalid_slug", async () => {
    const controller = new ConversationsController(
      creatingClient(packageRequestError("conversations.invalid_slug", false)).instance,
      () => undefined,
    );
    await controller.initialize();
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

function historyClient() {
  type Response = PackageValue;
  const pending: { operation: string; payload: PackageValue; reply: ReturnType<typeof deferred<Response>> }[] = [];
  const untyped: PackageClient = {
    request: <T extends PackageValue>(operation: string, payload: PackageValue) => {
      if (operation === "conversations.snapshot") return Promise.resolve({
        availability: "current",
        channels: [
          { id: CHANNEL, slug: "general", canHide: false, unreadCount: 0 },
          { id: OTHER_CHANNEL, slug: "other", canHide: false, unreadCount: 0 },
        ],
      }) as unknown as Promise<T>;
      const reply = deferred<Response>();
      pending.push({ operation, payload, reply });
      return reply.promise as Promise<T>;
    },
    subscribe: async () => ({ close: async () => undefined }),
  };
  const batch = (start: number, end: number) => Array.from({ length: start - end + 1 }, (_, i) => ({
    id: String(start - i), markdown: `fixture-${start - i}`, author: "fixture", isLocal: false,
  }));
  const page = (start: number, end: number, nextBefore?: string) => ({
    messages: batch(start, end), ...(nextBefore ? { nextBefore } : {}),
  });
  const flush = async () => { for (let i = 0; i < 8; i++) await Promise.resolve(); };
  return { instance: typedPackageClient<ConversationsContract>(untyped), pending, page, flush };
}

function historySnapshot() {
  return { availability: "current" as const, channels: [
    { id: CHANNEL, slug: "general", canHide: false, unreadCount: 0 },
    { id: OTHER_CHANNEL, slug: "other", canHide: false, unreadCount: 0 },
  ] };
}

describe("bounded history reconciliation", () => {
  it("crosses advancing empty older pages to reach readable history without concurrent requests", async () => {
    const fake = historyClient();
    const controller = new ConversationsController(fake.instance, () => undefined);
    const initial = controller.initialize(); await fake.flush();
    fake.pending[0]!.reply.resolve(fake.page(64, 33, "c33")); await initial;
    const older = controller.loadOlder();
    void controller.loadOlder();
    fake.pending[1]!.reply.resolve({ messages: [], nextBefore: "c1" }); await fake.flush();
    expect(fake.pending[2]?.payload).toEqual({ channelId: CHANNEL, before: "c1", limit: 32 });
    expect(controller.state.loadingOlder).toBe(true);
    fake.pending[2]!.reply.resolve(fake.page(1, 1)); await older;
    expect(controller.state.messages.map((m) => m.id)).toEqual(["1", ...fake.page(64, 33).messages.map((m) => m.id).reverse()]);
    expect(controller.state.hasMore).toBe(false);
    expect(fake.pending).toHaveLength(3);
  });

  it("stops an advancing empty-page run at a bounded retry instead of spinning", async () => {
    const fake = historyClient();
    const controller = new ConversationsController(fake.instance, () => undefined);
    const initial = controller.initialize(); await fake.flush();
    fake.pending[0]!.reply.resolve(fake.page(32, 1, "c0")); await initial;
    const older = controller.loadOlder();
    for (let i = 1; i <= 32; i++) {
      fake.pending[i]!.reply.resolve({ messages: [], nextBefore: `c${i}` });
      await fake.flush();
    }
    await older;
    expect(fake.pending).toHaveLength(33);
    expect(controller.state.olderError).toBe(true);
    expect(controller.state.messages).toHaveLength(32);
    await controller.loadOlder();
    expect(fake.pending).toHaveLength(33);
    const retry = controller.retryOlder();
    expect(fake.pending[33]?.payload).toEqual({ channelId: CHANNEL, before: "c32", limit: 32 });
    fake.pending[33]!.reply.resolve(fake.page(0, 0)); await retry;
    expect(controller.state.olderError).toBe(false);
  });

  it("chains strict older cursors, deduplicates, and stops after a terminal empty page", async () => {
    const fake = historyClient();
    const controller = new ConversationsController(fake.instance, () => undefined);
    const initial = controller.initialize();
    await fake.flush();
    fake.pending[0]!.reply.resolve(fake.page(64, 33, "cursor-33"));
    await initial;
    const first = controller.loadOlder();
    void controller.loadOlder();
    expect(fake.pending).toHaveLength(2);
    expect(fake.pending[1]!.payload).toEqual({ channelId: CHANNEL, before: "cursor-33", limit: 32 });
    fake.pending[1]!.reply.resolve({ messages: [fake.page(33, 33).messages[0]!, ...fake.page(32, 2).messages], nextBefore: "cursor-2" });
    await first;
    expect(new Set(controller.state.messages.map((m) => m.id)).size).toBe(63);
    const last = controller.loadOlder();
    expect(fake.pending[2]!.payload).toEqual({ channelId: CHANNEL, before: "cursor-2", limit: 32 });
    fake.pending[2]!.reply.resolve({ messages: fake.page(1, 1).messages, nextBefore: "cursor-1" });
    await last;
    const terminal = controller.loadOlder();
    expect(fake.pending[3]!.payload).toEqual({ channelId: CHANNEL, before: "cursor-1", limit: 32 });
    fake.pending[3]!.reply.resolve({ messages: [] });
    await terminal;
    await controller.loadOlder();
    expect(fake.pending).toHaveLength(4);
    expect(new Set(controller.state.messages.map((m) => m.id)).size).toBe(64);
    expect(controller.state.hasMore).toBe(false);
  });

  it("keeps history and requires explicit retry after a failed page", async () => {
    const fake = historyClient();
    const controller = new ConversationsController(fake.instance, () => undefined);
    const initial = controller.initialize(); await fake.flush();
    fake.pending[0]!.reply.resolve(fake.page(32, 1, "older")); await initial;
    const failed = controller.loadOlder(); fake.pending[1]!.reply.reject(new Error("fixture failure")); await failed;
    expect(controller.state.messages).toHaveLength(32);
    expect(controller.state.olderError).toBe(true);
    await controller.loadOlder();
    expect(fake.pending).toHaveLength(2);
    const retry = controller.retryOlder();
    expect(fake.pending[2]!.payload).toEqual({ channelId: CHANNEL, before: "older", limit: 32 });
    fake.pending[2]!.reply.resolve({ messages: [] }); await retry;
    expect(controller.state.olderError).toBe(false);
  });

  it("discards pending older and head replies after switching or disposal", async () => {
    const fake = historyClient();
    const controller = new ConversationsController(fake.instance, () => undefined);
    const initial = controller.initialize(); await fake.flush();
    fake.pending[0]!.reply.resolve(fake.page(32, 1, "older")); await initial;
    const older = controller.loadOlder();
    const refresh = controller.applySnapshot(historySnapshot());
    const switchRequest = controller.selectChannel(OTHER_CHANNEL);
    fake.pending[1]!.reply.resolve(fake.page(0, 0));
    fake.pending[2]!.reply.resolve(fake.page(99, 99));
    await older; await refresh;
    fake.pending[3]!.reply.resolve(fake.page(100, 100)); await switchRequest;
    expect(controller.state.messages.map((m) => m.id)).toEqual(["100"]);
    const back = controller.selectChannel(CHANNEL);
    controller.dispose();
    fake.pending[4]!.reply.resolve(fake.page(200, 200)); await back;
    expect(controller.state.messages).toEqual([]);
  });

  it("reconciles a >32 new-Message burst through bounded pages while older paging and posting overlap", async () => {
    const fake = historyClient();
    const controller = new ConversationsController(fake.instance, () => undefined);
    const initial = controller.initialize(); await fake.flush();
    fake.pending[0]!.reply.resolve(fake.page(64, 33, "c33")); await initial;
    const older = controller.loadOlder();
    const posting = controller.postMessage("fixture post");
    const refresh = controller.applySnapshot(historySnapshot());
    fake.pending[1]!.reply.resolve(fake.page(32, 1)); await older;
    const postCall = fake.pending.find((p) => p.operation === "conversations.message.post");
    postCall!.reply.resolve({ ok: true }); await fake.flush();
    const open = fake.pending.find((p) => p.operation === "conversations.channel.open" && p !== fake.pending[0]);
    open!.reply.resolve(fake.page(140, 109, "c109")); await fake.flush();
    const latestOpen = fake.pending.find((p) => p.operation === "conversations.channel.open" && p !== fake.pending[0] && p !== open);
    if (latestOpen) { latestOpen.reply.resolve(fake.page(140, 109, "c109")); await fake.flush(); }
    const next = fake.pending.find((p) => p.operation === "conversations.channel.page" && (p.payload as { before: string }).before === "c109");
    expect(next).toBeDefined();
    next!.reply.resolve(fake.page(108, 77, "c77")); await fake.flush();
    const overlap = fake.pending.find((p) => p.operation === "conversations.channel.page" && (p.payload as { before: string }).before === "c77");
    expect(overlap).toBeDefined();
    overlap!.reply.resolve(fake.page(76, 45, "c45")); await fake.flush();
    // A coalesced refresh may request the head again after the post.
    for (const request of fake.pending.filter((p) => p.operation === "conversations.channel.open" && p !== fake.pending[0] && p !== open)) {
      request.reply.resolve(fake.page(140, 109, "c109"));
    }
    await fake.flush(); await posting; await refresh;
    expect(controller.state.messages.map((m) => Number(m.id))).toEqual(Array.from({ length: 140 }, (_, i) => i + 1));
    expect(controller.state.nextBefore).toBeNull();
  });
});

describe("live history request ownership", () => {
  it("bounds an empty advancing head run and leaves explicit retry", async () => {
    const fake = historyClient();
    const controller = new ConversationsController(fake.instance, () => undefined);
    const initial = controller.initialize(); await fake.flush();
    fake.pending[0]!.reply.resolve(fake.page(32, 1)); await initial;
    const refresh = controller.applySnapshot(historySnapshot());
    fake.pending[1]!.reply.resolve({ messages: [], nextBefore: "c1" }); await fake.flush();
    for (let i = 2; i <= 33; i++) {
      fake.pending[i]!.reply.resolve({ messages: [], nextBefore: `c${i}` });
      await fake.flush();
    }
    await refresh;
    expect(fake.pending).toHaveLength(34);
    expect(controller.state.olderError).toBe(true);
    expect(controller.state.messages).toHaveLength(32);
  });

  it("bridges empty advancing head pages before finding overlap", async () => {
    const fake = historyClient();
    const controller = new ConversationsController(fake.instance, () => undefined);
    const initial = controller.initialize(); await fake.flush();
    fake.pending[0]!.reply.resolve(fake.page(32, 1)); await initial;
    const refresh = controller.applySnapshot(historySnapshot());
    fake.pending[1]!.reply.resolve({ messages: [], nextBefore: "gap" }); await fake.flush();
    expect(fake.pending[2]?.payload).toEqual({ channelId: CHANNEL, before: "gap", limit: 32 });
    fake.pending[2]!.reply.resolve(fake.page(96, 65, "overlap")); await fake.flush();
    expect(fake.pending[3]?.payload).toEqual({ channelId: CHANNEL, before: "overlap", limit: 32 });
    fake.pending[3]!.reply.resolve(fake.page(64, 33, "known")); await fake.flush();
    fake.pending[4]!.reply.resolve(fake.page(32, 1)); await refresh;
    expect(controller.state.messages.map((m) => Number(m.id))).toEqual(Array.from({ length: 96 }, (_, i) => i + 1));
  });

  it.each(["initial", "switch"])("offers explicit open retry after %s Channel failure, then recognizes successful exhaustion", async (mode) => {
    const fake = historyClient();
    const controller = new ConversationsController(fake.instance, () => undefined);
    const initial = controller.initialize(); await fake.flush();
    if (mode === "switch") {
      fake.pending[0]!.reply.resolve(fake.page(1, 1)); await initial;
      const selected = controller.selectChannel(OTHER_CHANNEL);
      fake.pending[1]!.reply.reject(new Error("open failed")); await selected;
    } else {
      fake.pending[0]!.reply.reject(new Error("open failed")); await initial;
    }
    expect(controller.state.olderError).toBe(true);
    expect(controller.state.hasMore).toBe(false);
    const count = fake.pending.length;
    await controller.loadOlder();
    expect(fake.pending).toHaveLength(count);
    const retry = controller.retryOlder();
    expect(fake.pending[count]?.operation).toBe("conversations.channel.open");
    fake.pending[count]!.reply.resolve({ messages: [] }); await retry;
    expect(controller.state.olderError).toBe(false);
    expect(controller.state.hasMore).toBe(false);
  });

  it("drains B's queued refresh after A's stale head request settles", async () => {
    const fake = historyClient();
    const controller = new ConversationsController(fake.instance, () => undefined);
    const initial = controller.initialize(); await fake.flush();
    fake.pending[0]!.reply.resolve(fake.page(1, 1)); await initial;
    const stale = controller.applySnapshot(historySnapshot());
    const switched = controller.selectChannel(OTHER_CHANNEL);
    fake.pending[2]!.reply.resolve(fake.page(2, 2)); await switched;
    const queued = controller.applySnapshot(historySnapshot());
    expect(fake.pending).toHaveLength(3);
    fake.pending[1]!.reply.resolve(fake.page(99, 99)); await stale; await fake.flush();
    expect(fake.pending[3]?.payload).toEqual({ channelId: OTHER_CHANNEL, limit: 32 });
    fake.pending[3]!.reply.resolve(fake.page(3, 3)); await queued; await fake.flush();
    expect(controller.state.messages.map((m) => m.id)).toEqual(["2", "3"]);
  });

  it("uses only the first head page when no Message IDs are known", async () => {
    const fake = historyClient();
    const controller = new ConversationsController(fake.instance, () => undefined);
    const initial = controller.initialize(); await fake.flush();
    fake.pending[0]!.reply.reject(new Error("initial open failed")); await initial;
    const refresh = controller.applySnapshot(historySnapshot());
    fake.pending[1]!.reply.resolve(fake.page(96, 65, "older")); await refresh;
    expect(fake.pending).toHaveLength(2);
    expect(controller.state.messages.map((m) => Number(m.id))).toEqual(Array.from({ length: 32 }, (_, i) => i + 65));
    expect(controller.state.nextBefore).toBe("older");
    expect(controller.state.hasMore).toBe(true);
  });

  it("acknowledges only a successful post for its selected Channel", async () => {
    const fake = historyClient();
    const controller = new ConversationsController(fake.instance, () => undefined);
    const initial = controller.initialize(); await fake.flush();
    fake.pending[0]!.reply.resolve(fake.page(1, 1)); await initial;
    const failed = controller.postMessage("fixture post");
    fake.pending[1]!.reply.reject(new Error("post failed"));
    expect(await failed).toBe(false);
    const stale = controller.postMessage("fixture post");
    const switched = controller.selectChannel(OTHER_CHANNEL);
    fake.pending[3]!.reply.resolve(fake.page(2, 2)); await switched;
    fake.pending[2]!.reply.resolve({ ok: true });
    expect(await stale).toBe(false);
    expect(controller.state.messages.map((m) => m.id)).toEqual(["2"]);
  });
  it("preserves paged history on an overlapping refresh and follows Backend order", async () => {
    const fake = historyClient();
    const controller = new ConversationsController(fake.instance, () => undefined);
    const initial = controller.initialize(); await fake.flush();
    fake.pending[0]!.reply.resolve(fake.page(64, 33, "c33")); await initial;
    const older = controller.loadOlder();
    fake.pending[1]!.reply.resolve(fake.page(32, 1)); await older;
    const refresh = controller.applySnapshot(historySnapshot());
    fake.pending[2]!.reply.resolve(fake.page(66, 35, "c35")); await refresh;
    expect(controller.state.messages.map((m) => Number(m.id))).toEqual(Array.from({ length: 66 }, (_, i) => i + 1));
    expect(controller.state.hasMore).toBe(false);
  });

  it("keeps visible history and offers retry when head backfill fails", async () => {
    const fake = historyClient();
    const controller = new ConversationsController(fake.instance, () => undefined);
    const initial = controller.initialize(); await fake.flush();
    fake.pending[0]!.reply.resolve(fake.page(32, 1)); await initial;
    const refresh = controller.applySnapshot(historySnapshot());
    fake.pending[1]!.reply.resolve(fake.page(96, 65, "c65")); await fake.flush();
    fake.pending[2]!.reply.reject(new Error("fixture failure")); await refresh;
    expect(controller.state.messages).toHaveLength(32);
    expect(controller.state.olderError).toBe(true);
    const retry = controller.retryOlder();
    fake.pending[3]!.reply.resolve(fake.page(96, 65, "c65")); await fake.flush();
    fake.pending[4]!.reply.resolve(fake.page(64, 33, "c33")); await fake.flush();
    fake.pending[5]!.reply.resolve(fake.page(32, 1)); await retry;
    expect(controller.state.messages).toHaveLength(96);
    expect(controller.state.olderError).toBe(false);
  });

  it("does not apply an old post acknowledgement after switching Channels", async () => {
    const fake = historyClient();
    const controller = new ConversationsController(fake.instance, () => undefined);
    const initial = controller.initialize(); await fake.flush();
    fake.pending[0]!.reply.resolve(fake.page(1, 1)); await initial;
    const post = controller.postMessage("fixture post");
    const switched = controller.selectChannel(OTHER_CHANNEL);
    fake.pending[1]!.reply.resolve({ ok: true }); await post;
    fake.pending[2]!.reply.resolve(fake.page(2, 2)); await switched;
    expect(controller.state.messages.map((m) => m.id)).toEqual(["2"]);
    expect(fake.pending.filter((p) => p.operation === "conversations.channel.open")).toHaveLength(2);
  });
});

describe("posting while a head backfill is pending", () => {
  it("releases the composer after commit without waiting for history pages", async () => {
    const fake = historyClient();
    const controller = new ConversationsController(fake.instance, () => undefined);
    const initial = controller.initialize(); await fake.flush();
    fake.pending[0]!.reply.resolve(fake.page(32, 1)); await initial;
    const refresh = controller.applySnapshot(historySnapshot());
    fake.pending[1]!.reply.resolve(fake.page(96, 65, "c65")); await fake.flush();
    const post = controller.postMessage("fixture post");
    fake.pending[3]!.reply.resolve({ ok: true }); await post;
    expect(controller.state.loading).toBe(false);
    expect(fake.pending[2]!.operation).toBe("conversations.channel.page");
    fake.pending[2]!.reply.resolve(fake.page(64, 33, "c33")); await fake.flush();
    fake.pending[4]!.reply.resolve(fake.page(32, 1)); await fake.flush();
    const queued = fake.pending.find((p, i) => i > 4 && p.operation === "conversations.channel.open");
    if (queued) queued.reply.resolve(fake.page(96, 65, "c65"));
    await refresh;
  });
});

describe("invalid cursor protection", () => {
  it("stops automatic repeats when a page does not advance its cursor", async () => {
    const fake = historyClient();
    const controller = new ConversationsController(fake.instance, () => undefined);
    const initial = controller.initialize(); await fake.flush();
    fake.pending[0]!.reply.resolve(fake.page(32, 1, "same")); await initial;
    const older = controller.loadOlder();
    fake.pending[1]!.reply.resolve({ messages: [], nextBefore: "same" }); await older;
    await controller.loadOlder();
    expect(fake.pending).toHaveLength(2);
    expect(controller.state.olderError).toBe(true);
    expect(controller.state.messages).toHaveLength(32);
  });
});
