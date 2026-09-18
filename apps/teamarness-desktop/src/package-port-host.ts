import type { PackagePresentation, PackageValue, WorkspacePresentation } from "@teamarness/package-ui-sdk";
import type { PackageHostTransport, PackageInstanceKey } from "./package-contracts";
import {
  MAX_OPEN_SUBSCRIPTIONS,
  MAX_OPERATION_LENGTH,
  MAX_PAYLOAD_BYTES,
  MAX_PENDING_REQUESTS,
  isPackageValue,
  packageValueByteLength,
  parseBridgeErrorEnvelope,
  parseFrameMessage,
  type BridgeErrorEnvelope,
  type FrameToShellMessage,
  type ThemeSnapshot,
} from "./package-frame/protocol";

export type PackageUiFaultSource =
  | "load"
  | "mount"
  | "unmount"
  | "listener"
  | "error"
  | "unhandledrejection"
  | "protocol"
  | "navigation"
  | "connect_timeout"
  | "stream_terminated";

export type PackagePortHostOptions = Readonly<{
  mount: string;
  instance: PackageInstanceKey;
  packageIdentity: string;
  port: MessagePort;
  transport: PackageHostTransport;
  onFault(source: PackageUiFaultSource): void;
}>;

export type PackageMountMessage = Readonly<{
  entrypoint: string;
  package: PackagePresentation;
  workspace: WorkspacePresentation;
  uiHostVersion: number;
  theme: ThemeSnapshot;
}>;

type PendingRequest = { readonly request: string; cancelled: boolean };
type OpenSubscription = { readonly subscription: string; delivered: bigint; acknowledged: bigint; closed: boolean };

const HOST_NAMESPACE = "teamarness.package-host";
const BRIDGE_NAMESPACE = "teamarness.desktop-ui-bridge";
const DIAGNOSTIC_ID = "desktop-ui-bridge";

/**
 * The Shell half of one Package UI Mount's bridge. Mount identity comes from the
 * port this host owns; no message field ever selects a mount, a Package or a host id.
 */
export class PackagePortHost {
  private readonly mount: string;
  private readonly instance: PackageInstanceKey;
  private readonly packageIdentity: string;
  private readonly port: MessagePort;
  private readonly transport: PackageHostTransport;
  private readonly onFault: (source: PackageUiFaultSource) => void;

  private started = false;
  private disposed = false;
  private faulted = false;
  private greeted = false;
  private nextRequest = 0;
  private nextSubscription = 0;

  private readonly pending = new Map<number, PendingRequest>();
  private readonly subscriptions = new Map<number, OpenSubscription>();

  private resolveReady: (() => void) | null = null;
  private settleMounted: ((error?: unknown) => void) | null = null;
  private settleUnmount: ((ok: boolean) => void) | null = null;

  public readonly ready: Promise<void>;
  public readonly mounted: Promise<void>;

  public constructor(options: PackagePortHostOptions) {
    this.mount = options.mount;
    this.instance = options.instance;
    this.packageIdentity = options.packageIdentity;
    this.port = options.port;
    this.transport = options.transport;
    this.onFault = options.onFault;
    this.ready = new Promise<void>((resolve) => { this.resolveReady = resolve; });
    this.mounted = new Promise<void>((resolve, reject) => {
      this.settleMounted = (error) => { if (error === undefined) resolve(); else reject(error); };
    });
    this.mounted.catch(() => undefined);
  }

  public start(): void {
    if (this.started || this.disposed) return;
    this.started = true;
    this.port.onmessage = (event: MessageEvent) => { this.receive(event.data); };
    this.port.start();
  }

  public sendMount(input: PackageMountMessage): void {
    this.post({
      kind: "mount",
      entrypoint: input.entrypoint,
      package: input.package,
      workspace: input.workspace,
      uiHostVersion: input.uiHostVersion,
      theme: input.theme,
    });
  }

  public sendTheme(theme: ThemeSnapshot): void {
    this.post({ kind: "theme", theme });
  }

  public requestUnmount(): Promise<boolean> {
    if (this.disposed || !this.greeted) return Promise.resolve(false);
    if (this.settleUnmount !== null) return Promise.resolve(false);
    const acknowledged = new Promise<boolean>((resolve) => { this.settleUnmount = resolve; });
    this.post({ kind: "unmount" });
    return acknowledged;
  }

  public async dispose(): Promise<void> {
    if (this.disposed) return;
    this.disposed = true;
    const closing = [...this.subscriptions.values()]
      .filter((entry) => !entry.closed)
      .map(async (entry) => {
        entry.closed = true;
        try {
          await this.transport.closeSubscription({ mount: this.mount, subscription: entry.subscription });
        } catch {
          // A stream the Package Host already ended needs no further close.
        }
      });
    this.subscriptions.clear();
    this.pending.clear();
    this.port.onmessage = null;
    try {
      this.port.close();
    } catch {
      // A port the frame already discarded needs no further close.
    }
    this.settleMounted?.(new Error("Package UI mount closed before it mounted"));
    this.settleMounted = null;
    this.settleUnmount?.(false);
    this.settleUnmount = null;
    await Promise.all(closing);
  }

  private post(message: unknown): void {
    if (this.disposed) return;
    this.port.postMessage(message);
  }

  private fault(source: PackageUiFaultSource): void {
    if (this.faulted) return;
    this.faulted = true;
    this.settleMounted?.(new Error(`Package UI fault: ${source}`));
    this.settleMounted = null;
    this.onFault(source);
  }

  private receive(data: unknown): void {
    if (this.disposed) return;
    const message = parseFrameMessage(data);
    if (message === null) {
      this.fault("protocol");
      return;
    }
    if (!this.greeted) {
      if (message.kind !== "hello") {
        this.fault("protocol");
        return;
      }
      this.greeted = true;
      this.resolveReady?.();
      this.resolveReady = null;
      return;
    }
    this.dispatch(message);
  }

  private dispatch(message: FrameToShellMessage): void {
    switch (message.kind) {
      case "hello":
        this.fault("protocol");
        return;
      case "mounted":
        this.settleMounted?.();
        this.settleMounted = null;
        return;
      case "request":
        this.startRequest(message.id, message.operation, message.payload);
        return;
      case "cancel":
        this.cancelRequest(message.id);
        return;
      case "subscribe":
        this.startSubscription(message.id, message.operation, message.payload);
        return;
      case "ack":
        this.acknowledge(message.id, message.sequence);
        return;
      case "unsubscribe":
        this.stopSubscription(message.id);
        return;
      case "fault":
        this.fault(message.source);
        return;
      case "unmounted":
        this.settleUnmount?.(message.ok);
        this.settleUnmount = null;
        return;
      default:
        this.fault("protocol");
    }
  }

  private isFreeId(id: number): boolean {
    return !this.pending.has(id) && !this.subscriptions.has(id);
  }

  private admits(operation: string, payload: unknown): payload is PackageValue {
    if (operation.length === 0 || operation.length > MAX_OPERATION_LENGTH) return false;
    if (!isPackageValue(payload)) return false;
    return packageValueByteLength(payload) <= MAX_PAYLOAD_BYTES;
  }

  private startRequest(id: number, operation: string, payload: unknown): void {
    if (!this.isFreeId(id)) {
      this.fault("protocol");
      return;
    }
    if (!this.admits(operation, payload)) {
      this.post({ kind: "response", id, ok: false, error: this.contractViolation() });
      return;
    }
    if (this.pending.size >= MAX_PENDING_REQUESTS) {
      this.post({ kind: "response", id, ok: false, error: this.overloaded() });
      return;
    }
    this.nextRequest += 1;
    const entry: PendingRequest = { request: `r${this.nextRequest}`, cancelled: false };
    this.pending.set(id, entry);
    void this.transport.request({ mount: this.mount, request: entry.request, operation, payload }).then(
      (value) => {
        if (this.disposed || entry.cancelled || this.pending.get(id) !== entry) return;
        this.pending.delete(id);
        if (!isPackageValue(value)) {
          this.post({ kind: "response", id, ok: false, error: this.bridgeUnavailable() });
          return;
        }
        this.post({ kind: "response", id, ok: true, value });
      },
      (error: unknown) => {
        if (this.disposed || entry.cancelled || this.pending.get(id) !== entry) return;
        this.pending.delete(id);
        this.post({ kind: "response", id, ok: false, error: this.envelope(error) });
      },
    );
  }

  private cancelRequest(id: number): void {
    const entry = this.pending.get(id);
    if (entry === undefined) {
      this.fault("protocol");
      return;
    }
    entry.cancelled = true;
    this.pending.delete(id);
    void this.transport.cancelRequest({ mount: this.mount, request: entry.request }).catch(() => undefined);
  }

  private startSubscription(id: number, operation: string, payload: unknown): void {
    if (!this.isFreeId(id)) {
      this.fault("protocol");
      return;
    }
    if (!this.admits(operation, payload)) {
      this.post({ kind: "subscribe_failed", id, error: this.contractViolation() });
      return;
    }
    if (this.subscriptions.size >= MAX_OPEN_SUBSCRIPTIONS) {
      this.post({ kind: "subscribe_failed", id, error: this.overloaded() });
      return;
    }
    this.nextSubscription += 1;
    const entry: OpenSubscription = {
      subscription: `s${this.nextSubscription}`,
      delivered: -1n,
      acknowledged: -1n,
      closed: false,
    };
    this.subscriptions.set(id, entry);
    void this.transport.openSubscription(
      { mount: this.mount, subscription: entry.subscription, operation, payload },
      (event) => { this.deliver(id, entry, event.mount, event.subscription, event.sequence, event.payload); },
      (outcome) => { this.terminate(id, entry, outcome.error); },
    ).then(
      () => {
        if (this.disposed || this.subscriptions.get(id) !== entry) return;
        this.post({ kind: "subscribed", id });
      },
      (error: unknown) => {
        if (this.disposed || this.subscriptions.get(id) !== entry) return;
        this.subscriptions.delete(id);
        this.post({ kind: "subscribe_failed", id, error: this.envelope(error) });
      },
    );
  }

  private deliver(
    id: number,
    entry: OpenSubscription,
    mount: string,
    subscription: string,
    sequence: string,
    payload: PackageValue,
  ): void {
    if (this.disposed || entry.closed || this.subscriptions.get(id) !== entry) return;
    if (mount !== this.mount || subscription !== entry.subscription) return;
    let value: bigint;
    try {
      value = BigInt(sequence);
    } catch {
      return;
    }
    if (value > entry.delivered) entry.delivered = value;
    this.post({ kind: "event", id, sequence, payload });
  }

  /**
   * Ends one subscription.
   *
   * A completion and a Package-owned failure both reach the Package's own frame and leave the
   * mount healthy; only a host-owned failure is still a mount fault (SHELL-R06 as amended).
   */
  private terminate(id: number, entry: OpenSubscription, failure: unknown): void {
    if (this.disposed || entry.closed || this.subscriptions.get(id) !== entry) return;
    entry.closed = true;
    this.subscriptions.delete(id);
    void this.transport
      .closeSubscription({ mount: this.mount, subscription: entry.subscription })
      .catch(() => undefined);
    if (failure === null) {
      this.post({ kind: "stream_terminated", id, error: null });
      return;
    }
    const error = parseBridgeErrorEnvelope(failure);
    if (error !== null && error.packageError !== null) {
      this.post({ kind: "stream_terminated", id, error });
      return;
    }
    this.post({ kind: "stream_terminated", id, error: error ?? this.bridgeUnavailable() });
    this.fault("stream_terminated");
  }

  private acknowledge(id: number, sequence: string): void {
    const entry = this.subscriptions.get(id);
    if (entry === undefined || entry.closed) {
      this.fault("protocol");
      return;
    }
    let value: bigint;
    try {
      value = BigInt(sequence);
    } catch {
      this.fault("protocol");
      return;
    }
    if (value > entry.delivered || value <= entry.acknowledged) {
      this.fault("protocol");
      return;
    }
    entry.acknowledged = value;
    void this.transport
      .acknowledgeEvents({ mount: this.mount, subscription: entry.subscription, throughSequence: sequence })
      .catch(() => undefined);
  }

  private stopSubscription(id: number): void {
    const entry = this.subscriptions.get(id);
    if (entry === undefined) {
      this.fault("protocol");
      return;
    }
    entry.closed = true;
    this.subscriptions.delete(id);
    void this.transport
      .closeSubscription({ mount: this.mount, subscription: entry.subscription })
      .catch(() => undefined)
      .finally(() => { this.post({ kind: "unsubscribed", id }); });
  }

  private shellEnvelope(namespace: string, code: string, retryable: boolean): BridgeErrorEnvelope {
    return Object.freeze({
      version: 2,
      namespace,
      code,
      stage: "ui",
      affectedPackage: this.packageIdentity,
      affectedInstance: Object.freeze({
        packageIdentity: this.instance.packageIdentity,
        workspaceId: this.instance.workspaceId,
      }),
      retryable,
      recovery: "none",
      diagnosticId: DIAGNOSTIC_ID,
      packageError: null,
    });
  }

  private contractViolation(): BridgeErrorEnvelope {
    return this.shellEnvelope(HOST_NAMESPACE, "ui_contract_violation", false);
  }

  private overloaded(): BridgeErrorEnvelope {
    return this.shellEnvelope(HOST_NAMESPACE, "overloaded", true);
  }

  private bridgeUnavailable(): BridgeErrorEnvelope {
    return this.shellEnvelope(BRIDGE_NAMESPACE, "ui_bridge_unavailable", true);
  }

  private envelope(error: unknown): BridgeErrorEnvelope {
    return parseBridgeErrorEnvelope(error) ?? this.bridgeUnavailable();
  }
}
