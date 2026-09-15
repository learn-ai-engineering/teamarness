import { Channel, invoke } from "@tauri-apps/api/core";

export const TEAMARNESS_THEME_TOKENS = Object.freeze([
  "--teamarness-color-background",
  "--teamarness-color-panel",
  "--teamarness-color-text",
  "--teamarness-color-text-muted",
  "--teamarness-color-border",
  "--teamarness-color-accent",
  "--teamarness-color-danger",
  "--teamarness-color-focus",
  "--teamarness-font-body",
  "--teamarness-font-mono",
  "--teamarness-radius-control",
  "--teamarness-radius-panel",
] as const);

export type TeamarnessThemeToken = (typeof TEAMARNESS_THEME_TOKENS)[number];

const PACKAGE_UI_KIT_CSS = `
  :host, :host * { box-sizing: border-box; }

  .teamarness-control {
    border: 1px solid var(--teamarness-color-border);
    border-radius: var(--teamarness-radius-control);
    background: var(--teamarness-color-panel);
    color: var(--teamarness-color-text);
    font-family: var(--teamarness-font-body);
    padding: 0.55rem 0.8rem;
  }
  .teamarness-control--primary {
    border-color: var(--teamarness-color-accent);
    background: var(--teamarness-color-accent);
    color: var(--teamarness-color-background);
  }
  .teamarness-control--secondary {
    border-color: var(--teamarness-color-border);
    background: var(--teamarness-color-panel);
    color: var(--teamarness-color-text);
  }
  .teamarness-field { display: grid; gap: 0.35rem; color: var(--teamarness-color-text); }
  .teamarness-input {
    width: 100%;
    border: 1px solid var(--teamarness-color-border);
    border-radius: var(--teamarness-radius-control);
    background: var(--teamarness-color-background);
    color: var(--teamarness-color-text);
    font: inherit;
    padding: 0.55rem 0.65rem;
  }
  .teamarness-help { color: var(--teamarness-color-text-muted); }
  .teamarness-validation { color: var(--teamarness-color-danger); font-family: var(--teamarness-font-mono); }
  .teamarness-notice {
    border: 1px solid var(--teamarness-color-border);
    border-radius: var(--teamarness-radius-control);
    background: var(--teamarness-color-panel);
    color: var(--teamarness-color-text);
    padding: 0.75rem;
  }
  .teamarness-notice--danger { border-color: var(--teamarness-color-danger); color: var(--teamarness-color-danger); }
  .teamarness-panel {
    border: 1px solid var(--teamarness-color-border);
    border-radius: var(--teamarness-radius-panel);
    background: var(--teamarness-color-panel);
    color: var(--teamarness-color-text);
    padding: 1rem;
  }
  .teamarness-stack { display: flex; flex-direction: column; gap: 0.75rem; }
  .teamarness-inline { display: flex; flex-wrap: wrap; align-items: center; gap: 0.75rem; }
  .teamarness-focus:focus-visible,
  .teamarness-control:focus-visible,
  .teamarness-input:focus-visible {
    outline: 3px solid var(--teamarness-color-focus);
    outline-offset: 2px;
  }
`;

const installedKits = new WeakMap<ShadowRoot, { readonly style: HTMLStyleElement; users: number }>();

export function installPackageUiKit(root: ShadowRoot): () => void {
  let installation = installedKits.get(root);
  if (installation === undefined) {
    const style = root.ownerDocument.createElement("style");
    style.setAttribute("data-teamarness-ui-kit", "");
    style.textContent = PACKAGE_UI_KIT_CSS;
    installation = { style, users: 0 };
    installedKits.set(root, installation);
    root.append(style);
  }
  installation.users += 1;

  let cleaned = false;
  return () => {
    if (cleaned) return;
    cleaned = true;
    const current = installedKits.get(root);
    if (current !== installation) return;
    current.users -= 1;
    if (current.users === 0) {
      current.style.remove();
      installedKits.delete(root);
    }
  };
}

export type PackageValue = null | boolean | number | string | readonly PackageValue[] | { readonly [key: string]: PackageValue };

export type PackagePresentation = Readonly<{
  identity: string;
  version: string;
  displayName: string;
  source: "bundled" | "local";
  trust: "application_bundled" | "trusted_native_code_v1";
  lifecycle: "active";
  processGeneration: number;
}>;
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
  openSubscription(
    input: SubscriptionInput,
    deliver: (event: UiEvent) => void,
    terminate: () => void,
  ): Promise<void>;
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
  themeTokens: readonly TeamarnessThemeToken[];
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
      let failed = false;
      let delivery = Promise.resolve();

      const close = async (): Promise<void> => {
        if (closed) return;
        closed = true;
        subscriptions.delete(close);
        await transport.closeSubscription({ mount, subscription });
      };
      subscriptions.add(close);

      const fail = async (): Promise<void> => {
        if (failed) return;
        failed = true;
        try {
          await close();
        } catch {
          // The host may already have closed a terminal stream.
        }
        await transport.reportUiFault({ mount, code: "ui_exception" });
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
        () => {
          if (closed) return;
          delivery = delivery.then(fail, fail);
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
    openSubscription: async (input, deliver, terminate) => {
      type StreamMessage =
        | Readonly<{ kind: "event"; value: UiEvent }>
        | Readonly<{ kind: "failure"; value: unknown }>;
      const onEvent = makeChannel<StreamMessage>((message) => {
        if (message.kind === "event") deliver(message.value);
        else terminate();
      });
      await call("package_open_stream", { ...input, onEvent });
    },
    acknowledgeEvents: async (input) => { await call("package_acknowledge_events", input); },
    closeSubscription: async (input) => { await call("package_close_stream", input); },
    reportUiFault: async (input) => { await call("package_report_ui_fault", input); },
  };
  return Object.freeze(transport);
}
