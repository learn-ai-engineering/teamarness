import { typedPackageClient, type PackageClient, type PackageValue } from "@teamarness/package-ui-sdk";
import { expect, it } from "vitest";
import type { ConversationsContract } from "./contract.generated";
import { ConversationsController } from "./controller";

const channelId = "00000000000000000000000000000000";
const snapshot = { availability: "current" as const, channels: [{ id: channelId, slug: "general", canHide: false, unreadCount: 0 }] };
const position = (n: number) => String(Math.round(n * 10)).padStart(6, "0");
const message = (n: number) => ({ id: String(n), position: position(n), author: "author", markdown: "sample", isLocal: false });
function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>(done => { resolve = done; });
  return { promise, resolve };
}
type Change = { revision: string; id: string; position: string; kind: "insert" | "remove"; status?: "ready" | "pending"; message?: ReturnType<typeof message> };

/** Stateful test adapter: strict raw-row cursor, actual arrival revisions, independently sortable truth. */
function historyBackend(size = 64) {
  const records = Array.from({ length: size }, (_, i) => i + 1);
  const unreadable = new Set<number>();
  const changes: Change[] = [];
  const calls: string[] = [];
  let failFeed = false;
  let failOpen = false;
  let failPage = false;
  let epoch = "a".repeat(32);
  let epochCount = 0;
  let loadedFrontier: string | null = null;
  let failResolve = false;
  let pageSelected: (() => Promise<void>) | null = null;
  let pageGate: ReturnType<typeof deferred> | null = null;
  let pageRelease: ReturnType<typeof deferred> | null = null;
  let openGate: ReturnType<typeof deferred> | null = null;
  let openSelected: ReturnType<typeof deferred> | null = null;
  let openRelease: ReturnType<typeof deferred> | null = null;
  let resolveGate: ReturnType<typeof deferred> | null = null;
  const raw: PackageClient = {
    request: async <T extends PackageValue>(operation: string, payload: PackageValue): Promise<T> => {
      calls.push(operation);
      if (operation === "conversations.snapshot") return snapshot as unknown as T;
      if (operation === "conversations.channel.open" || operation === "conversations.channel.page") {
        if (operation === "conversations.channel.open" && failOpen) {
          failOpen = false; throw new Error("temporary open failure");
        }
        if (operation === "conversations.channel.page" && failPage) {
          failPage = false; throw new Error("temporary older-page failure");
        }
        const before = (payload as { before?: string }).before;
        const selected = records.filter(n => !before || position(n) < before)
          .sort((a, b) => b - a).slice(0, 32);
        loadedFrontier = selected.length === 32 ? position(selected[31]!) : null;
        const response = { messages: selected.filter(n => !unreadable.has(n)).map(message),
          pending: selected.filter(n => unreadable.has(n)).map(n => ({ id: String(n), position: position(n), status: "pending" })),
          epoch, revision: String(changes.length),
          ...(loadedFrontier ? { nextBefore: loadedFrontier } : {}) };
        if (operation === "conversations.channel.page" && pageGate) {
          pageRelease = pageGate; pageGate = null;
          await pageRelease.promise;
        }
        if (operation === "conversations.channel.page" && pageSelected) await pageSelected();
        if (operation === "conversations.channel.open" && openGate) {
          openRelease = openGate; openGate = null;
          openSelected?.resolve(); openSelected = null;
          await openRelease.promise;
        }
        return response as unknown as T;
      }
      if (operation === "conversations.channel.changes") {
        if ((payload as { epoch: string }).epoch !== epoch) {
          return { events: [], continuation: "0", highWater: "0", resyncRequired: true } as unknown as T;
        }
        if (failFeed) { failFeed = false; throw new Error("temporary change failure"); }
        const after = Number((payload as { after: string }).after);
        const events = changes.filter(change => Number(change.revision) > after).slice(0, 32);
        return { events, continuation: events.at(-1)?.revision ?? String(changes.length), highWater: String(changes.length) } as unknown as T;
      }
      if (operation === "conversations.message.post") {
        const n = Math.max(...records) + 1;
        records.push(n);
        changes.push({ revision: String(changes.length + 1), id: String(n), position: position(n),
          kind: "insert", status: "ready", message: message(n) });
        return { ok: true } as unknown as T;
      }
      if (operation === "conversations.channel.resolve") {
        if (failResolve) { failResolve = false; throw new Error("temporary resolve failure"); }
        const ids = (payload as { ids: string[] }).ids.map(Number);
        const response = { messages: ids.filter(n => records.includes(n) && !unreadable.has(n)).map(message),
          pending: ids.filter(n => records.includes(n) && unreadable.has(n)).map(n => ({ id: String(n), position: position(n), status: "pending" })),
          missing: ids.filter(n => !records.includes(n)).map(String) };
        if (resolveGate) await resolveGate.promise;
        return response as unknown as T;
      }
      throw new Error(`unexpected operation ${operation}`);
    },
    subscribe: async () => ({ close: async () => undefined }),
  };
  return { client: typedPackageClient<ConversationsContract>(raw), calls,
    insert(n: number, ready = true) {
      records.push(n);
      if (!ready) unreadable.add(n);
      changes.push({ revision: String(changes.length + 1), id: String(n), position: position(n), kind: "insert",
        status: ready ? "ready" : "pending", ...(ready ? { message: message(n) } : {}) });
    },
    remove(n: number) {
      records.splice(records.indexOf(n), 1);
      changes.push({ revision: String(changes.length + 1), id: String(n), position: position(n), kind: "remove" });
    },
    makeUnreadable(n: number) { unreadable.add(n); },
    makeReadable(n: number) { unreadable.delete(n); },
    failNextFeed() { failFeed = true; },
    failNextOpen() { failOpen = true; },
    failNextPage() { failPage = true; },
    resetEpoch() { epoch = (++epochCount).toString(16).padStart(32, "0"); changes.length = 0; },
    failNextResolve() { failResolve = true; },
    pausePage() { pageGate = deferred(); },
    releasePage() { pageRelease?.resolve(); pageRelease = null; },
    onPageSelected(callback: (() => Promise<void>) | null) { pageSelected = callback; },
    pauseOpen(): Promise<void> { openGate = deferred(); openSelected = deferred(); return openSelected.promise; },
    releaseOpen() { openRelease?.resolve(); openRelease = null; },
    pauseResolve() { resolveGate = deferred(); },
    releaseResolve() { resolveGate?.resolve(); resolveGate = null; },
    expected(): string[] { return [...records].sort((a, b) => a - b).map(String); },
    expectedLoadedReadable(): string[] { return records.filter(n => !unreadable.has(n)
      && (!loadedFrontier || position(n) >= loadedFrontier)).sort((a, b) => a - b).map(String); },
    loadedFrontier(): string | null { return loadedFrontier; },
    unreadableIds(): number[] { return records.filter(n => unreadable.has(n)); },
  };
}

it("installs replay after a failed initial open is explicitly retried", async () => {
  const backend = historyBackend();
  backend.failNextOpen();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  expect(controller.state.olderError).toBe(true);
  await controller.retryOlder();
  await controller.loadOlder();
  backend.insert(31.5);
  backend.remove(31);
  await controller.applySnapshot(snapshot);
  expect(controller.state.messages.map(m => m.id)).toEqual(backend.expected());
});

it("reopens an expired projection epoch without losing loaded history", async () => {
  const backend = historyBackend();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  await controller.loadOlder();
  backend.remove(64);
  backend.resetEpoch();
  await controller.applySnapshot(snapshot);
  expect(controller.state.messages.some(m => m.id === "64")).toBe(false);
  expect(controller.state.messages.some(m => m.id === "1")).toBe(true);
  expect(controller.state.feedError).toBe(false);
  backend.insert(65);
  await controller.applySnapshot(snapshot);
  expect(controller.state.messages.at(-1)?.id).toBe("65");
});

it("preserves an independent older-page failure across epoch rebuild until explicit retry", async () => {
  const backend = historyBackend();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  backend.failNextPage();
  await controller.loadOlder();
  expect(controller.state.olderError).toBe(true);
  backend.resetEpoch();
  await controller.applySnapshot(snapshot);
  expect(controller.state.olderError).toBe(true);
  const calls = backend.calls.filter(operation => operation === "conversations.channel.page").length;
  await controller.loadOlder();
  expect(backend.calls.filter(operation => operation === "conversations.channel.page")).toHaveLength(calls);
  await controller.retryOlder();
  expect(controller.state.olderError).toBe(false);
  expect(controller.state.messages.some(m => m.id === "1")).toBe(true);
});

it("keeps a failed older-page retry actionable while epoch reconstruction is opening", async () => {
  const backend = historyBackend();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  backend.failNextPage();
  await controller.loadOlder();
  expect(controller.state.olderError).toBe(true);
  backend.resetEpoch();
  const selected = backend.pauseOpen();
  const rebuilding = controller.applySnapshot(snapshot);
  await selected;
  const pages = backend.calls.filter(operation => operation === "conversations.channel.page").length;
  await controller.retryOlder();
  expect(controller.state.olderError).toBe(true);
  expect(backend.calls.filter(operation => operation === "conversations.channel.page")).toHaveLength(pages);
  backend.releaseOpen();
  await rebuilding;
  expect(controller.state.olderError).toBe(true);
  await controller.retryOlder();
  expect(controller.state.olderError).toBe(false);
  expect(controller.state.messages.some(m => m.id === "1")).toBe(true);
  expect(controller.state.messages.map(m => m.id)).toEqual(backend.expected());
});

it("preserves a failed pending-resolve across resync until explicit retry", async () => {
  const backend = historyBackend();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  backend.insert(65, false);
  backend.failNextResolve();
  await controller.applySnapshot(snapshot);
  expect(controller.state.resolveError).toBe(true);
  backend.resetEpoch();
  await controller.applySnapshot(snapshot);
  expect(controller.state.resolveError).toBe(true);
  expect(controller.state.pendingHistory).toBe(1);
  const calls = backend.calls.filter(operation => operation === "conversations.channel.resolve").length;
  backend.makeReadable(65);
  await controller.applySnapshot(snapshot);
  expect(backend.calls.filter(operation => operation === "conversations.channel.resolve")).toHaveLength(calls);
  await controller.retryPending();
  expect(controller.state.resolveError).toBe(false);
  expect(controller.state.messages.at(-1)?.id).toBe("65");
});

it("retains loaded pending-only raw rows across epoch rebuild and explicit retry", async () => {
  const backend = historyBackend();
  for (let n = 1; n <= 32; n++) backend.makeUnreadable(n);
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  while (controller.state.hasMore) await controller.loadOlder();
  expect(controller.state.pendingHistory).toBe(32);
  backend.resetEpoch();
  await controller.applySnapshot(snapshot);
  expect(controller.state.pendingHistory).toBe(32);
  backend.makeReadable(20);
  await controller.retryPending();
  expect(controller.state.messages.some(m => m.id === "20")).toBe(true);
});

it("keeps a long loaded range until bounded epoch rebuild continues on retry", async () => {
  const backend = historyBackend(1100);
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  while (controller.state.hasMore) await controller.loadOlder();
  expect(controller.state.messages).toHaveLength(1100);
  backend.resetEpoch();
  await controller.applySnapshot(snapshot);
  expect(controller.state.feedError).toBe(true);
  expect(controller.state.messages).toHaveLength(1100);
  await controller.retryFeed();
  expect(controller.state.feedError).toBe(false);
  expect(controller.state.messages.map(m => m.id)).toEqual(backend.expected());
});

it("rejects an epoch-old older page after resync rebuilds the visible range", async () => {
  const backend = historyBackend();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  backend.pausePage();
  const older = controller.loadOlder();
  backend.remove(31);
  backend.resetEpoch();
  await controller.applySnapshot(snapshot);
  backend.releasePage();
  await older;
  expect(controller.state.messages.some(m => m.id === "31")).toBe(false);
  expect(controller.state.messages.some(m => m.id === "33")).toBe(true);
  expect(controller.state.messages.some(m => m.id === "32")).toBe(false);
  expect(controller.state.feedError).toBe(false);
});

it("replays a notification delivered during resync-open from the replacement watermark", async () => {
  const backend = historyBackend();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  backend.resetEpoch();
  backend.pauseOpen();
  const resync = controller.applySnapshot(snapshot);
  // The adapter captures the open response before suspending its reply.
  await Promise.resolve(); await Promise.resolve();
  backend.insert(65);
  const notification = controller.applySnapshot(snapshot);
  backend.releaseOpen();
  await resync; await notification;
  expect(controller.state.messages.at(-1)?.id).toBe("65");
});

it("catches up from the initial page watermark once the subscription is installed", async () => {
  const backend = historyBackend();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  backend.insert(65);
  await controller.catchUpAfterSubscribe();
  expect(controller.state.messages.at(-1)?.id).toBe("65");
});

it("replays a snapshot delivered during a Channel switch after the switch-open watermark", async () => {
  const other = "1".repeat(32);
  const channels = [snapshot.channels[0]!, { id: other, slug: "other", canHide: false, unreadCount: 0 }];
  const gate = deferred();
  const calls: string[] = [];
  const raw: PackageClient = {
    request: async <T extends PackageValue>(operation: string, payload: PackageValue): Promise<T> => {
      calls.push(operation);
      if (operation === "conversations.snapshot") return { ...snapshot, channels } as unknown as T;
      if (operation === "conversations.channel.open") {
        if ((payload as { channelId: string }).channelId === other) {
          await gate.promise;
          return { messages: [], epoch: "a".repeat(32), revision: "0" } as unknown as T;
        }
        return { messages: [], epoch: "a".repeat(32), revision: "0" } as unknown as T;
      }
      if (operation === "conversations.channel.changes") return {
        events: [{ revision: "1", id: "65", position: position(65), kind: "insert", status: "ready", message: message(65) }],
        continuation: "1", highWater: "1",
      } as unknown as T;
      throw new Error(`unexpected operation ${operation}`);
    },
    subscribe: async () => ({ close: async () => undefined }),
  };
  const controller = new ConversationsController(typedPackageClient<ConversationsContract>(raw), () => undefined);
  await controller.initialize();
  const switching = controller.selectChannel(other);
  const notification = controller.applySnapshot({ ...snapshot, channels });
  expect(notification).toBeUndefined(); // In-flight open receives a signal, not a settled replay.
  expect(controller.state.messages).toEqual([]);
  gate.resolve();
  await switching;
  expect(controller.state.messages.map(m => m.id)).toEqual(["65"]);
  expect(calls.filter(operation => operation === "conversations.channel.open")).toHaveLength(2);
});

it("leaves the loaded Message array unchanged for an empty change batch", async () => {
  const backend = historyBackend();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  const messages = controller.state.messages;
  await controller.applySnapshot(snapshot);
  expect(controller.state.messages).toBe(messages);
});

it("matches independently sorted truth across fixed-seed short history sequences", async () => {
  const covered = new Set<string>();
  for (let seed = 1; seed <= 12; seed++) {
    let randomState = seed;
    const random = () => {
      randomState ^= randomState << 13;
      randomState ^= randomState >>> 17;
      randomState ^= randomState << 5;
      return randomState >>> 0;
    };
    const backend = historyBackend();
    const controller = new ConversationsController(backend.client, () => undefined);
    const interior = Array.from({ length: 63 }, (_, index) => index + 1.5);
    const operations: string[] = [];
    let tail = 64;
    await controller.initialize();
    for (let step = 0; step < 24; step++) {
      const choice = random() % 7;
      if (choice === 0 && interior.length) {
        backend.insert(interior.splice(random() % interior.length, 1)[0]!);
        operations.push("interior");
      } else if (choice === 1 && backend.expected().length) {
        backend.remove(Number(backend.expected()[random() % backend.expected().length]));
        operations.push("remove");
      } else if (choice === 2) {
        backend.resetEpoch();
        operations.push("epoch");
      } else if (choice === 3) {
        backend.insert(++tail, false);
        operations.push("deferred-access");
      } else if (choice === 4 && backend.unreadableIds().length) {
        backend.makeReadable(backend.unreadableIds()[random() % backend.unreadableIds().length]!);
        await controller.retryPending();
        operations.push("access-ready");
      } else if (choice === 5 && controller.state.hasMore) {
        await controller.loadOlder();
        operations.push("page");
      } else {
        backend.insert(++tail);
        operations.push("tail");
      }
      covered.add(operations.at(-1)!);
      await controller.applySnapshot(snapshot);
      const context = `seed=${seed}, operations=${operations.join(",")}`;
      expect(controller.state.messages.map(m => m.id), context).toEqual(backend.expectedLoadedReadable());
      expect(controller.state.nextBefore, context).toBe(backend.loadedFrontier());
      expect(controller.state.hasMore, context).toBe(backend.loadedFrontier() !== null);
      expect(controller.state.feedError, context).toBe(false);
    }
  }
  expect(covered).toEqual(new Set(["interior", "remove", "epoch", "deferred-access", "access-ready", "page", "tail"]));
});

it("replays a burst beyond one bounded change batch without resetting the older cursor", async () => {
  const backend = historyBackend();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  const cursor = controller.state.nextBefore;
  for (let n = 65; n <= 140; n++) backend.insert(n);
  await controller.applySnapshot(snapshot);
  expect(controller.state.messages.map(m => m.id)).toEqual(backend.expected().filter(id => Number(id) >= 33));
  expect(controller.state.nextBefore).toBe(cursor);
  expect(backend.calls.filter(operation => operation === "conversations.channel.changes")).toHaveLength(3);
});

it("finds an insert below the raw lower cursor on older paging, including a full terminal page", async () => {
  const backend = historyBackend();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  backend.insert(10.5);
  await controller.applySnapshot(snapshot);
  expect(controller.state.messages.some(m => m.id === "10.5")).toBe(false);
  await controller.loadOlder();
  await controller.loadOlder();
  expect(controller.state.messages.map(m => m.id)).toEqual(backend.expected());
  expect(controller.state.hasMore).toBe(false);
  expect(backend.calls.filter(operation => operation === "conversations.channel.open")).toHaveLength(1);
});

it("does not lose an interior insert projected after older rows were selected", async () => {
  const backend = historyBackend();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  backend.pausePage();
  const older = controller.loadOlder();
  // The adapter captured 32..1 and its revision before suspending the reply.
  backend.insert(10.5);
  await controller.applySnapshot(snapshot);
  backend.releasePage();
  await older;
  while (controller.state.hasMore) await controller.loadOlder();
  expect(controller.state.messages.map(m => m.id)).toEqual(backend.expected());
  expect(controller.state.olderError).toBe(false);
});

it("does not refetch a page for an insert below its returned raw cursor", async () => {
  const backend = historyBackend();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  backend.pausePage();
  const older = controller.loadOlder();
  backend.insert(0.5);
  await controller.applySnapshot(snapshot);
  backend.releasePage();
  await older;
  expect(backend.calls.filter(operation => operation === "conversations.channel.page")).toHaveLength(1);
  await controller.loadOlder();
  expect(controller.state.messages.map(m => m.id)).toEqual(backend.expected());
  expect(controller.state.olderError).toBe(false);
});

it("stops invalidated pages at a bounded failure and retries only on request", async () => {
  const backend = historyBackend();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  let attempts = 0;
  backend.onPageSelected(async () => {
    backend.insert(1 + (2 * ++attempts - 1) / 10);
    await controller.applySnapshot(snapshot);
  });
  await controller.loadOlder();
  expect(attempts).toBe(32);
  expect(controller.state.olderError).toBe(true);
  await controller.applySnapshot(snapshot);
  await controller.loadOlder();
  expect(attempts).toBe(32);
  backend.onPageSelected(null);
  await controller.retryOlder();
  while (controller.state.hasMore) await controller.loadOlder();
  expect(controller.state.messages.map(m => m.id)).toEqual(backend.expected());
  expect(controller.state.olderError).toBe(false);
});

it("keeps post and older page ownership while a change replay overlaps them", async () => {
  const backend = historyBackend();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  backend.pausePage();
  const older = controller.loadOlder();
  const posted = await controller.postMessage("new");
  expect(posted).toBe(true);
  await controller.applySnapshot(snapshot);
  backend.releasePage();
  await older;
  expect(controller.state.messages.map(m => m.id)).toEqual(backend.expected());
  expect(controller.state.posting).toBe(false);
});

it("replays an interior insert with an older cursor still open, preserving sorted truth", async () => {
  const backend = historyBackend();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  await controller.loadOlder();
  expect(controller.state.hasMore).toBe(true);
  backend.insert(31.5);
  await controller.applySnapshot(snapshot);
  expect(controller.state.messages.map(m => m.id)).toEqual(backend.expected());
  expect(backend.calls).toContain("conversations.channel.changes");
});

it("removes a cached Message after hide and does not reset the older cursor", async () => {
  const backend = historyBackend();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  await controller.loadOlder();
  const cursor = controller.state.nextBefore;
  backend.remove(31);
  await controller.applySnapshot(snapshot);
  expect(controller.state.messages.map(m => m.id)).toEqual(backend.expected());
  expect(controller.state.nextBefore).toBe(cursor);
});

it("retains pending identity until explicit retry succeeds without another notification", async () => {
  const backend = historyBackend();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  await controller.loadOlder();
  backend.insert(31.5, false);
  await controller.applySnapshot(snapshot);
  expect(controller.state.pendingHistory).toBe(1);
  expect(controller.state.messages.some(m => m.id === "31.5")).toBe(false);
  backend.makeReadable(31.5);
  await controller.retryPending();
  expect(controller.state.pendingHistory).toBe(0);
  expect(controller.state.messages.map(m => m.id)).toEqual(backend.expected());
});

it("keeps a pending raw slot from an older page even when it has no ready rows", async () => {
  const backend = historyBackend();
  for (let n = 1; n <= 32; n++) backend.makeUnreadable(n);
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  await controller.loadOlder();
  expect(controller.state.pendingHistory).toBe(32);
  expect(controller.state.nextBefore).toBeNull();
  backend.makeReadable(20);
  await controller.retryPending();
  expect(controller.state.messages.map(m => m.id)).toContain("20");
  expect(controller.state.pendingHistory).toBe(31);
});

it("walks a pending-only raw page to reach readable older rows", async () => {
  const backend = historyBackend(96);
  for (let n = 33; n <= 64; n++) backend.makeUnreadable(n);
  // The scrollable head is ready; the first older page is pending-only.
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  await controller.loadOlder();
  expect(controller.state.messages.map(m => m.id)).toEqual([
    ...Array.from({ length: 32 }, (_, i) => String(i + 1)),
    ...Array.from({ length: 32 }, (_, i) => String(i + 65)),
  ]);
  expect(controller.state.pendingHistory).toBe(32);
});

it("does not resurrect a removed row from an older page in flight", async () => {
  const backend = historyBackend();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  backend.pausePage();
  const older = controller.loadOlder();
  backend.remove(31);
  await controller.applySnapshot(snapshot);
  backend.releasePage();
  await older;
  expect(controller.state.messages.some(m => m.id === "31")).toBe(false);
});

it("does not resurrect a removed pending row when its resolve completes", async () => {
  const backend = historyBackend();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  backend.insert(65, false);
  await controller.applySnapshot(snapshot);
  backend.makeReadable(65);
  backend.pauseResolve();
  const resolving = controller.retryPending();
  backend.remove(65);
  await controller.applySnapshot(snapshot);
  backend.releaseResolve();
  await resolving;
  expect(controller.state.messages.some(m => m.id === "65")).toBe(false);
  expect(controller.state.pendingHistory).toBe(0);
});

it("uses a snapshot as a bounded signal to resolve newly readable history", async () => {
  const backend = historyBackend();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  backend.insert(65, false);
  await controller.applySnapshot(snapshot);
  backend.makeReadable(65);
  await controller.applySnapshot(snapshot);
  expect(controller.state.messages.at(-1)?.id).toBe("65");
});

it("rotates retries across a pending set larger than one request", async () => {
  const backend = historyBackend();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  for (let n = 1; n <= 33; n++) backend.insert(64 + n, false);
  await controller.applySnapshot(snapshot);
  expect(controller.state.pendingHistory).toBe(33);
  backend.makeReadable(97);
  await controller.retryPending();
  expect(controller.state.messages.at(-1)?.id).toBe("97");
});

it("distinguishes a failed resolve from pending access and waits for explicit retry", async () => {
  const backend = historyBackend();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  backend.insert(65, false);
  backend.failNextResolve();
  await controller.applySnapshot(snapshot);
  expect(controller.state.resolveError).toBe(true);
  const calls = backend.calls.filter(call => call === "conversations.channel.resolve").length;
  backend.makeReadable(65);
  await controller.applySnapshot(snapshot);
  expect(backend.calls.filter(call => call === "conversations.channel.resolve")).toHaveLength(calls);
  await controller.retryPending();
  expect(controller.state.resolveError).toBe(false);
  expect(controller.state.messages.at(-1)?.id).toBe("65");
});

it("a failed feed needs explicit retry and does not clear older-page ownership", async () => {
  const backend = historyBackend();
  const controller = new ConversationsController(backend.client, () => undefined);
  await controller.initialize();
  backend.insert(65);
  backend.failNextFeed();
  await controller.applySnapshot(snapshot);
  expect(controller.state.feedError).toBe(true);
  const callCount = backend.calls.filter(call => call === "conversations.channel.changes").length;
  await controller.applySnapshot(snapshot);
  expect(backend.calls.filter(call => call === "conversations.channel.changes")).toHaveLength(callCount);
  await controller.retryFeed();
  expect(controller.state.feedError).toBe(false);
  expect(controller.state.messages.at(-1)?.id).toBe("65");
});
