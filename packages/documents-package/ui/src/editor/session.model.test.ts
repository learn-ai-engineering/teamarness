// Verifies: TEAM.SYS.DOCS-R05 — a doc-holding Editor Session never drops a local change silently.
//
// A model-based test of `session.ts`. A fake Backend that follows the real Backend's session rules
// (`backend.rs` `open_session`, `register_session`, `update_from_session`, recovery) is driven by a
// seeded scheduler that interleaves stream events, request answers, request processing, stream
// releases, local edits, remote records (in and out of causal order), lags, generation
// replacements, refusals and timer advances in random orders. After the run heals, every local
// edit must be durable on the Backend or have been reported through `onLost`, and the session and
// the Backend must converge. `SEEDS` sets the number of seeds; `SEED` runs one seed.
import { createPackageRequestError } from "@teamarness/package-ui-sdk/testing";
import { LoroDoc } from "loro-crdt";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RequestStatus, SessionEvent } from "../contract.generated";
import { EditorSession, RETRY_MS, fromBase64, toBase64 } from "./session";
import type { LostNotice, SessionBridge } from "./session";

const DOCUMENT = "d".repeat(64);
const CHUNK = 64;
const SEEDS = Number(process.env.SEEDS ?? 1000);
const ONLY = process.env.SEED === undefined ? null : Number(process.env.SEED);
const STEPS = 120;

type Random = () => number;

function random(seed: number): Random {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let value = state;
    value = Math.imul(value ^ (value >>> 15), value | 1);
    value ^= value + Math.imul(value ^ (value >>> 7), value | 61);
    return ((value ^ (value >>> 14)) >>> 0) / 4294967296;
  };
}

type Delivery = { kind: "event"; event: SessionEvent } | { kind: "terminated" };

/** One subscription stream, as the port client delivers it: its events in order. */
type Stream = {
  readonly id: number;
  readonly listener: (event: SessionEvent) => void;
  readonly terminated: (error: unknown) => void;
  readonly outbox: Delivery[];
  /** The UI closed it: nothing more is delivered. */
  uiClosed: boolean;
  /** The Backend's session task has ended (its peer is released). */
  released: boolean;
  session: BackendSession | null;
};

/** The state `open_session` keeps for one registered session. */
type BackendSession = {
  readonly peer: string;
  readonly stream: Stream;
  readonly generation: number;
  editable: boolean;
  holdsDoc: boolean;
  live: boolean;
  /** It missed updates that only a fresh opening with the body can bring. */
  behind: boolean;
};

type Pending = {
  readonly generation: number;
  process: () => void;
  processed: boolean;
};

/** A request answer waiting to be delivered to the UI. */
type Answer = { settle: () => void; fail: () => void };

const unknown = () => createPackageRequestError({ code: "backend_unavailable", retryable: true });
const refusal = (code: string, retryable: boolean) => createPackageRequestError({ packageError: { code, retryable } });

class ModelBackend {
  readonly body = new LoroDoc();
  readonly appended = new Set<string>();
  generation = 0;
  private nextPeer = 1000;
  private nextStream = 1;
  readonly streams: Stream[] = [];
  readonly registered = new Map<string, BackendSession>();
  /** Peers this generation handed out (`claim_peer`), and those whose session ended. */
  private handed = new Set<string>();
  private ended = new Set<string>();
  readonly requests: Pending[] = [];
  readonly answers: Answer[] = [];
  readonly closes: Array<() => void> = [];
  /** Remote records that wait for their causal history. */
  private readonly waiting: Uint8Array[] = [];
  refuseNext = false;
  private nextRemote = 9_000;

  constructor() {
    this.body.setPeerId(1n);
    this.body.getText("body").insert(0, "genesis");
    this.body.commit();
  }

  get editable(): boolean {
    return this.waiting.length === 0;
  }

  bridge(): SessionBridge {
    return {
      open: (request, listener, terminated) => new Promise((resolve, reject) => {
        const generation = this.generation;
        this.requests.push({
          generation, processed: false,
          process: () => {
            if (generation !== this.generation) {
              this.answers.push({ settle: () => reject(unknown()), fail: () => reject(unknown()) });
              return;
            }
            const stream = this.register(request.resumePeer, listener, terminated);
            const close = async () => {
              stream.uiClosed = true;
              await new Promise<void>((done) => this.closes.push(done));
            };
            this.answers.push({ settle: () => resolve({ close }), fail: () => reject(unknown()) });
          },
        });
      }),
      update: (request) => new Promise((resolve, reject) => {
        const generation = this.generation;
        this.requests.push({
          generation, processed: false,
          process: () => {
            if (generation !== this.generation) {
              this.answers.push({ settle: () => reject(unknown()), fail: () => reject(unknown()) });
              return;
            }
            const outcome = this.update(request.sessionPeer, request.requestId, fromBase64(request.update));
            this.answers.push({
              settle: () => outcome === null ? resolve({ requestId: request.requestId }) : reject(outcome),
              fail: () => reject(unknown()),
            });
          },
        });
      }),
      recover: (_documentId, requestIds) => new Promise((resolve, reject) => {
        const generation = this.generation;
        this.requests.push({
          generation, processed: false,
          process: () => {
            const statuses = requestIds.map((requestId): RequestStatus => (
              !this.editable ? { requestId, inclusion: "unresolved", durability: "unresolved" }
                : this.appended.has(requestId) ? { requestId, inclusion: "included", durability: "durable" }
                  : { requestId, inclusion: "absent", durability: "unresolved" }));
            const failed = generation !== this.generation;
            this.answers.push({ settle: () => failed ? reject(unknown()) : resolve(statuses), fail: () => reject(unknown()) });
          },
        });
      }),
    };
  }

  /** `register_session` then the opening, as `open_session` sends it. */
  private register(resume: string | undefined, listener: Stream["listener"], terminated: Stream["terminated"]): Stream {
    const stream: Stream = { id: this.nextStream++, listener, terminated, outbox: [], uiClosed: false, released: false, session: null };
    this.streams.push(stream);
    // A resume is refused while the peer is still registered; then a fresh peer is handed out.
    const resumable = resume !== undefined && !this.registered.has(resume) && (this.ended.has(resume) || !this.handed.has(resume));
    const peer = resumable ? resume : String(this.nextPeer++);
    this.handed.add(peer);
    this.ended.delete(peer);
    const editable = this.editable;
    const session: BackendSession = {
      peer, stream, generation: this.generation, editable, holdsDoc: editable || resumable, live: editable, behind: false,
    };
    stream.session = session;
    this.registered.set(peer, session);
    this.sendOpening(session);
    return stream;
  }

  private sendOpening(session: BackendSession): void {
    const snapshot = this.editable ? this.body.export({ mode: "snapshot" }) : new Uint8Array();
    const chunks: Uint8Array[] = [];
    for (let start = 0; start < snapshot.byteLength; start += CHUNK) chunks.push(snapshot.subarray(start, start + CHUNK));
    const out = session.stream.outbox;
    out.push({ kind: "event", event: { event: "begin", documentId: DOCUMENT, sessionPeer: session.peer, version: "0", editable: this.editable, chunks: chunks.length } });
    chunks.forEach((chunk, chunkIndex) => out.push({ kind: "event", event: { event: "chunk", documentId: DOCUMENT, chunkIndex, chunk: toBase64(chunk) } }));
    out.push({ kind: "event", event: { event: "end", documentId: DOCUMENT } });
  }

  /** The session task sends `resync` and ends: its peer is released. */
  private resync(session: BackendSession): void {
    session.stream.outbox.push({ kind: "event", event: { event: "resync", documentId: DOCUMENT } });
    this.end(session);
  }

  private end(session: BackendSession): void {
    if (this.registered.get(session.peer) === session) {
      this.registered.delete(session.peer);
      this.ended.add(session.peer);
    }
    session.stream.session = null;
  }

  /** `update_from_session`: validate, append, relay, report durable. `null` when appended. */
  private update(peer: string, requestId: string, bytes: Uint8Array): unknown {
    const session = this.registered.get(peer);
    if (session === undefined) return refusal("documents.unknown_session", false);
    if (this.appended.has(requestId)) return null;
    if (!this.editable) return refusal("documents.document_pending", true);
    const probe = this.body.fork();
    const status = probe.import(bytes);
    if (this.refuseNext || (status.pending !== undefined && status.pending !== null)) {
      // A refusal retrying cannot cure ends the session with `resync`.
      this.refuseNext = false;
      this.resync(session);
      return refusal("documents.invalid_update", false);
    }
    const before = this.body.oplogVersion();
    this.body.import(bytes);
    this.appended.add(requestId);
    this.relay(this.body.export({ mode: "update", from: before }), peer);
    this.status([requestId]);
    return null;
  }

  private relay(update: Uint8Array, origin: string | null): void {
    for (const session of this.registered.values()) {
      if (session.peer === origin || !(session.editable || session.live)) continue;
      session.stream.outbox.push({ kind: "event", event: { event: "update", documentId: DOCUMENT, update: toBase64(update) } });
    }
  }

  /** A `BodyEvent::Status`, as each session task handles it. */
  private status(durable: string[]): void {
    for (const session of [...this.registered.values()]) {
      const now = this.editable;
      if (now !== session.editable && now && !session.holdsDoc) {
        this.resync(session);
        continue;
      }
      session.editable = now;
      if (session.editable && (!session.live || session.behind)) this.reopen(session);
      session.stream.outbox.push({ kind: "event", event: { event: "status", documentId: DOCUMENT, durable, editable: session.editable } });
    }
  }

  private reopen(session: BackendSession): void {
    this.sendOpening(session);
    session.editable = this.editable;
    session.live = session.live || this.editable;
    session.behind = !this.editable;
  }

  /** The session missed updates (a catch-up too large to relay, or a lagging receiver). */
  lag(session: BackendSession): void {
    if (!session.holdsDoc) this.resync(session);
    else this.reopen(session);
  }

  /** A remote Installation's two records; the second may arrive before the first. */
  remote(text: string, outOfOrder: boolean): void {
    const remote = new LoroDoc();
    remote.import(this.body.export({ mode: "snapshot" }));
    for (const waiting of this.waiting) remote.import(waiting);
    remote.setPeerId(BigInt(this.nextRemote++));
    const from = remote.oplogVersion();
    remote.getText("body").insert(0, `${text}a `);
    remote.commit();
    const middle = remote.oplogVersion();
    remote.getText("body").insert(0, `${text}b `);
    remote.commit();
    const first = remote.export({ mode: "update", from });
    const second = remote.export({ mode: "update", from: middle });
    const wasEditable = this.editable;
    const before = this.body.oplogVersion();
    if (outOfOrder) {
      // The second waits in the body for the first, which is still on its way.
      this.body.import(second);
      this.waiting.push(first);
    } else {
      this.body.import(first);
      this.body.import(second);
    }
    this.adopted(before, wasEditable);
  }

  /** Every record still on its way arrives. */
  catchUp(): void {
    if (this.waiting.length === 0) return;
    const before = this.body.oplogVersion();
    for (const record of this.waiting.splice(0)) this.body.import(record);
    this.adopted(before, false);
  }

  /** `adopt`: the body's gain is relayed, and a change of editability is a status. */
  private adopted(before: ReturnType<LoroDoc["oplogVersion"]>, wasEditable: boolean): void {
    if (this.body.oplogVersion().compare(before) !== 0) this.relay(this.body.export({ mode: "update", from: before }), null);
    if (wasEditable !== this.editable) this.status([]);
  }

  /** The Backend restarts: every stream ends, and the new generation knows no session. */
  replaceGeneration(next: Random): void {
    this.generation += 1;
    for (const stream of this.streams) {
      if (!stream.released) stream.outbox.push({ kind: "terminated" });
      stream.session = null;
    }
    this.registered.clear();
    this.handed = new Set();
    this.ended = new Set();
    for (const answer of this.answers) if (next() < 0.5) answer.settle = answer.fail;
  }

  /** The Backend's task for a stream the UI closed notices it and ends, releasing the peer. */
  release(stream: Stream): void {
    stream.released = true;
    if (stream.session !== null) this.end(stream.session);
  }

  /** Whether anything is still on its way. */
  get busy(): boolean {
    return this.requests.some((request) => !request.processed) || this.answers.length > 0 || this.closes.length > 0
      || this.streams.some((stream) => (stream.outbox.length > 0 && !stream.uiClosed) || (stream.uiClosed && !stream.released));
  }
}

function take<T>(items: T[], next: Random): T | undefined {
  if (items.length === 0) return undefined;
  return items.splice(Math.floor(next() * items.length), 1)[0];
}

type Discard = { doc: LoroDoc | null; words: string[]; notice: LostNotice };

async function run(seed: number): Promise<string[]> {
  const next = random(seed);
  const backend = new ModelBackend();
  const trace: string[] = [];
  const discards: Discard[] = [];
  /** The words typed into each doc the session held. */
  const typed = new Map<LoroDoc, string[]>();
  let session: EditorSession | null = null;
  session = new EditorSession({
    bridge: backend.bridge(),
    documentId: DOCUMENT,
    onLost: (notice) => {
      const doc = session?.doc ?? null;
      discards.push({ doc, words: doc === null ? [] : [...(typed.get(doc) ?? [])], notice });
    },
  });
  void session.start();
  let disposing: Promise<void> | null = null;
  let word = 0;
  const settle = () => vi.advanceTimersByTimeAsync(0);

  const deliverStream = (stream: Stream) => {
    const delivery = stream.outbox.shift();
    if (delivery === undefined || stream.uiClosed) return;
    if (delivery.kind === "terminated") {
      stream.outbox.length = 0;
      stream.released = true;
      stream.terminated(unknown());
    } else stream.listener(delivery.event);
  };

  for (let index = 0; index < STEPS; index += 1) {
    const roll = next();
    const open = backend.streams.filter((stream) => stream.outbox.length > 0 && !stream.uiClosed);
    if (roll < 0.25 && open.length > 0) {
      const stream = open[Math.floor(next() * open.length)]!;
      trace.push(`deliver#${stream.id}:${stream.outbox[0]!.kind === "event" ? (stream.outbox[0] as { event: SessionEvent }).event.event : "terminated"}`);
      deliverStream(stream);
    } else if (roll < 0.4) {
      const request = take(backend.requests.filter((each) => !each.processed), next);
      if (request !== undefined) {
        request.processed = true;
        trace.push("process");
        request.process();
      }
    } else if (roll < 0.52) {
      const answer = take(backend.answers, next);
      if (answer !== undefined) {
        trace.push("answer");
        answer.settle();
      }
    } else if (roll < 0.57) {
      const closing = take(backend.closes, next);
      if (closing !== undefined) {
        trace.push("closed");
        closing();
      }
    } else if (roll < 0.62) {
      const stream = backend.streams.find((each) => each.uiClosed && !each.released);
      if (stream !== undefined) {
        trace.push(`release#${stream.id}`);
        backend.release(stream);
      }
    } else if (roll < 0.8) {
      const doc = session.doc;
      if (doc !== null && session.state.editable && session.state.phase === "ready" && disposing === null) {
        const text = `w${word++}q`;
        typed.set(doc, [...(typed.get(doc) ?? []), text]);
        doc.getText("body").insert(0, `${text} `);
        doc.commit();
        session.flush();
        trace.push(`type:${text}`);
      }
    } else if (roll < 0.85) {
      const outOfOrder = next() < 0.5;
      trace.push(outOfOrder ? "remote-out-of-order" : "remote");
      backend.remote(`r${index}`, outOfOrder);
    } else if (roll < 0.88) {
      backend.catchUp();
      trace.push("catch-up");
    } else if (roll < 0.91) {
      const sessions = [...backend.registered.values()];
      const lagging = sessions[Math.floor(next() * sessions.length)];
      if (lagging !== undefined) {
        trace.push(`lag:${lagging.peer}`);
        backend.lag(lagging);
      }
    } else if (roll < 0.925) {
      trace.push("replace-generation");
      backend.replaceGeneration(next);
    } else if (roll < 0.93) {
      trace.push("refuse-next");
      backend.refuseNext = true;
    } else if (roll < 0.932 && disposing === null && index > STEPS / 2) {
      trace.push("dispose");
      disposing = session.dispose();
    } else {
      const ms = Math.floor(next() * RETRY_MS * 1.5);
      trace.push(`wait:${ms}`);
      await vi.advanceTimersByTimeAsync(ms);
    }
    await settle();
  }

  // Heal: the body gets its history, nothing more fails, and everything on its way arrives.
  backend.refuseNext = false;
  backend.catchUp();
  for (let round = 0; round < 400; round += 1) {
    for (const request of backend.requests) {
      if (!request.processed) {
        request.processed = true;
        request.process();
      }
    }
    await settle();
    for (const answer of backend.answers.splice(0)) answer.settle();
    await settle();
    for (const stream of backend.streams) while (stream.outbox.length > 0 && !stream.uiClosed) deliverStream(stream);
    await settle();
    for (const closing of backend.closes.splice(0)) closing();
    for (const stream of backend.streams) if (stream.uiClosed && !stream.released) backend.release(stream);
    await settle();
    const quiet = !backend.busy && (disposing !== null || (session.state.outstanding === 0 && session.state.phase === "ready" && session.state.editable));
    if (quiet && round > 3) break;
    await vi.advanceTimersByTimeAsync(RETRY_MS);
  }
  /** Lets `closing` finish: time passes and the Host acknowledges every close. */
  const finish = async (closing: Promise<void>): Promise<boolean> => {
    let done = false;
    void closing.then(() => { done = true; });
    for (let wait = 0; wait < 20 && !done; wait += 1) {
      for (const close of backend.closes.splice(0)) close();
      await vi.advanceTimersByTimeAsync(RETRY_MS);
    }
    return done;
  };
  if (disposing !== null && !(await finish(disposing))) return [`seed ${seed}: dispose never finished`, `trace: ${trace.join(", ")}`];

  const durable = backend.body.getText("body").toString();
  const failures: string[] = [];
  for (const { words, notice } of discards) {
    const lost = words.filter((each) => !durable.includes(each));
    if (lost.length > notice.requests + (notice.unsentBytes > 0 ? words.length : 0)) {
      failures.push(`seed ${seed}: ${lost.length} words lost (${lost.join(" ")}) but ${notice.requests} requests reported`);
    }
  }
  // Every doc the session held and did not report: each of its edits is durable.
  for (const [doc, words] of typed) {
    if (discards.some((discard) => discard.doc === doc)) continue;
    const missing = words.filter((each) => !durable.includes(each));
    if (missing.length > 0) failures.push(`seed ${seed}: ${missing.join(" ")} never became durable and was never reported`);
  }
  const current = session.doc;
  if (disposing === null) {
    if (current === null) failures.push(`seed ${seed}: the session holds no doc after healing`);
    else {
      if (current.getText("body").toString() !== durable) failures.push(`seed ${seed}: the session and the Backend did not converge`);
      if (session.state.outstanding !== 0) failures.push(`seed ${seed}: ${session.state.outstanding} requests still outstanding`);
    }
    if (!(await finish(session.dispose()))) failures.push(`seed ${seed}: dispose never finished`);
  }
  if (failures.length > 0) failures.push(`trace: ${trace.join(", ")}`);
  return failures;
}

describe("EditorSession under seeded interleavings", () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  it(`every local edit becomes durable or is reported lost (${ONLY ?? SEEDS} seeds)`, async () => {
    const seeds = ONLY === null ? Array.from({ length: SEEDS }, (_, index) => index + 1) : [ONLY];
    const failures: string[] = [];
    for (const seed of seeds) {
      const found = await run(seed);
      failures.push(...found);
      if (failures.length > 0 && ONLY === null) break;
    }
    expect(failures).toEqual([]);
  }, 600_000);
});
