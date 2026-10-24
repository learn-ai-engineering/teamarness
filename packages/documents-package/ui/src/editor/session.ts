import { isPackageRequestError } from "@teamarness/package-ui-sdk";
import type { TypedPackageClient } from "@teamarness/package-ui-sdk";
import { LoroDoc, VersionVector } from "loro-crdt";
import type { PeerID } from "loro-crdt";
import type { BodyReason, DocumentsContract, RequestStatus, SessionEvent } from "../contract.generated";
import { MARK_EXPAND } from "./schema";

/** How long the session waits after the last local change before it sends what it holds. */
export const SEND_IDLE_MS = 300;
/** Local changes this large are sent at once rather than at the next idle point. */
export const EXPORT_BUDGET_BYTES = 64 * 1024;
/** How long an update whose outcome is unknown waits before it is sent again. */
export const RETRY_MS = 1000;
/** How long closing a session waits for its last requests to be acknowledged. */
const DISPOSE_DRAIN_MS = 2000;
/** `documents.document.recover` answers at most this many request IDs at a time. */
const RECOVER_BATCH = 8;

type UpdateRequest = DocumentsContract["requests"]["documents.document.update"]["request"];
type SessionRequest = DocumentsContract["subscriptions"]["documents.document.session"]["request"];

/** What an editor session needs from the Package: the three session operations, and nothing else. */
export interface SessionBridge {
  open(
    request: SessionRequest,
    listener: (event: SessionEvent) => void,
    onTerminated: (error: unknown) => void,
  ): Promise<{ close(): Promise<void> }>;
  update(request: UpdateRequest): Promise<unknown>;
  recover(documentId: string, requestIds: readonly string[]): Promise<readonly RequestStatus[]>;
}

/** The bridge over the mount's Package client. */
export function clientBridge(client: TypedPackageClient<DocumentsContract>): SessionBridge {
  return {
    open: (request, listener, onTerminated) =>
      client.subscribe("documents.document.session", request, listener, { onTerminated }),
    update: (request) => client.request("documents.document.update", request),
    recover: async (documentId, requestIds) =>
      (await client.request("documents.document.recover", { documentId, requestIds })).requestStatuses,
  };
}

export type SessionState = Readonly<{
  /** `opening` until a snapshot has loaded; `ready` while it follows the body. */
  phase: "opening" | "ready" | "closed";
  editable: boolean;
  reason?: BodyReason;
  diagnosticRecordId?: string;
  sessionPeer: string | null;
  /** Requests sent or queued whose append is not yet acknowledged. */
  outstanding: number;
}>;

/** Local changes a reload discarded before the Backend acknowledged them. */
export type LostNotice = Readonly<{ requests: number; unsentBytes: number }>;

export type EditorSessionOptions = Readonly<{
  bridge: SessionBridge;
  documentId: string;
  /** Called after every state change, and when a new doc replaces the old one. */
  onChange?: (state: SessionState) => void;
  /** Called when a reload discarded local changes. */
  onLost?: (notice: LostNotice) => void;
}>;

type Outbound = { readonly requestId: string; readonly update: Uint8Array };
type Assembly = { begin: Extract<SessionEvent, { event: "begin" }>; parts: Uint8Array[] };

/**
 * One open Document as a Loro peer: it loads the body only from the Backend's snapshot, sends
 * its own changes as identified updates, and imports everyone else's from the session stream.
 *
 * Sending follows a fixed cadence. Changes are exported once the Member has been idle for
 * {@link SEND_IDLE_MS}, or at once when they reach {@link EXPORT_BUDGET_BYTES}. Each export gets a
 * fresh 128-bit request ID and is one request, sent strictly in order. A request whose outcome is
 * unknown is sent again with the same ID and bytes. When the stream ends without `resync` (the
 * Backend generation was replaced), the session reopens as the same peer, asks
 * `documents.document.recover` which outstanding requests arrived, and resends only those proven
 * absent. On `resync`, a refused update, or a lost peer, it discards the doc, opens a new session,
 * and reports what it could not keep through `onLost`.
 */
export class EditorSession {
  private readonly bridge: SessionBridge;
  private readonly documentId: string;
  private readonly onChange: (state: SessionState) => void;
  private readonly onLost: (notice: LostNotice) => void;
  private current: SessionState = { phase: "opening", editable: false, sessionPeer: null, outstanding: 0 };
  private loro: LoroDoc | null = null;
  private stream: { close(): Promise<void> } | null = null;
  private generation = 0;
  private assembly: Assembly | null = null;
  private unsubscribeLocal: (() => void) | null = null;
  /** The own-peer counter every export so far covers. */
  private sentCounter = 0;
  private unsentBytes = 0;
  private idleTimer: ReturnType<typeof setTimeout> | null = null;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private readonly queue: Outbound[] = [];
  private inFlight: Outbound | null = null;
  /** Sending waits while the session reconnects and recovery has not decided what to resend. */
  private paused = false;
  private disposed = false;
  private readonly drainWaiters: Array<() => void> = [];

  constructor(options: EditorSessionOptions) {
    this.bridge = options.bridge;
    this.documentId = options.documentId;
    this.onChange = options.onChange ?? (() => {});
    this.onLost = options.onLost ?? (() => {});
  }

  get state(): SessionState {
    return this.current;
  }

  /** The body as this peer holds it, once a snapshot has loaded. */
  get doc(): LoroDoc | null {
    return this.loro;
  }

  async start(): Promise<void> {
    await this.open(null);
  }

  /** Exports and sends local changes now, without waiting for the idle point. */
  flush(): void {
    this.clearIdle();
    const doc = this.loro;
    const peer = this.current.sessionPeer;
    if (doc === null || peer === null || this.disposed) return;
    const own = doc.oplogVersion().get(peer as PeerID) ?? 0;
    if (own > this.sentCounter) {
      const from = doc.oplogVersion().toJSON();
      if (this.sentCounter === 0) from.delete(peer as PeerID);
      else from.set(peer as PeerID, this.sentCounter);
      const update = doc.export({ mode: "update", from: new VersionVector(from) });
      this.sentCounter = own;
      this.queue.push({ requestId: newRequestId(), update });
    }
    this.unsentBytes = 0;
    this.announce();
    void this.pump();
  }

  /**
   * Sends what is left, waits briefly for its acknowledgement while the session is still open,
   * then closes the session.
   */
  async dispose(): Promise<void> {
    if (this.disposed) return;
    this.flush();
    if (!this.paused) await this.drained(DISPOSE_DRAIN_MS);
    this.disposed = true;
    this.clearIdle();
    this.clearRetry();
    this.generation += 1;
    this.unsubscribeLocal?.();
    this.unsubscribeLocal = null;
    this.current = { ...this.current, phase: "closed" };
    const stream = this.stream;
    this.stream = null;
    await stream?.close();
  }

  private announce(changes: Partial<SessionState> = {}): void {
    this.current = { ...this.current, ...changes, outstanding: this.queue.length };
    if (this.queue.length === 0) for (const waiter of this.drainWaiters.splice(0)) waiter();
    this.onChange(this.current);
  }

  /** Opens the session stream; `resumePeer` asks to continue as the same peer. */
  private async open(resumePeer: string | null): Promise<void> {
    const generation = ++this.generation;
    this.assembly = null;
    const request: SessionRequest = resumePeer === null
      ? { documentId: this.documentId }
      : { documentId: this.documentId, resumePeer };
    try {
      const stream = await this.bridge.open(
        request,
        (event) => { if (generation === this.generation) this.receive(event, resumePeer); },
        () => { if (generation === this.generation && !this.disposed) this.reconnect(); },
      );
      if (generation !== this.generation || this.disposed) {
        await stream.close();
        return;
      }
      this.stream = stream;
    } catch {
      if (generation === this.generation && !this.disposed) this.retryOpen(resumePeer);
    }
  }

  private retryOpen(resumePeer: string | null): void {
    this.clearRetry();
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null;
      void this.open(resumePeer);
    }, RETRY_MS);
  }

  private receive(event: SessionEvent, resumePeer: string | null): void {
    if (event.documentId !== this.documentId || this.disposed) return;
    switch (event.event) {
      case "begin":
        this.assembly = { begin: event, parts: [] };
        return;
      case "chunk": {
        const assembly = this.assembly;
        if (assembly === null || event.chunkIndex !== assembly.parts.length) {
          void this.reload();
          return;
        }
        assembly.parts.push(fromBase64(event.chunk));
        return;
      }
      case "end": {
        const assembly = this.assembly;
        this.assembly = null;
        if (assembly === null || assembly.parts.length !== assembly.begin.chunks) {
          void this.reload();
          return;
        }
        this.load(assembly, resumePeer);
        return;
      }
      case "update":
        this.loro?.import(fromBase64(event.update));
        return;
      case "status":
        this.announce({ editable: event.editable, reason: event.reason, diagnosticRecordId: event.diagnosticRecordId });
        return;
      case "resync":
        void this.reload();
        return;
    }
  }

  private load({ begin, parts }: Assembly, resumePeer: string | null): void {
    const snapshot = concat(parts);
    const resumed = resumePeer !== null && begin.sessionPeer === resumePeer && this.loro !== null;
    if (resumed && this.loro !== null) {
      // The same peer continues: keep every local change, and learn what arrived meanwhile.
      if (snapshot.byteLength > 0) this.loro.import(snapshot);
    } else {
      // A different peer cannot continue this peer's operations: what was unacknowledged is lost.
      if (resumePeer !== null) this.discardLocal();
      this.paused = false;
      this.unsubscribeLocal?.();
      const doc = new LoroDoc();
      if (snapshot.byteLength > 0) doc.import(snapshot);
      doc.setPeerId(begin.sessionPeer as PeerID);
      doc.configTextStyle(MARK_EXPAND);
      this.loro = doc;
      this.sentCounter = doc.oplogVersion().get(begin.sessionPeer as PeerID) ?? 0;
      this.unsubscribeLocal = doc.subscribeLocalUpdates((bytes) => this.local(bytes.byteLength));
    }
    this.announce({
      phase: "ready", editable: begin.editable, reason: begin.reason,
      diagnosticRecordId: begin.diagnosticRecordId, sessionPeer: begin.sessionPeer,
    });
    if (resumed) void this.recoverOutstanding();
  }

  private local(bytes: number): void {
    if (this.disposed) return;
    this.unsentBytes += bytes;
    if (this.unsentBytes >= EXPORT_BUDGET_BYTES) {
      this.flush();
      return;
    }
    this.clearIdle();
    this.idleTimer = setTimeout(() => {
      this.idleTimer = null;
      this.flush();
    }, SEND_IDLE_MS);
  }

  private async pump(): Promise<void> {
    const next = this.queue[0];
    const peer = this.current.sessionPeer;
    if (this.inFlight !== null || this.paused || this.retryTimer !== null) return;
    if (next === undefined || peer === null) return;
    this.inFlight = next;
    // A request answered after a reconnect or reload is decided by recovery or the new snapshot.
    const generation = this.generation;
    let failure: unknown = null;
    try {
      await this.bridge.update({
        documentId: this.documentId, sessionPeer: peer, requestId: next.requestId, update: toBase64(next.update),
      });
    } catch (error) {
      failure = error ?? new Error("update failed");
    }
    if (generation !== this.generation || this.inFlight !== next) return;
    this.inFlight = null;
    if (failure === null) {
      this.queue.shift();
      this.announce();
      void this.pump();
      return;
    }
    if (this.disposed) return;
    if (outcomeUnknown(failure)) {
      // Same ID, same bytes: the Backend answers a retry from what it already appended.
      this.retryTimer = setTimeout(() => {
        this.retryTimer = null;
        void this.pump();
      }, RETRY_MS);
      return;
    }
    void this.reload();
  }

  /** Resolves once every queued request is acknowledged, or after `ms`. */
  private drained(ms: number): Promise<void> {
    if (this.queue.length === 0 && this.inFlight === null) return Promise.resolve();
    return new Promise((resolve) => {
      const timer = setTimeout(finish, ms);
      const waiter = () => { if (this.queue.length === 0) finish(); };
      function finish(): void {
        clearTimeout(timer);
        resolve();
      }
      this.drainWaiters.push(waiter);
    });
  }

  /** The stream ended without `resync`: reconnect as the same peer, then recover. */
  private reconnect(): void {
    this.paused = true;
    this.inFlight = null;
    this.clearRetry();
    this.stream = null;
    void this.open(this.current.sessionPeer);
  }

  /** Decides each outstanding request from the Backend's own record, then resends the absent. */
  private async recoverOutstanding(): Promise<void> {
    const generation = this.generation;
    const settled = new Set<string>();
    try {
      const ids = this.queue.map((entry) => entry.requestId);
      for (let start = 0; start < ids.length; start += RECOVER_BATCH) {
        const statuses = await this.bridge.recover(this.documentId, ids.slice(start, start + RECOVER_BATCH));
        for (const status of statuses) {
          if (status.inclusion === "included") settled.add(status.requestId);
          else if (status.inclusion === "unresolved") throw new Error("unresolved");
        }
      }
    } catch {
      if (generation !== this.generation || this.disposed) return;
      this.retryTimer = setTimeout(() => {
        this.retryTimer = null;
        void this.recoverOutstanding();
      }, RETRY_MS);
      return;
    }
    if (generation !== this.generation || this.disposed) return;
    for (let index = this.queue.length - 1; index >= 0; index -= 1) {
      if (settled.has(this.queue[index]!.requestId)) this.queue.splice(index, 1);
    }
    this.paused = false;
    this.announce();
    void this.pump();
  }

  /** Discards the doc and every unacknowledged change, then opens a new session. */
  private async reload(): Promise<void> {
    if (this.disposed) return;
    this.discardLocal();
    this.loro = null;
    this.paused = false;
    this.announce({ phase: "opening", sessionPeer: null });
    const stream = this.stream;
    this.stream = null;
    this.generation += 1;
    await stream?.close().catch(() => {});
    if (!this.disposed) await this.open(null);
  }

  private discardLocal(): void {
    this.clearIdle();
    this.clearRetry();
    this.unsubscribeLocal?.();
    this.unsubscribeLocal = null;
    const doc = this.loro;
    const peer = this.current.sessionPeer;
    const unexported = doc !== null && peer !== null
      ? (doc.oplogVersion().get(peer as PeerID) ?? 0) > this.sentCounter : false;
    const notice = { requests: this.queue.length, unsentBytes: unexported ? Math.max(this.unsentBytes, 1) : 0 };
    this.queue.length = 0;
    this.inFlight = null;
    this.unsentBytes = 0;
    for (const waiter of this.drainWaiters.splice(0)) waiter();
    this.sentCounter = 0;
    if (notice.requests > 0 || notice.unsentBytes > 0) this.onLost(notice);
  }

  private clearIdle(): void {
    if (this.idleTimer !== null) clearTimeout(this.idleTimer);
    this.idleTimer = null;
  }

  private clearRetry(): void {
    if (this.retryTimer !== null) clearTimeout(this.retryTimer);
    this.retryTimer = null;
  }
}

/** Whether a failed request may still have been appended: only an explicit refusal says no. */
function outcomeUnknown(error: unknown): boolean {
  if (!isPackageRequestError(error)) return true;
  return error.packageError === null ? error.retryable : error.packageError.retryable;
}

export function newRequestId(): string {
  const bytes = new Uint8Array(16);
  globalThis.crypto.getRandomValues(bytes);
  return [...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

export function toBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let start = 0; start < bytes.length; start += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(start, start + 0x8000));
  }
  return globalThis.btoa(binary);
}

export function fromBase64(text: string): Uint8Array {
  const binary = globalThis.atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

function concat(parts: readonly Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((total, part) => total + part.byteLength, 0));
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.byteLength;
  }
  return out;
}
