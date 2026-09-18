import type { PackageClient, PackageSubscription, PackageValue } from "@teamarness/package-ui-sdk";
import type { ShellToFrameMessage } from "./protocol";

export type PortPackageClient = Readonly<{
  client: PackageClient;
  handle(message: ShellToFrameMessage): boolean;
  dispose(): Promise<void>;
}>;

type PortSink = Readonly<{ postMessage(message: unknown): void }>;

type SubscriptionState = {
  closed: boolean;
  failed: boolean;
  delivery: Promise<void>;
  listener(event: PackageValue): void | Promise<void>;
  settleOpen: ((error?: unknown) => void) | null;
  settleClose: (() => void) | null;
};

const CLOSED_MESSAGE = "Package UI mount is closed";

function cancelled(): DOMException {
  return new DOMException("The Package request was cancelled", "AbortError");
}

/**
 * The frame half of the Package UI bridge: one `PackageClient` bound to one
 * transferred port. Identity is the port; no message ever names a mount.
 */
export function createPortPackageClient(
  port: PortSink,
  hooks: Readonly<{ onListenerFault: () => void }>,
): PortPackageClient {
  let nextId = 1;
  let disposed = false;
  const pending = new Map<number, Readonly<{ resolve(value: PackageValue): void; reject(error: unknown): void }>>();
  const subscriptions = new Map<number, SubscriptionState>();

  const post = (message: unknown): void => {
    if (disposed) return;
    port.postMessage(message);
  };

  const failSubscription = (id: number, state: SubscriptionState): void => {
    if (state.failed) return;
    state.failed = true;
    if (!state.closed) {
      state.closed = true;
      post({ kind: "unsubscribe", id });
    }
    subscriptions.delete(id);
    state.settleClose?.();
    state.settleClose = null;
    hooks.onListenerFault();
  };

  const closeSubscription = async (id: number): Promise<void> => {
    const state = subscriptions.get(id);
    if (state === undefined || state.closed) return;
    state.closed = true;
    if (disposed) {
      subscriptions.delete(id);
      return;
    }
    const acknowledged = new Promise<void>((resolve) => { state.settleClose = resolve; });
    post({ kind: "unsubscribe", id });
    await acknowledged;
  };

  const client: PackageClient = {
    async request<TResponse extends PackageValue>(
      operation: string,
      payload: PackageValue,
      options: Readonly<{ signal?: AbortSignal }> = {},
    ): Promise<TResponse> {
      if (disposed) throw new Error(CLOSED_MESSAGE);
      if (options.signal?.aborted) throw cancelled();
      const id = nextId++;
      const answered = new Promise<PackageValue>((resolve, reject) => {
        pending.set(id, Object.freeze({ resolve, reject }));
      });
      post({ kind: "request", id, operation, payload });

      const signal = options.signal;
      let stopWatchingAbort = (): void => undefined;
      if (signal !== undefined) {
        const onAbort = (): void => {
          const entry = pending.get(id);
          if (entry === undefined) return;
          pending.delete(id);
          post({ kind: "cancel", id });
          entry.reject(cancelled());
        };
        signal.addEventListener("abort", onAbort, { once: true });
        stopWatchingAbort = () => { signal.removeEventListener("abort", onAbort); };
      }
      try {
        return (await answered) as TResponse;
      } finally {
        stopWatchingAbort();
      }
    },

    async subscribe<TEvent extends PackageValue>(
      operation: string,
      payload: PackageValue,
      listener: (event: TEvent) => void | Promise<void>,
    ): Promise<PackageSubscription> {
      if (disposed) throw new Error(CLOSED_MESSAGE);
      const id = nextId++;
      const state: SubscriptionState = {
        closed: false,
        failed: false,
        delivery: Promise.resolve(),
        listener: listener as (event: PackageValue) => void | Promise<void>,
        settleOpen: null,
        settleClose: null,
      };
      subscriptions.set(id, state);
      const opened = new Promise<void>((resolve, reject) => {
        state.settleOpen = (error) => { if (error === undefined) resolve(); else reject(error); };
      });
      post({ kind: "subscribe", id, operation, payload });
      try {
        await opened;
      } catch (error) {
        subscriptions.delete(id);
        throw error;
      }
      return Object.freeze({ close: () => closeSubscription(id) });
    },
  };

  const handle = (message: ShellToFrameMessage): boolean => {
    switch (message.kind) {
      case "response": {
        const entry = pending.get(message.id);
        if (entry === undefined) return true;
        pending.delete(message.id);
        if (message.ok) entry.resolve(message.value);
        else entry.reject(message.error);
        return true;
      }
      case "subscribed": {
        const state = subscriptions.get(message.id);
        state?.settleOpen?.();
        if (state !== undefined) state.settleOpen = null;
        return true;
      }
      case "subscribe_failed": {
        const state = subscriptions.get(message.id);
        state?.settleOpen?.(message.error);
        if (state !== undefined) state.settleOpen = null;
        return true;
      }
      case "unsubscribed": {
        const state = subscriptions.get(message.id);
        subscriptions.delete(message.id);
        state?.settleClose?.();
        if (state !== undefined) state.settleClose = null;
        return true;
      }
      case "stream_terminated": {
        const state = subscriptions.get(message.id);
        if (state === undefined) return true;
        state.closed = true;
        subscriptions.delete(message.id);
        state.settleOpen?.(new Error("Package event stream ended"));
        state.settleOpen = null;
        state.settleClose?.();
        state.settleClose = null;
        return true;
      }
      case "event": {
        const state = subscriptions.get(message.id);
        if (state === undefined || state.closed) return true;
        state.delivery = state.delivery.then(async () => {
          if (state.closed) return;
          await state.listener(message.payload);
          if (!state.closed) post({ kind: "ack", id: message.id, sequence: message.sequence });
        }).catch(() => { failSubscription(message.id, state); });
        return true;
      }
      default:
        return false;
    }
  };

  const dispose = async (): Promise<void> => {
    if (disposed) return;
    for (const [id, state] of [...subscriptions]) {
      if (!state.closed) {
        state.closed = true;
        post({ kind: "unsubscribe", id });
      }
      subscriptions.delete(id);
      state.settleOpen?.(new Error(CLOSED_MESSAGE));
      state.settleOpen = null;
      state.settleClose?.();
      state.settleClose = null;
    }
    disposed = true;
    for (const [id, entry] of [...pending]) {
      pending.delete(id);
      entry.reject(new Error(CLOSED_MESSAGE));
    }
    await Promise.resolve();
  };

  return Object.freeze({ client: Object.freeze(client), handle, dispose });
}
