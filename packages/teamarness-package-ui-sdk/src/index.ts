import { Channel, invoke } from "@tauri-apps/api/core";

export type PackageValue = null | boolean | number | string | readonly PackageValue[] | { readonly [key: string]: PackageValue };

export type PackagePresentation = Readonly<{ identity: string; displayName: string }>;
export type WorkspacePresentation = Readonly<{ id: string; name: string }>;

export type UiEvent = Readonly<{
  mount: string;
  subscription: string;
  sequence: string;
  payload: PackageValue;
}>;

export type RequestInput = Readonly<{
  mount: string;
  request: string;
  operation: string;
  payload: PackageValue;
}>;

export type SubscriptionInput = Readonly<{
  mount: string;
  subscription: string;
  operation: string;
  payload: PackageValue;
}>;

export interface PackageTransport {
  request(input: RequestInput): Promise<PackageValue>;
  cancelRequest(input: Readonly<{ mount: string; request: string }>): Promise<void>;
  openSubscription(input: SubscriptionInput, deliver: (event: UiEvent) => void): Promise<void>;
  acknowledgeEvents(input: Readonly<{ mount: string; subscription: string; throughSequence: string }>): Promise<void>;
  closeSubscription(input: Readonly<{ mount: string; subscription: string }>): Promise<void>;
  reportUiFault(input: Readonly<{ mount: string; code: "ui_exception" }>): Promise<void>;
}

export interface PackageSubscription {
  close(): Promise<void>;
}

export interface PackageClient {
  request<TResponse extends PackageValue = PackageValue>(
    operation: string,
    payload: PackageValue,
    options?: Readonly<{ signal?: AbortSignal }>,
  ): Promise<TResponse>;
  subscribe<TEvent extends PackageValue = PackageValue>(
    operation: string,
    payload: PackageValue,
    listener: (event: TEvent) => void | Promise<void>,
  ): Promise<PackageSubscription>;
}

export type PackageMountContext = Readonly<{
  root: ShadowRoot;
  package: PackagePresentation;
  workspace: WorkspacePresentation;
  uiHostVersion: number;
  themeTokens: readonly string[];
  client: PackageClient;
}>;

export type PackageUnmount = () => void | Promise<void>;
export type PackageUiModule = Readonly<{
  mount(context: PackageMountContext): PackageUnmount | Promise<PackageUnmount>;
}>;

function aborted(): DOMException {
  return new DOMException("The Package request was cancelled", "AbortError");
}

export function createPackageClient(
  mount: string,
  transport: PackageTransport,
): Readonly<{ client: PackageClient; dispose: () => Promise<void> }> {
  let nextRequest = 1;
  let nextSubscription = 1;
  let disposed = false;
  const subscriptions = new Set<() => Promise<void>>();

  const client: PackageClient = {
    async request<TResponse extends PackageValue>(
      operation: string,
      payload: PackageValue,
      options: Readonly<{ signal?: AbortSignal }> = {},
    ): Promise<TResponse> {
      if (disposed) throw new Error("Package UI mount is closed");
      const request = `request-${nextRequest++}`;
      if (options.signal?.aborted) throw aborted();

      let removeAbort: () => void = () => {};
      const cancellation = new Promise<never>((_resolve, reject) => {
        const onAbort = (): void => {
          void transport.cancelRequest({ mount, request }).finally(() => reject(aborted()));
        };
        options.signal?.addEventListener("abort", onAbort, { once: true });
        removeAbort = () => options.signal?.removeEventListener("abort", onAbort);
      });
      try {
        return await Promise.race([
          transport.request({ mount, request, operation, payload }) as Promise<TResponse>,
          cancellation,
        ]);
      } finally {
        removeAbort();
      }
    },

    async subscribe<TEvent extends PackageValue>(
      operation: string,
      payload: PackageValue,
      listener: (event: TEvent) => void | Promise<void>,
    ): Promise<PackageSubscription> {
      if (disposed) throw new Error("Package UI mount is closed");
      const subscription = `subscription-${nextSubscription++}`;
      let closed = false;
      let delivery = Promise.resolve();

      const close = async (): Promise<void> => {
        if (closed) return;
        closed = true;
        subscriptions.delete(close);
        await transport.closeSubscription({ mount, subscription });
      };
      subscriptions.add(close);

      const fail = async (): Promise<void> => {
        try {
          await close();
        } finally {
          await transport.reportUiFault({ mount, code: "ui_exception" });
        }
      };

      await transport.openSubscription(
        { mount, subscription, operation, payload },
        (event) => {
          if (closed) return;
          delivery = delivery.then(async () => {
            if (closed) return;
            if (event.mount !== mount || event.subscription !== subscription) {
              throw new Error("Package event route changed");
            }
            await listener(event.payload as TEvent);
            if (!closed) {
              await transport.acknowledgeEvents({
                mount,
                subscription,
                throughSequence: event.sequence,
              });
            }
          }).catch(fail);
        },
      );
      return Object.freeze({ close });
    },
  };

  const dispose = async (): Promise<void> => {
    if (disposed) return;
    disposed = true;
    await Promise.all([...subscriptions].map((close) => close()));
  };

  return Object.freeze({ client: Object.freeze(client), dispose });
}

export function createTauriPackageTransport(
  call: typeof invoke = invoke,
  makeChannel: <T>(handler: (message: T) => void) => Channel<T> = (handler) => new Channel(handler),
): PackageTransport {
  const transport: PackageTransport = {
    request: async (input) => call<PackageValue>("package_request", input),
    cancelRequest: async (input) => { await call("package_cancel_request", input); },
    openSubscription: async (input, deliver) => {
      const onEvent = makeChannel(deliver);
      await call("package_open_stream", { ...input, onEvent });
    },
    acknowledgeEvents: async (input) => { await call("package_acknowledge_events", input); },
    closeSubscription: async (input) => { await call("package_close_stream", input); },
    reportUiFault: async (input) => { await call("package_report_ui_fault", input); },
  };
  return Object.freeze(transport);
}
