import { afterEach, expect, it, vi } from "vitest";
import type { PackageHostSnapshot, PackageHostStateNotification, PackageInstanceKey } from "./package-contracts";
import {
  BrowserPackageShellView,
  PackageShell,
  type PackageShellView,
  type PackageSnapshotBridge,
} from "./package-shell";

const first: PackageInstanceKey = { packageIdentity: "com.example.one", workspaceId: "workspace-one" };
const second: PackageInstanceKey = { packageIdentity: "com.example.two", workspaceId: "workspace-two" };
const snapshot: PackageHostSnapshot = {
  revision: 1,
  state: "running",
  packages: [
    { identity: first.packageIdentity, version: "1.0.0", displayName: "One", source: "bundled", trust: "application_bundled", uiContractDigest: "a" },
    { identity: second.packageIdentity, version: "1.0.0", displayName: "Two", source: "local", trust: "trusted_native_code_v1", uiContractDigest: "b" },
  ],
  instances: [
    { key: first, lifecycle: "active", processGeneration: 7, mountState: null },
    { key: second, lifecycle: "active", processGeneration: 9, mountState: null },
  ],
  diagnostics: [],
};

function snapshotAt(revision: number): PackageHostSnapshot {
  return { ...snapshot, revision };
}

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason: Error) => void;
} {
  let resolve: (value: T) => void = () => undefined;
  let reject: (reason: Error) => void = () => undefined;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function snapshotBridge(snapshotRequest: () => Promise<PackageHostSnapshot>): PackageSnapshotBridge {
  return {
    snapshot: snapshotRequest,
    subscribeHostState: async () => undefined,
  };
}

function mounts() {
  return {
    mount: async () => ({}),
    unmount: async () => undefined,
    suspendBackend: async () => undefined,
    recoverBackend: async () => undefined,
    disposeBackend: async () => undefined,
  };
}

function view(failures: { value: number } = { value: 0 }): PackageShellView {
  return {
    mountTarget: {},
    showFailure: () => { failures.value += 1; },
  };
}

it("accepts one initial snapshot and refreshes it from the existing host-state subscription", async () => {
  const initial = deferred<PackageHostSnapshot>();
  const notified = deferred<PackageHostSnapshot>();
  const snapshots = [initial, notified];
  let listener: ((event: PackageHostStateNotification) => void) | undefined;
  const accepted: number[] = [];
  const updated = deferred<void>();
  const shell = new PackageShell({
    snapshot: async () => snapshots.shift()!.promise,
    subscribeHostState: async (afterRevision, subscribed) => {
      expect(afterRevision).toBe(1);
      listener = subscribed;
    },
  }, mounts(), view());

  const started = shell.startSnapshotUpdates({
    update: (value) => {
      accepted.push(value.revision);
      if (value.revision === 2) updated.resolve();
    },
    fail: () => { accepted.push(-1); },
  });
  initial.resolve(snapshotAt(1));
  await started;
  expect(accepted).toEqual([1]);

  listener?.({ revision: 2, revisionGap: false });
  notified.resolve(snapshotAt(2));
  await updated.promise;
  expect(accepted).toEqual([1, 2]);
});

it("coalesces notification bursts and refreshes the current snapshot after a revision gap", async () => {
  const requests: Array<ReturnType<typeof deferred<PackageHostSnapshot>>> = [];
  const thirdRequested = deferred<void>();
  const revisionFiveAccepted = deferred<void>();
  let listener: ((event: PackageHostStateNotification) => void) | undefined;
  const accepted: number[] = [];
  const shell = new PackageShell({
    snapshot: async () => {
      const request = deferred<PackageHostSnapshot>();
      requests.push(request);
      if (requests.length === 3) thirdRequested.resolve();
      return request.promise;
    },
    subscribeHostState: async (_afterRevision, subscribed) => { listener = subscribed; },
  }, mounts(), view());

  const started = shell.startSnapshotUpdates({
    update: (value) => {
      accepted.push(value.revision);
      if (value.revision === 5) revisionFiveAccepted.resolve();
    },
    fail: () => { accepted.push(-1); },
  });
  requests[0]!.resolve(snapshotAt(1));
  await started;

  listener?.({ revision: 2, revisionGap: false });
  listener?.({ revision: 5, revisionGap: true });
  expect(requests).toHaveLength(2);
  requests[1]!.resolve(snapshotAt(2));
  await thirdRequested.promise;
  expect(requests).toHaveLength(3);
  requests[2]!.resolve(snapshotAt(5));
  await revisionFiveAccepted.promise;
  expect(accepted).toEqual([1, 2, 5]);
});

it("does not let older requests, failures, or revisions replace newer accepted state", async () => {
  const requests: Array<ReturnType<typeof deferred<PackageHostSnapshot>>> = [];
  const accepted: number[] = [];
  let failures = 0;
  const shell = new PackageShell({
    snapshot: async () => {
      const request = deferred<PackageHostSnapshot>();
      requests.push(request);
      return request.promise;
    },
    subscribeHostState: async () => undefined,
  }, mounts(), view());

  const started = shell.startSnapshotUpdates({
    update: (value) => { accepted.push(value.revision); },
    fail: () => { failures += 1; },
  });
  requests[0]!.resolve(snapshotAt(1));
  await started;

  const older = shell.refresh();
  const newer = shell.refresh();
  requests[2]!.resolve(snapshotAt(4));
  await expect(newer).resolves.toMatchObject({ revision: 4 });
  requests[1]!.reject(new Error("older failed"));
  await expect(older).resolves.toMatchObject({ revision: 4 });

  const lowerRevision = shell.refresh();
  requests[3]!.resolve(snapshotAt(3));
  await expect(lowerRevision).resolves.toMatchObject({ revision: 4 });
  expect(accepted).toEqual([1, 4]);
  expect(failures).toBe(0);
});

it("accepts a notification snapshot that finishes after a later request starts and suppresses the obsolete failure", async () => {
  const requests: Array<ReturnType<typeof deferred<PackageHostSnapshot>>> = [];
  let listener: ((event: PackageHostStateNotification) => void) | undefined;
  const accepted: number[] = [];
  const revisionTwoAccepted = deferred<void>();
  let failures = 0;
  const shell = new PackageShell({
    snapshot: async () => {
      const request = deferred<PackageHostSnapshot>();
      requests.push(request);
      return request.promise;
    },
    subscribeHostState: async (_afterRevision, subscribed) => { listener = subscribed; },
  }, mounts(), view());

  const started = shell.startSnapshotUpdates({
    update: (value) => {
      accepted.push(value.revision);
      if (value.revision === 2) revisionTwoAccepted.resolve();
    },
    fail: () => { failures += 1; },
  });
  requests[0]!.resolve(snapshotAt(1));
  await started;

  listener?.({ revision: 2, revisionGap: false });
  const later = shell.refresh();
  requests[1]!.resolve(snapshotAt(2));
  await revisionTwoAccepted.promise;
  requests[2]!.reject(new Error("later refresh failed"));

  await expect(later).resolves.toMatchObject({ revision: 2 });
  expect(accepted).toEqual([1, 2]);
  expect(failures).toBe(0);
});

it("restores live updates when visible retry follows an initial snapshot rejection", async () => {
  const requests: Array<ReturnType<typeof deferred<PackageHostSnapshot>>> = [];
  let listener: ((event: PackageHostStateNotification) => void) | undefined;
  let subscriptions = 0;
  const accepted: number[] = [];
  let failures = 0;
  const revisionTwoAccepted = deferred<void>();
  const shell = new PackageShell({
    snapshot: async () => {
      const request = deferred<PackageHostSnapshot>();
      requests.push(request);
      return request.promise;
    },
    subscribeHostState: async (_afterRevision, subscribed) => {
      subscriptions += 1;
      listener = subscribed;
    },
  }, mounts(), view());

  const started = shell.startSnapshotUpdates({
    update: (value) => {
      accepted.push(value.revision);
      if (value.revision === 2) revisionTwoAccepted.resolve();
    },
    fail: () => { failures += 1; },
  });
  requests[0]!.reject(new Error("initial snapshot failed"));
  await expect(started).rejects.toThrow("initial snapshot failed");
  expect(failures).toBe(1);
  expect(subscriptions).toBe(0);

  const retried = shell.refresh();
  requests[1]!.resolve(snapshotAt(1));
  await expect(retried).resolves.toMatchObject({ revision: 1 });
  expect(subscriptions).toBe(1);

  listener?.({ revision: 2, revisionGap: false });
  requests[2]!.resolve(snapshotAt(2));
  await revisionTwoAccepted.promise;
  expect(accepted).toEqual([1, 2]);
});

it("reports subscription rejection and establishes one live subscription on visible retry", async () => {
  const requests: Array<ReturnType<typeof deferred<PackageHostSnapshot>>> = [];
  let listener: ((event: PackageHostStateNotification) => void) | undefined;
  let subscriptionAttempts = 0;
  const accepted: number[] = [];
  let failures = 0;
  const revisionTwoAccepted = deferred<void>();
  const retrySubscriptionStarted = deferred<void>();
  const allowRetrySubscription = deferred<void>();
  const shell = new PackageShell({
    snapshot: async () => {
      const request = deferred<PackageHostSnapshot>();
      requests.push(request);
      return request.promise;
    },
    subscribeHostState: async (_afterRevision, subscribed) => {
      subscriptionAttempts += 1;
      if (subscriptionAttempts === 1) throw new Error("subscription failed");
      listener = subscribed;
      retrySubscriptionStarted.resolve();
      await allowRetrySubscription.promise;
    },
  }, mounts(), view());

  const started = shell.startSnapshotUpdates({
    update: (value) => {
      accepted.push(value.revision);
      if (value.revision === 2) revisionTwoAccepted.resolve();
    },
    fail: () => { failures += 1; },
  });
  requests[0]!.resolve(snapshotAt(1));
  await expect(started).rejects.toThrow("subscription failed");
  expect(failures).toBe(1);

  const retried = shell.refresh();
  const concurrentRefresh = shell.refresh();
  requests[1]!.resolve(snapshotAt(1));
  requests[2]!.resolve(snapshotAt(1));
  await retrySubscriptionStarted.promise;
  expect(subscriptionAttempts).toBe(2);
  allowRetrySubscription.resolve();
  await expect(Promise.all([retried, concurrentRefresh])).resolves.toEqual([
    snapshotAt(1),
    snapshotAt(1),
  ]);

  listener?.({ revision: 2, revisionGap: false });
  requests[3]!.resolve(snapshotAt(2));
  await revisionTwoAccepted.promise;
  expect(accepted).toEqual([1, 2]);
  expect(failures).toBe(1);
});

it("keeps snapshot retry failed when fallback cannot restore the rejected subscription", async () => {
  const requests: Array<ReturnType<typeof deferred<PackageHostSnapshot>>> = [];
  const retrySubscriptionStarted = deferred<void>();
  const rejectRetrySubscription = deferred<void>();
  let subscriptionAttempts = 0;
  const accepted: number[] = [];
  let failures = 0;
  const shell = new PackageShell({
    snapshot: async () => {
      const request = deferred<PackageHostSnapshot>();
      requests.push(request);
      return request.promise;
    },
    subscribeHostState: async () => {
      subscriptionAttempts += 1;
      if (subscriptionAttempts === 1) throw new Error("startup subscription failed");
      if (subscriptionAttempts === 2) {
        retrySubscriptionStarted.resolve();
        await rejectRetrySubscription.promise;
      }
      throw new Error("replacement subscription failed");
    },
  }, mounts(), view());

  const started = shell.startSnapshotUpdates({
    update: (value) => { accepted.push(value.revision); },
    fail: () => { failures += 1; },
  });
  requests[0]!.resolve(snapshotAt(1));
  await expect(started).rejects.toThrow("startup subscription failed");

  const retryA = shell.refresh();
  const retryB = shell.refresh();
  requests[1]!.resolve(snapshotAt(2));
  await retrySubscriptionStarted.promise;
  rejectRetrySubscription.reject(new Error("retry subscription failed"));
  await expect(retryA).rejects.toThrow("retry subscription failed");

  requests[2]!.reject(new Error("retry snapshot failed"));
  await expect(retryB).rejects.toThrow("replacement subscription failed");
  expect(accepted).toEqual([1, 2]);
  expect(subscriptionAttempts).toBe(3);
  expect(failures).toBe(3);
});

it.each(["snapshot", "subscription"] as const)(
  "restarts after %s startup rejection and keeps one usable subscription",
  async (failureStage) => {
    let snapshotCalls = 0;
    let subscriptionAttempts = 0;
    let listener: ((event: PackageHostStateNotification) => void) | undefined;
    const accepted: number[] = [];
    let failures = 0;
    const revisionTwoAccepted = deferred<void>();
    const shell = new PackageShell({
      snapshot: async () => {
        snapshotCalls += 1;
        if (failureStage === "snapshot" && snapshotCalls === 1) {
          throw new Error("startup snapshot failed");
        }
        return snapshotAt(snapshotCalls >= 3 ? 2 : 1);
      },
      subscribeHostState: async (_afterRevision, subscribed) => {
        subscriptionAttempts += 1;
        if (failureStage === "subscription" && subscriptionAttempts === 1) {
          throw new Error("startup subscription failed");
        }
        listener = subscribed;
      },
    }, mounts(), view());
    const observer = {
      update: (value: PackageHostSnapshot) => {
        accepted.push(value.revision);
        if (value.revision === 2) revisionTwoAccepted.resolve();
      },
      fail: () => { failures += 1; },
    };

    await expect(shell.startSnapshotUpdates(observer)).rejects.toThrow(/startup .* failed/);
    await expect(shell.startSnapshotUpdates(observer)).resolves.toBeUndefined();

    listener?.({ revision: 2, revisionGap: false });
    await revisionTwoAccepted.promise;
    const snapshotCallsAfterUpdate = snapshotCalls;
    const subscriptionsAfterUpdate = subscriptionAttempts;
    await expect(shell.startSnapshotUpdates(observer)).resolves.toBeUndefined();

    expect(accepted).toEqual([1, 2]);
    expect(failures).toBe(1);
    expect(snapshotCalls).toBe(snapshotCallsAfterUpdate);
    expect(subscriptionAttempts).toBe(subscriptionsAfterUpdate);
    expect(subscriptionAttempts).toBe(failureStage === "snapshot" ? 1 : 2);
  },
);

it("targets lifecycle actions at the supplied Package Instance and returns typed refreshed results", async () => {
  const calls: string[] = [];
  let snapshotCalls = 0;
  const mounts = {
    mount: async (instance: PackageInstanceKey) => { calls.push(`mount:${instance.packageIdentity}`); return {}; },
    unmount: async () => undefined,
    suspendBackend: async (instance: PackageInstanceKey) => { calls.push(`suspend:${instance.packageIdentity}`); },
    recoverBackend: async (instance: PackageInstanceKey) => { calls.push(`recover:${instance.packageIdentity}`); },
    disposeBackend: async (instance: PackageInstanceKey) => { calls.push(`dispose:${instance.packageIdentity}`); },
  };
  const shell = new PackageShell(snapshotBridge(async () => {
    snapshotCalls += 1;
    return snapshot;
  }), mounts, view());
  await shell.refresh();
  await shell.select(first);
  await expect(shell.suspendBackend(second)).resolves.toEqual({ snapshot, failure: null });
  await expect(shell.recoverBackend(second)).resolves.toEqual({ snapshot, failure: null });
  await expect(shell.disposeBackend(second)).resolves.toEqual({ snapshot, failure: null });

  expect(calls).toEqual([
    "mount:com.example.one",
    "suspend:com.example.two",
    "recover:com.example.two",
    "dispose:com.example.two",
  ]);
  expect(snapshotCalls).toBe(4);
});

it("keeps Reload UI separate from Backend lifecycle", async () => {
  const calls: string[] = [];
  const mounts = {
    mount: async (instance: PackageInstanceKey) => { calls.push(`mount:${instance.packageIdentity}`); return {}; },
    unmount: async () => undefined,
    suspendBackend: async () => { calls.push("suspend"); },
    recoverBackend: async () => { calls.push("recover"); },
    disposeBackend: async () => { calls.push("dispose"); },
  };
  const shell = new PackageShell(snapshotBridge(async () => snapshot), mounts, view());
  await shell.select(second);
  await shell.reloadUi();

  expect(calls).toEqual(["mount:com.example.two", "mount:com.example.two"]);
});

it("unmounts a hidden Package UI without a Backend lifecycle call", async () => {
  const calls: string[] = [];
  const mounts = {
    mount: async () => ({}),
    unmount: async () => { calls.push("unmount"); },
    suspendBackend: async () => { calls.push("suspend"); },
    recoverBackend: async () => { calls.push("recover"); },
    disposeBackend: async () => { calls.push("dispose"); },
  };
  const shell = new PackageShell(snapshotBridge(async () => snapshot), mounts, view());

  await shell.select(first);
  await shell.hide();

  expect(calls).toEqual(["unmount"]);
});

it("returns an operation failure with the refreshed snapshot without replacing the selected mount", async () => {
  const failures = { value: 0 };
  const mounts = {
    mount: async () => ({}),
    unmount: async () => undefined,
    suspendBackend: async () => { throw new Error("suspend failed"); },
    recoverBackend: async () => undefined,
    disposeBackend: async () => undefined,
  };
  const shell = new PackageShell(snapshotBridge(async () => snapshot), mounts, view(failures));
  await shell.select(first);

  await expect(shell.suspendBackend(second)).resolves.toEqual({ snapshot, failure: "operation" });
  expect(failures.value).toBe(0);
});

it("returns refresh and combined lifecycle failures while preserving the last usable result", async () => {
  const failures = { value: 0 };
  let operationFails = false;
  const mounts = {
    mount: async () => ({}),
    unmount: async () => undefined,
    suspendBackend: async () => { if (operationFails) throw new Error("suspend failed"); },
    recoverBackend: async () => undefined,
    disposeBackend: async () => undefined,
  };
  const shell = new PackageShell(snapshotBridge(async () => { throw new Error("snapshot failed"); }), mounts, view(failures));

  await expect(shell.suspendBackend(second)).resolves.toEqual({ snapshot: null, failure: "refresh" });
  operationFails = true;
  await expect(shell.suspendBackend(second)).resolves.toEqual({ snapshot: null, failure: "operation-and-refresh" });
  expect(failures.value).toBe(0);
});

it("shows Shell recovery when the selected mount reports a terminal stream failure", async () => {
  let failMount: (() => void) | undefined;
  const failures = { value: 0 };
  const mounts = {
    mount: async (_instance: PackageInstanceKey, _target: unknown, onFailure: () => void) => { failMount = onFailure; return {}; },
    unmount: async () => undefined,
    suspendBackend: async () => undefined,
    recoverBackend: async () => undefined,
    disposeBackend: async () => undefined,
  };
  const shell = new PackageShell(snapshotBridge(async () => snapshot), mounts, view(failures));
  await shell.select(first);

  failMount?.();

  expect(failures.value).toBe(1);
});

class TestElement {
  public type = "";
  public textContent = "";
  public className = "";
  public role: string | null = null;
  public children: TestElement[] = [];
  private onClick: (() => void) | undefined;

  public addEventListener(type: string, listener: () => void): void {
    if (type === "click") this.onClick = listener;
  }
  public click(): void { this.onClick?.(); }
  public replaceChildren(...children: TestElement[]): void { this.children = children; }
}

afterEach(() => { vi.unstubAllGlobals(); });

it("contains mount failure in the selected target and keeps Reload UI separate from Recover Backend", () => {
  vi.stubGlobal("document", { createElement: () => new TestElement() });
  const target = new TestElement();
  const view = new BrowserPackageShellView(target as unknown as HTMLElement);
  const calls: string[] = [];

  view.showFailure({
    reloadUi: async () => { calls.push("reload"); },
    recoverBackend: async () => { calls.push("recover"); },
  });

  expect(target.children.map(({ textContent }) => textContent)).toEqual([
    "This Package interface failed.",
    "Reload UI",
    "Recover Backend",
  ]);
  target.children[1]?.click();
  target.children[2]?.click();
  expect(calls).toEqual(["reload", "recover"]);
});
