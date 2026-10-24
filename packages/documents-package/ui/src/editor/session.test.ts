import { createPackageRequestError } from "@teamarness/package-ui-sdk/testing";
import { LoroDoc, decodeImportBlobMeta } from "loro-crdt";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RequestStatus, SessionEvent } from "../contract.generated";
import { EXPORT_BUDGET_BYTES, EditorSession, RESUME_ATTEMPTS, RETRY_MS, SEND_IDLE_MS, fromBase64, toBase64 } from "./session";
import type { LostNotice, SessionBridge } from "./session";

const DOCUMENT = "d".repeat(64);
/** The Backend sends snapshots in 18 KiB raw chunks: 24 KiB once base64-encoded. */
const CHUNK = 18 * 1024;

type Sent = { requestId: string; sessionPeer: string; update: string };
type Answer = "ok" | "unknown" | "refuse" | "hold" | "lost_session";
type Live = { listener: (event: SessionEvent) => void; terminated: (error: unknown) => void };

/**
 * The Backend as a session sees it: a body, a peer per session, relay of each accepted update to
 * every other session, `status` with the durable request ID, and recovery by request ID.
 */
class FakeBackend {
  readonly body = new LoroDoc();
  readonly sent: Sent[] = [];
  readonly appended = new Set<string>();
  readonly opened: Array<{ resumePeer?: string; sessionPeer: string }> = [];
  answers: Answer[] = [];
  /** A replacement generation that cannot prove the old peer was this UI's hands out a new one. */
  refuseResume = false;
  /** The body waits for missing causal history: it is not editable and refuses updates as pending. */
  pending = false;
  /**
   * When set, a closed stream's session stays registered this long after the close is
   * acknowledged, as the Backend's session task notices the close only later.
   */
  releaseAfterMs: number | null = null;
  private nextPeer = 1000n;
  private readonly live = new Map<string, Live>();
  private readonly held: Array<{ resolve: () => void; reject: (error: unknown) => void; sent: Sent }> = [];
  /** Sessions the Backend ended whose streams have not reported it yet. */
  readonly ended = new Map<string, Live>();

  constructor() {
    this.body.setPeerId(1n);
    this.body.getText("body").insert(0, "genesis");
    this.body.commit();
  }

  bridge(): SessionBridge {
    return {
      open: async (request, listener, terminated) => {
        const resume = request.resumePeer;
        const sessionPeer = resume !== undefined && !this.live.has(resume) && !this.refuseResume
          ? resume : String(this.nextPeer++);
        this.opened.push({ resumePeer: resume, sessionPeer });
        const live = { listener, terminated };
        this.live.set(sessionPeer, live);
        queueMicrotask(() => this.sendOpening(listener, sessionPeer));
        // Closing an old stream must not end a newer session resumed as the same peer.
        const release = () => { if (this.live.get(sessionPeer) === live) this.live.delete(sessionPeer); };
        return {
          close: async () => {
            if (this.releaseAfterMs === null) release();
            else setTimeout(release, this.releaseAfterMs);
          },
        };
      },
      update: async (request) => {
        const sent = { requestId: request.requestId, sessionPeer: request.sessionPeer, update: request.update };
        this.sent.push(sent);
        const answer = this.pending ? "unknown" : this.answers.shift() ?? "ok";
        if (answer === "unknown") throw createPackageRequestError({ packageError: { code: "documents.document_pending", retryable: true } });
        if (answer === "lost_session") {
          // The Backend no longer knows the session (it ended); its stream ends only later.
          this.ended.set(request.sessionPeer, this.live.get(request.sessionPeer)!);
          this.live.delete(request.sessionPeer);
          throw createPackageRequestError({ packageError: { code: "documents.unknown_session", retryable: false } });
        }
        if (answer === "refuse") {
          this.resync(request.sessionPeer);
          throw createPackageRequestError({ packageError: { code: "documents.schema_violation", retryable: false } });
        }
        if (answer === "hold") {
          await new Promise<void>((resolve, reject) => this.held.push({ resolve, reject, sent }));
        }
        this.accept(sent);
        return { requestId: request.requestId };
      },
      recover: async (_documentId, requestIds) => requestIds.map((requestId): RequestStatus => (
        this.appended.has(requestId) && !this.pending
          ? { requestId, inclusion: "included", durability: "durable" }
          : this.pending
            ? { requestId, inclusion: "unresolved", durability: "unresolved" }
            : { requestId, inclusion: "absent", durability: "unresolved" })),
    };
  }

  /**
   * `begin`, the snapshot chunks and `end`, as the Backend opens a session. A pending body has no
   * snapshot. `between` runs after `begin`, while the opening is still arriving.
   */
  private sendOpening(listener: Live["listener"], sessionPeer: string, between: () => void = () => {}): void {
    const snapshot = this.pending ? new Uint8Array() : this.body.export({ mode: "snapshot" });
    const chunks: Uint8Array[] = [];
    for (let start = 0; start < snapshot.byteLength; start += CHUNK) chunks.push(snapshot.subarray(start, start + CHUNK));
    listener({ event: "begin", documentId: DOCUMENT, sessionPeer, version: "0", editable: !this.pending, chunks: chunks.length });
    between();
    chunks.forEach((chunk, chunkIndex) => listener({ event: "chunk", documentId: DOCUMENT, chunkIndex, chunk: toBase64(chunk) }));
    listener({ event: "end", documentId: DOCUMENT });
  }

  /** Sends an open session a fresh opening as the same peer, on the same stream. */
  reopen(peer: string, between?: () => void): void {
    const session = this.live.get(peer);
    if (session !== undefined) this.sendOpening(session.listener, peer, between);
  }

  /** Sends an open session a chunk it cannot place: a malformed opening. */
  malformed(peer: string): void {
    this.live.get(peer)?.listener({ event: "chunk", documentId: DOCUMENT, chunkIndex: 7, chunk: "" });
  }

  /** Imports a remote record without relaying it, as when the catch-up is too large for one update. */
  deliverUnrelayed(update: Uint8Array): void {
    this.body.import(update);
  }

  /** The body's records stay; every session stream ends and held requests fail unknowably. */
  replaceGeneration(applyHeld: number): void {
    for (const [index, held] of this.held.splice(0).entries()) {
      if (index < applyHeld) this.accept(held.sent);
      held.reject(createPackageRequestError({ code: "backend_unavailable", retryable: true }));
    }
    const live = [...this.live.values()];
    this.live.clear();
    for (const session of live) session.terminated(createPackageRequestError({ code: "backend_unavailable", retryable: true }));
  }

  /** Ends the streams of sessions the Backend already ended. */
  terminateEnded(): void {
    for (const session of this.ended.values()) session.terminated(createPackageRequestError({ code: "backend_unavailable", retryable: true }));
    this.ended.clear();
  }

  /** Fails the held requests as the Backend answers a session it no longer knows. */
  failHeld(code: string): void {
    for (const held of this.held.splice(0)) held.reject(createPackageRequestError({ packageError: { code, retryable: false } }));
  }

  /**
   * Imports a remote record, as the record log delivers it. One whose causal history is missing
   * waits, and the body is pending until it arrives; then sessions get what the body gained.
   */
  deliverRemote(update: Uint8Array): void {
    const before = this.body.oplogVersion();
    const status = this.body.import(update);
    const waiting = status.pending !== undefined && status.pending !== null;
    if (waiting !== this.pending) {
      this.pending = waiting;
      for (const session of this.live.values()) session.listener({ event: "status", documentId: DOCUMENT, durable: [], editable: !waiting });
    }
    if (this.body.oplogVersion().compare(before) === 0) return;
    const gained = toBase64(this.body.export({ mode: "update", from: before }));
    for (const session of this.live.values()) session.listener({ event: "update", documentId: DOCUMENT, update: gained });
  }

  resync(peer: string): void {
    const session = this.live.get(peer);
    this.live.delete(peer);
    session?.listener({ event: "resync", documentId: DOCUMENT });
  }

  /** Validates, appends and relays one update, as the real Backend does. */
  private accept(sent: Sent): void {
    if (this.appended.has(sent.requestId)) return;
    const bytes = fromBase64(sent.update);
    this.body.import(bytes);
    this.appended.add(sent.requestId);
    for (const [peer, session] of this.live) {
      if (peer !== sent.sessionPeer) session.listener({ event: "update", documentId: DOCUMENT, update: sent.update });
      session.listener({ event: "status", documentId: DOCUMENT, durable: [sent.requestId], editable: true });
    }
  }
}

async function opened(backend: FakeBackend, lost: LostNotice[] = []): Promise<EditorSession> {
  const session = new EditorSession({ bridge: backend.bridge(), documentId: DOCUMENT, onLost: (notice) => lost.push(notice) });
  await session.start();
  await vi.advanceTimersByTimeAsync(0);
  expect(session.state.phase).toBe("ready");
  return session;
}

function type(session: EditorSession, text: string, at = 0): void {
  const doc = session.doc!;
  doc.getText("body").insert(at, text);
  doc.commit();
}

function text(doc: LoroDoc | null): string {
  return doc?.getText("body").toString() ?? "";
}

/** Text that compresses poorly, so its update is about its own size. */
function noise(bytes: number): string {
  let out = "";
  for (let index = 0; out.length < bytes; index += 1) out += (index * 2654435761 >>> 0).toString(36);
  return out.slice(0, bytes);
}

describe("EditorSession", () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  it("loads only from the chunked snapshot and edits as its session peer", async () => {
    const backend = new FakeBackend();
    backend.body.getText("body").insert(7, noise(60 * 1024));
    backend.body.commit();
    const session = await opened(backend);
    expect(text(session.doc)).toBe(text(backend.body));
    expect(session.state.sessionPeer).toBe("1000");
    expect(session.doc!.peerIdStr).toBe("1000");
    expect(session.state.editable).toBe(true);
    await session.dispose();
  });

  it("sends after 300 ms idle, once per idle period, with only its own operations", async () => {
    const backend = new FakeBackend();
    const session = await opened(backend);
    type(session, "a");
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS - 1);
    type(session, "b");
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS - 1);
    expect(backend.sent).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(1);
    expect(backend.sent).toHaveLength(1);
    expect(backend.sent[0]!.requestId).toMatch(/^[0-9a-f]{32}$/);
    expect(text(backend.body)).toBe("bagenesis");
    // The update holds this peer's operations only, not the snapshot it loaded.
    const meta = decodeImportBlobMeta(fromBase64(backend.sent[0]!.update), false);
    expect(meta.partialEndVersionVector.toJSON()).toEqual(new Map([["1000", 2]]));
    type(session, "c");
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS);
    expect(backend.sent).toHaveLength(2);
    expect(new Set(backend.sent.map((sent) => sent.requestId)).size).toBe(2);
    expect(session.state.outstanding).toBe(0);
    await session.dispose();
  });

  it("reports its unexported bytes to the size guard until it flushes", async () => {
    const backend = new FakeBackend();
    const session = await opened(backend);
    expect(session.unexportedBytes).toBe(0);
    type(session, noise(1000));
    expect(session.unexportedBytes).toBeGreaterThanOrEqual(1000);
    session.flush();
    expect(session.unexportedBytes).toBe(0);
    await vi.advanceTimersByTimeAsync(0);
    expect(backend.sent).toHaveLength(1);
    await session.dispose();
  });

  it("flushes at once when the export budget is reached", async () => {
    const backend = new FakeBackend();
    const session = await opened(backend);
    type(session, noise(EXPORT_BUDGET_BYTES + 16));
    await vi.advanceTimersByTimeAsync(0);
    expect(backend.sent).toHaveLength(1);
    expect(fromBase64(backend.sent[0]!.update).byteLength).toBeGreaterThanOrEqual(EXPORT_BUDGET_BYTES);
    await session.dispose();
  });

  it("retries an unknown outcome with the same request ID and bytes", async () => {
    const backend = new FakeBackend();
    backend.answers = ["unknown", "unknown"];
    const session = await opened(backend);
    type(session, "x");
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS);
    expect(backend.sent).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(RETRY_MS);
    await vi.advanceTimersByTimeAsync(RETRY_MS);
    expect(backend.sent).toHaveLength(3);
    expect(new Set(backend.sent.map((sent) => sent.requestId)).size).toBe(1);
    expect(new Set(backend.sent.map((sent) => sent.update)).size).toBe(1);
    expect(text(backend.body)).toBe("xgenesis");
    expect(session.state.outstanding).toBe(0);
    await session.dispose();
  });

  it("after a generation replacement recovers its outstanding IDs and resends only the absent", async () => {
    const backend = new FakeBackend();
    backend.answers = ["hold"];
    const session = await opened(backend);
    type(session, "one ");
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS);
    type(session, "two ", 4);
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS);
    expect(backend.sent).toHaveLength(1);
    expect(session.state.outstanding).toBe(2);
    const [first] = backend.sent;
    // The first request reached the record log before the old generation stopped.
    backend.replaceGeneration(1);
    await vi.advanceTimersByTimeAsync(0);
    expect(backend.opened.at(-1)).toEqual({ resumePeer: "1000", sessionPeer: "1000" });
    await vi.advanceTimersByTimeAsync(0);
    expect(backend.sent).toHaveLength(2);
    expect(backend.sent[1]!.requestId).not.toBe(first!.requestId);
    expect(text(backend.body)).toBe("one two genesis");
    expect(text(session.doc)).toBe("one two genesis");
    expect(session.state.outstanding).toBe(0);
    await session.dispose();
  });

  it("on resync discards its doc, reopens, and reports unsent changes as lost", async () => {
    const backend = new FakeBackend();
    const lost: LostNotice[] = [];
    const session = await opened(backend, lost);
    type(session, "kept ");
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS);
    type(session, "unsent ");
    backend.resync("1000");
    await vi.advanceTimersByTimeAsync(0);
    expect(lost).toEqual([{ requests: 0, unsentBytes: expect.any(Number) as number }]);
    expect(lost[0]!.unsentBytes).toBeGreaterThan(0);
    expect(session.state.sessionPeer).toBe("1001");
    expect(text(session.doc)).toBe("kept genesis");
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS);
    expect(backend.sent).toHaveLength(1);
    await session.dispose();
  });

  it("a refused update reloads and reports the refused request as lost", async () => {
    const backend = new FakeBackend();
    backend.answers = ["refuse"];
    const lost: LostNotice[] = [];
    const session = await opened(backend, lost);
    type(session, "bad ");
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS);
    await vi.advanceTimersByTimeAsync(0);
    expect(lost).toEqual([{ requests: 1, unsentBytes: 0 }]);
    expect(text(session.doc)).toBe("genesis");
    expect(session.state.sessionPeer).toBe("1001");
    await session.dispose();
  });

  it("keeps unacknowledged edits while the body waits for history delivered out of order", async () => {
    const backend = new FakeBackend();
    const lost: LostNotice[] = [];
    const session = await opened(backend, lost);
    const remote = backend.body.fork();
    remote.setPeerId(9n);
    const from = remote.oplogVersion();
    remote.getText("body").insert(7, " B");
    remote.commit();
    const afterB = remote.oplogVersion();
    const b = remote.export({ mode: "update", from });
    remote.getText("body").insert(9, " C");
    remote.commit();
    const c = remote.export({ mode: "update", from: afterB });

    type(session, "mine ");
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS - 10);
    // C arrives before B: the body is pending, and the first update is refused as pending.
    backend.deliverRemote(c);
    expect(session.state.editable).toBe(false);
    await vi.advanceTimersByTimeAsync(10);
    expect(backend.sent).toHaveLength(1);
    type(session, "more ", 5);
    await vi.advanceTimersByTimeAsync(RETRY_MS);
    expect(session.state.outstanding).toBe(2);
    // B arrives: the body is editable again, the session gains both, and its retries land.
    backend.deliverRemote(b);
    expect(session.state.editable).toBe(true);
    await vi.advanceTimersByTimeAsync(RETRY_MS);
    await vi.advanceTimersByTimeAsync(0);
    expect(lost).toEqual([]);
    expect(session.state.sessionPeer).toBe("1000");
    expect(session.state.outstanding).toBe(0);
    expect(text(backend.body)).toBe("mine more genesis B C");
    expect(text(session.doc)).toBe(text(backend.body));
    await session.dispose();
  });

  it("reconnects as the same peer when the Backend no longer knows its session, keeping its queue", async () => {
    const backend = new FakeBackend();
    backend.answers = ["lost_session"];
    const lost: LostNotice[] = [];
    const session = await opened(backend, lost);
    type(session, "kept ");
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS);
    await vi.advanceTimersByTimeAsync(0);
    // The ended stream reports it only after the refusal; it is no longer this session's.
    backend.terminateEnded();
    await vi.advanceTimersByTimeAsync(0);
    expect(lost).toEqual([]);
    expect(backend.opened.at(-1)).toEqual({ resumePeer: "1000", sessionPeer: "1000" });
    expect(text(backend.body)).toBe("kept genesis");
    expect(session.state.outstanding).toBe(0);
    expect(backend.opened).toHaveLength(2);
    await session.dispose();
  });

  it("an unknown session answered after its stream ended changes nothing more", async () => {
    const backend = new FakeBackend();
    backend.answers = ["hold"];
    const lost: LostNotice[] = [];
    const session = await opened(backend, lost);
    type(session, "kept ");
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS);
    backend.replaceGeneration(0);
    backend.failHeld("documents.unknown_session");
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(0);
    expect(lost).toEqual([]);
    expect(backend.opened).toHaveLength(2);
    expect(text(backend.body)).toBe("kept genesis");
    expect(session.state.outstanding).toBe(0);
    await session.dispose();
  });

  it("a reopen as the same peer merges the snapshot into its doc, keeps its queue and retries", async () => {
    const backend = new FakeBackend();
    backend.answers = ["unknown"];
    const lost: LostNotice[] = [];
    const session = await opened(backend, lost);
    const remote = backend.body.fork();
    remote.setPeerId(9n);
    const from = remote.oplogVersion();
    remote.getText("body").insert(7, " remote");
    remote.commit();
    type(session, "queued ");
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS);
    expect(session.state.outstanding).toBe(1);
    type(session, "unsent ", 7);
    // The Backend gains more than one update can carry and reopens the session instead of relaying it.
    backend.deliverUnrelayed(remote.export({ mode: "update", from }));
    const doc = session.doc;
    backend.reopen("1000", () => type(session, "during ", 14));
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS);
    await vi.advanceTimersByTimeAsync(RETRY_MS);
    expect(lost).toEqual([]);
    expect(session.doc).toBe(doc);
    expect(session.state.sessionPeer).toBe("1000");
    expect(backend.opened).toHaveLength(1);
    expect(session.state.outstanding).toBe(0);
    expect(text(backend.body)).toBe("queued unsent during genesis remote");
    expect(text(session.doc)).toBe(text(backend.body));
    await session.dispose();
  });

  it("a reopen whose snapshot already holds a queued request does not send it again", async () => {
    const backend = new FakeBackend();
    backend.answers = ["hold"];
    const lost: LostNotice[] = [];
    const session = await opened(backend, lost);
    type(session, "one ");
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS);
    type(session, "two ", 4);
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS);
    expect(backend.sent).toHaveLength(1);
    // The held request is appended, but its answer has not arrived when the session is reopened.
    backend.deliverUnrelayed(fromBase64(backend.sent[0]!.update));
    backend.appended.add(backend.sent[0]!.requestId);
    backend.reopen("1000");
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(0);
    expect(backend.sent).toHaveLength(2);
    expect(backend.sent[1]!.requestId).not.toBe(backend.sent[0]!.requestId);
    expect(lost).toEqual([]);
    expect(session.state.outstanding).toBe(0);
    expect(text(backend.body)).toBe("one two genesis");
    expect(text(session.doc)).toBe("one two genesis");
    await session.dispose();
  });

  it("keeps its edits through repeated reopens", async () => {
    const backend = new FakeBackend();
    const lost: LostNotice[] = [];
    const session = await opened(backend, lost);
    for (const word of ["a ", "b ", "c "]) {
      type(session, word);
      backend.reopen("1000");
      await vi.advanceTimersByTimeAsync(0);
    }
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS);
    expect(lost).toEqual([]);
    expect(text(backend.body)).toBe("c b a genesis");
    expect(session.state.outstanding).toBe(0);
    await session.dispose();
  });

  it("a session resumed onto a pending body keeps its edits when it is reopened with the body", async () => {
    const backend = new FakeBackend();
    backend.answers = ["hold"];
    const lost: LostNotice[] = [];
    const session = await opened(backend, lost);
    const remote = backend.body.fork();
    remote.setPeerId(9n);
    const from = remote.oplogVersion();
    remote.getText("body").insert(7, " B");
    remote.commit();
    const afterB = remote.oplogVersion();
    const b = remote.export({ mode: "update", from });
    remote.getText("body").insert(9, " C");
    remote.commit();
    type(session, "mine ");
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS);
    // The Backend restarts; the body waits for B, so the resumed session opens without a snapshot.
    backend.deliverRemote(remote.export({ mode: "update", from: afterB }));
    backend.replaceGeneration(0);
    await vi.advanceTimersByTimeAsync(0);
    expect(backend.opened.at(-1)).toEqual({ resumePeer: "1000", sessionPeer: "1000" });
    expect(session.state.editable).toBe(false);
    await vi.advanceTimersByTimeAsync(RETRY_MS);
    // B arrives: the Backend reopens the session with the body instead of resyncing it.
    backend.deliverUnrelayed(b);
    backend.pending = false;
    backend.reopen("1000");
    await vi.advanceTimersByTimeAsync(RETRY_MS);
    await vi.advanceTimersByTimeAsync(0);
    expect(lost).toEqual([]);
    expect(session.state.editable).toBe(true);
    expect(session.state.outstanding).toBe(0);
    expect(text(backend.body)).toBe("mine genesis B C");
    expect(text(session.doc)).toBe(text(backend.body));
    await session.dispose();
  });

  it("CR-202: a malformed opening reconnects as the same peer only after the old stream is closed", async () => {
    const backend = new FakeBackend();
    backend.answers = ["hold"];
    const lost: LostNotice[] = [];
    const session = await opened(backend, lost);
    type(session, "kept ");
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS);
    backend.malformed("1000");
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(0);
    expect(backend.opened.at(-1)).toEqual({ resumePeer: "1000", sessionPeer: "1000" });
    expect(lost).toEqual([]);
    expect(text(backend.body)).toBe("kept genesis");
    expect(session.state.outstanding).toBe(0);
    await session.dispose();
  });

  it("asks again to continue as its peer while the Backend still holds the old session", async () => {
    const backend = new FakeBackend();
    backend.answers = ["hold"];
    backend.releaseAfterMs = RETRY_MS / 2;
    const lost: LostNotice[] = [];
    const session = await opened(backend, lost);
    type(session, "kept ");
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS);
    backend.malformed("1000");
    await vi.advanceTimersByTimeAsync(0);
    // The old session is still registered: the Backend hands out another peer, which is closed.
    expect(backend.opened.at(-1)).toEqual({ resumePeer: "1000", sessionPeer: "1001" });
    await vi.advanceTimersByTimeAsync(RETRY_MS);
    await vi.advanceTimersByTimeAsync(0);
    expect(backend.opened.at(-1)).toEqual({ resumePeer: "1000", sessionPeer: "1000" });
    expect(lost).toEqual([]);
    expect(session.state.sessionPeer).toBe("1000");
    expect(text(backend.body)).toBe("kept genesis");
    await session.dispose();
  });

  it("gives up its doc and reports it after the Backend keeps handing out other peers", async () => {
    const backend = new FakeBackend();
    backend.answers = ["hold"];
    const lost: LostNotice[] = [];
    const session = await opened(backend, lost);
    type(session, "pending ");
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS);
    backend.refuseResume = true;
    backend.replaceGeneration(0);
    await vi.advanceTimersByTimeAsync(0);
    for (let attempt = 0; attempt < RESUME_ATTEMPTS; attempt += 1) await vi.advanceTimersByTimeAsync(RETRY_MS);
    await vi.advanceTimersByTimeAsync(0);
    expect(backend.opened.filter((open) => open.resumePeer === "1000")).toHaveLength(RESUME_ATTEMPTS + 1);
    expect(lost).toEqual([{ requests: 1, unsentBytes: 0 }]);
    expect(text(session.doc)).toBe("genesis");
    await session.dispose();
  });

  it("CR-201: a recovery answered during a reopen never unpauses sending or drops another request", async () => {
    const body = new LoroDoc();
    body.setPeerId(1n);
    body.getText("body").insert(0, "genesis");
    body.commit();
    const appended = new Set<string>();
    type Held<T> = { resolve: (value: T) => void; reject: (error: unknown) => void };
    const updates: Array<{ requestId: string; update: string } & Held<unknown>> = [];
    const recovers: Array<{ ids: readonly string[] } & Held<readonly RequestStatus[]>> = [];
    const sent: string[] = [];
    let listener: (event: SessionEvent) => void = () => {};
    let terminated: (error: unknown) => void = () => {};
    let pending = false;
    const opening = (): SessionEvent[] => {
      const chunks = pending ? [] : [body.export({ mode: "snapshot" })];
      return [
        { event: "begin", documentId: DOCUMENT, sessionPeer: "1000", version: "0", editable: !pending, chunks: chunks.length },
        ...chunks.map((chunk, chunkIndex): SessionEvent => ({ event: "chunk", documentId: DOCUMENT, chunkIndex, chunk: toBase64(chunk) })),
        { event: "end", documentId: DOCUMENT },
      ];
    };
    const bridge: SessionBridge = {
      open: async (_request, l, t) => {
        listener = l;
        terminated = t;
        const events = opening();
        queueMicrotask(() => events.forEach((event) => listener(event)));
        return { close: async () => {} };
      },
      update: (request) => new Promise((resolve, reject) => {
        sent.push(request.requestId);
        updates.push({ requestId: request.requestId, update: request.update, resolve, reject });
      }),
      recover: (_documentId, ids) => new Promise((resolve, reject) => { recovers.push({ ids, resolve, reject }); }),
    };
    const lost: LostNotice[] = [];
    const session = new EditorSession({ bridge, documentId: DOCUMENT, onLost: (notice) => lost.push(notice) });
    await session.start();
    await vi.advanceTimersByTimeAsync(0);
    // X is sent and its answer is held; Y queues behind it.
    type(session, "X");
    session.flush();
    type(session, "Y", 1);
    session.flush();
    expect(updates).toHaveLength(1);

    // The generation is replaced; the resumed session opens onto a pending body.
    pending = true;
    terminated(createPackageRequestError({ code: "backend_unavailable", retryable: true }));
    await vi.advanceTimersByTimeAsync(0);
    expect(recovers).toHaveLength(1);
    const ids = [...recovers[0]!.ids];
    recovers.shift()!.resolve(ids.map((requestId) => ({ requestId, inclusion: "unresolved", durability: "unresolved" })));
    await vi.advanceTimersByTimeAsync(RETRY_MS);
    expect(recovers).toHaveLength(1);

    // The body becomes editable and the session is reopened on its stream; only `begin` has arrived
    // when the retried recovery is answered.
    pending = false;
    const reopen = opening();
    listener(reopen[0]!);
    recovers.shift()!.resolve(ids.map((requestId) => ({ requestId, inclusion: "absent", durability: "unresolved" })));
    await vi.advanceTimersByTimeAsync(0);
    expect(sent).toHaveLength(1);
    reopen.slice(1).forEach((event) => listener(event));
    await vi.advanceTimersByTimeAsync(0);

    // The Backend answers everything from its own record from here on.
    for (let round = 0; round < 10; round += 1) {
      for (const recover of recovers.splice(0)) {
        recover.resolve(recover.ids.map((requestId): RequestStatus => appended.has(requestId)
          ? { requestId, inclusion: "included", durability: "durable" }
          : { requestId, inclusion: "absent", durability: "unresolved" }));
      }
      for (const update of updates.splice(0)) {
        if (!appended.has(update.requestId)) {
          const status = body.import(fromBase64(update.update));
          expect(status.pending ?? null).toBeNull();
          appended.add(update.requestId);
        }
        update.resolve({ requestId: update.requestId });
      }
      await vi.advanceTimersByTimeAsync(RETRY_MS);
    }
    type(session, "Z");
    session.flush();
    await vi.advanceTimersByTimeAsync(0);
    for (const update of updates.splice(0)) {
      expect(body.import(fromBase64(update.update)).pending ?? null).toBeNull();
      update.resolve({ requestId: update.requestId });
    }
    await vi.advanceTimersByTimeAsync(0);
    expect(lost).toEqual([]);
    expect(text(body)).toBe("ZXYgenesis");
    expect(text(session.doc)).toBe("ZXYgenesis");
    expect(session.state.outstanding).toBe(0);
    await session.dispose();
  });

  it("closing during a reopen still sends its queue once recovery decides it", async () => {
    const backend = new FakeBackend();
    backend.answers = ["hold"];
    const lost: LostNotice[] = [];
    const session = await opened(backend, lost);
    type(session, "last ");
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS);
    let closing: Promise<void> | null = null;
    backend.reopen("1000", () => { closing = session.dispose(); });
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(0);
    await closing;
    expect(lost).toEqual([]);
    expect(text(backend.body)).toBe("last genesis");
  });

  it("closing reports what the Backend never acknowledged", async () => {
    const backend = new FakeBackend();
    backend.pending = true;
    const lost: LostNotice[] = [];
    const session = await opened(backend, lost);
    type(session, "unconfirmed ");
    const closing = session.dispose();
    await vi.advanceTimersByTimeAsync(5 * RETRY_MS);
    await closing;
    expect(lost).toEqual([{ requests: 1, unsentBytes: 0 }]);
    expect(text(backend.body)).toBe("genesis");
  });

  it("two sessions converge through the relay", async () => {
    const backend = new FakeBackend();
    const left = await opened(backend);
    const right = await opened(backend);
    expect(left.state.sessionPeer).not.toBe(right.state.sessionPeer);
    type(left, "left ");
    type(right, " right", 7);
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS);
    type(left, "more ");
    await vi.advanceTimersByTimeAsync(SEND_IDLE_MS);
    expect(text(left.doc)).toBe(text(right.doc));
    expect(text(left.doc)).toBe(text(backend.body));
    expect(text(left.doc)).toContain("left");
    expect(text(left.doc)).toContain("right");
    expect(text(left.doc)).toContain("more");
    await left.dispose();
    await right.dispose();
  });

  it("closing sends what is left before the session ends", async () => {
    const backend = new FakeBackend();
    const session = await opened(backend);
    type(session, "last ");
    await session.dispose();
    expect(backend.sent).toHaveLength(1);
    expect(text(backend.body)).toBe("last genesis");
  });
});
