// The harness Backends: Installations of one Workspace relayed through an in-memory record log,
// with offline control. Each Installation serves the Documents UI contract to its views the way
// the Rust Backend does for the operations the editor uses: snapshot-fed sessions with a fresh
// peer each, validate-then-append of identified updates, relay to the other sessions, recovery,
// chunked Markdown, and Comments. Canonical Markdown and Markdown revisions come from the Rust
// Markdown core (`backend/examples/frame_scenarios.rs`), the only Markdown authority.
import { execFileSync } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { LoroDoc } from "loro-crdt";

/** One record carries at most this much update (the Backend's `MAX_UPDATE`). */
export const MAX_UPDATE = 160 * 1024;
/** Snapshot chunks are this many raw bytes: 24 KiB once base64-encoded. */
const SNAPSHOT_CHUNK = 18 * 1024;
/** Markdown chunks are at most this many UTF-8 bytes. */
const MARKDOWN_CHUNK = 24 * 1024;
/** A UI payload is at most this large once JSON-encoded (`MAX_UI_PAYLOAD_BYTES`). */
export const MAX_UI_PAYLOAD = 256 * 1024;

export const COLLECTION = "c".repeat(64);
export const DOCUMENT = "d".repeat(64);

const PACKAGE_ERROR = "teamarness.package-request-error";

function packageError(code, retryable = false) {
  return {
    kind: PACKAGE_ERROR, version: 2, namespace: "teamarness.package-host", code: "backend_error", stage: "backend",
    retryable, recovery: "none", diagnosticId: "frame-scenarios", packageError: { code, retryable, details: {} },
  };
}

/** The Rust Markdown core, run as a program. */
export class MarkdownAuthority {
  constructor(binary) {
    this.binary = binary;
    this.directory = fs.mkdtempSync(path.join(os.tmpdir(), "documents-frame-scenarios-"));
    this.sequence = 0;
  }

  genesis(peer) {
    return new Uint8Array(execFileSync(this.binary, ["genesis", String(peer)]));
  }

  /** The Canonical Markdown of `doc`; throws when the body is not a valid tree. */
  export(doc) {
    const file = this.file("body", doc.export({ mode: "snapshot" }));
    return execFileSync(this.binary, ["export", file], { encoding: "utf8", maxBuffer: 16 * 1024 * 1024 });
  }

  /** The update that merges `markdown` against the current version of `doc` as `peer`. */
  revise(doc, markdown, peer) {
    const body = this.file("body", doc.export({ mode: "snapshot" }));
    const revision = this.file("revision", Buffer.from(markdown, "utf8"));
    return new Uint8Array(execFileSync(this.binary, ["revise", body, revision, String(peer)], { maxBuffer: 16 * 1024 * 1024 }));
  }

  file(kind, bytes) {
    const file = path.join(this.directory, `${kind}-${++this.sequence}.bin`);
    fs.writeFileSync(file, bytes);
    return file;
  }

  dispose() {
    fs.rmSync(this.directory, { recursive: true, force: true });
  }
}

function freshPeer(taken) {
  for (;;) {
    const peer = randomBytes(8).readBigUInt64BE() >> 1n;
    if (peer !== 0n && !taken.has(peer)) {
      taken.add(peer);
      return peer;
    }
  }
}

function peersOf(doc) {
  return new Set([...doc.oplogVersion().toJSON().keys()].map((peer) => BigInt(peer)));
}

/** The peers whose operations `update` adds beyond `doc`. */
function addedPeers(doc, update) {
  const fork = doc.fork();
  const before = fork.oplogVersion().toJSON();
  fork.import(update);
  const added = [];
  for (const [peer, counter] of fork.oplogVersion().toJSON()) {
    if ((before.get(peer) ?? 0) < counter) added.push(BigInt(peer));
  }
  return added;
}

export class Workspace {
  constructor(authority) {
    this.authority = authority;
    /** The record log: `{author, bytes, requestId, published}` in append order. */
    this.records = [];
    this.installations = new Map();
    this.comments = [];
    this.allocated = new Set();
    /** Updates a Backend refused, by view; a scenario expects none unless it says so. */
    this.refusals = [];
    /** The largest UI payload sent, JSON-encoded. */
    this.largestPayload = 0;
    this.deliver = () => {};
    const genesis = authority.genesis(freshPeer(this.allocated));
    this.records.push({ author: null, bytes: genesis, requestId: null, published: true });
  }

  installation(id) {
    let installation = this.installations.get(id);
    if (!installation) {
      installation = new Installation(this, id);
      this.installations.set(id, installation);
      installation.sync();
    }
    return installation;
  }

  setOnline(id, online) {
    const installation = this.installation(id);
    installation.online = online;
    if (online) for (const record of this.records) if (record.author === id) record.published = true;
    this.sync();
  }

  sync() {
    for (const installation of this.installations.values()) installation.sync();
  }

  append(installation, bytes, requestId) {
    this.records.push({ author: installation.id, bytes, requestId, published: installation.online });
    this.sync();
  }

  send(view, message) {
    const size = Buffer.byteLength(JSON.stringify(message.event ?? message.value ?? {}));
    if (size > this.largestPayload) this.largestPayload = size;
    if (size > MAX_UI_PAYLOAD) throw new Error(`a UI payload of ${size} bytes exceeds ${MAX_UI_PAYLOAD}`);
    this.deliver(view, message);
  }
}

class Installation {
  constructor(workspace, id) {
    this.workspace = workspace;
    this.id = id;
    this.online = true;
    this.body = new LoroDoc();
    this.applied = new Set();
    /** Open sessions by peer: `{view, subscription}`. */
    this.sessions = new Map();
    this.subscriptions = new Map();
    this.appendedRequests = new Set();
    this.revision = 0;
  }

  /** Imports every record this Installation can now see, and relays each to its sessions. */
  sync() {
    this.workspace.records.forEach((record, index) => {
      if (this.applied.has(index)) return;
      const visible = record.author === this.id || (record.published && this.online);
      if (!visible) return;
      this.applied.add(index);
      this.adopt(record.bytes, null);
    });
  }

  adopt(bytes, origin) {
    const before = this.body.oplogVersion();
    this.body.import(bytes);
    const added = this.body.export({ mode: "update", from: before });
    this.revision += 1;
    for (const [peer, session] of this.sessions) {
      if (peer === origin) continue;
      this.event(session, { event: "update", documentId: DOCUMENT, update: Buffer.from(added).toString("base64") });
    }
  }

  event(session, event) {
    this.workspace.send(session.view, { kind: "event", id: session.subscription, event });
  }

  async handle(view, message) {
    const respond = (value) => ({ kind: "response", id: message.id, ok: true, value });
    const refuse = (error) => ({ kind: "response", id: message.id, ok: false, error });
    try {
      switch (message.kind) {
        case "request": return respond(await this.request(view, message.operation, message.payload));
        case "subscribe": return respond(this.subscribe(view, message.id, message.operation, message.payload));
        case "close": this.close(message.id); return null;
        default: return null;
      }
    } catch (error) {
      if (error?.kind === PACKAGE_ERROR) return refuse(error);
      throw error;
    }
  }

  subscribe(view, id, operation, payload) {
    if (operation === "documents.document.session") {
      const peer = payload.resumePeer !== undefined && !this.sessions.has(payload.resumePeer)
        && !peersOf(this.body).has(BigInt(payload.resumePeer))
        ? BigInt(payload.resumePeer) : freshPeer(this.workspace.allocated);
      for (const known of peersOf(this.body)) this.workspace.allocated.add(known);
      const session = { view, subscription: id, peer: String(peer) };
      this.sessions.set(String(peer), session);
      this.subscriptions.set(id, String(peer));
      const snapshot = this.body.export({ mode: "snapshot" });
      const chunks = [];
      for (let start = 0; start < snapshot.byteLength; start += SNAPSHOT_CHUNK) chunks.push(snapshot.subarray(start, start + SNAPSHOT_CHUNK));
      queueMicrotask(() => {
        this.event(session, { event: "begin", documentId: DOCUMENT, sessionPeer: String(peer), version: String(this.revision), editable: true, chunks: chunks.length });
        chunks.forEach((chunk, chunkIndex) => this.event(session, { event: "chunk", documentId: DOCUMENT, chunkIndex, chunk: Buffer.from(chunk).toString("base64") }));
        this.event(session, { event: "end", documentId: DOCUMENT });
      });
    }
    // `documents.changes` and `documents.names` stay quiet: the listing never changes here.
    return null;
  }

  close(id) {
    const peer = this.subscriptions.get(id);
    this.subscriptions.delete(id);
    if (peer !== undefined) this.sessions.delete(peer);
  }

  async request(view, operation, payload) {
    switch (operation) {
      case "documents.snapshot":
        return {
          availability: this.online ? "current" : "offline", warnings: [],
          collections: [{ id: COLLECTION, name: "Frame scenarios", archived: false }],
          documents: [{ id: DOCUMENT, collectionId: COLLECTION, title: "Scenario document", archived: false }],
        };
      case "documents.document.open":
        return { documentId: DOCUMENT, editable: true };
      case "documents.document.update":
        return this.update(view, payload);
      case "documents.document.recover":
        return {
          documentId: DOCUMENT,
          requestStatuses: payload.requestIds.map((requestId) => this.appendedRequests.has(requestId)
            ? { requestId, inclusion: "included", durability: "durable" }
            : { requestId, inclusion: "absent", durability: "unresolved" }),
        };
      case "documents.document.markdown":
        return this.markdownChunk(payload.chunkIndex);
      case "documents.comments":
        return { comments: this.workspace.comments.map((comment) => ({ ...comment, isLocal: comment.author === this.id })) };
      case "documents.comment.add": {
        const id = randomBytes(32).toString("hex");
        this.workspace.comments.push({
          id, documentId: DOCUMENT, author: this.id, position: String(this.workspace.comments.length + 1).padStart(4, "0"),
          resolved: false, markdown: payload.markdown, ...(payload.quote !== undefined ? { quote: payload.quote } : {}),
          ...(payload.parentId !== undefined ? { parentId: payload.parentId } : {}), createdAt: String(Date.now()),
        });
        return { ok: true, id };
      }
      default:
        throw packageError("documents.unsupported_in_frame_scenarios");
    }
  }

  /** Validates an identified update as the Rust Backend does before it appends one record. */
  update(view, { sessionPeer, requestId, update }) {
    const session = this.sessions.get(sessionPeer);
    if (session === undefined) throw packageError("documents.unknown_session");
    if (this.appendedRequests.has(requestId)) return { requestId };
    const bytes = new Uint8Array(Buffer.from(update, "base64"));
    const refuse = (code) => {
      this.workspace.refusals.push({ view, code, bytes: bytes.byteLength });
      this.sessions.delete(sessionPeer);
      queueMicrotask(() => this.event(session, { event: "resync", documentId: DOCUMENT }));
      throw packageError(code);
    };
    if (bytes.byteLength > MAX_UPDATE) refuse("documents.edit_request_too_large");
    let peers;
    try {
      peers = addedPeers(this.body, bytes);
    } catch {
      refuse("documents.invalid_update");
    }
    if (peers.length === 0 || peers.some((peer) => String(peer) !== sessionPeer)) refuse("documents.foreign_peer");
    this.appendedRequests.add(requestId);
    this.adopt(bytes, sessionPeer);
    this.applied.add(this.workspace.records.length);
    this.workspace.append(this, bytes, requestId);
    queueMicrotask(() => this.event(session, { event: "status", documentId: DOCUMENT, durable: [requestId], editable: true }));
    return { requestId };
  }

  markdownChunk(chunkIndex) {
    const version = JSON.stringify([...this.body.oplogVersion().toJSON()]);
    if (this.markdownCache?.version !== version) {
      const text = this.workspace.authority.export(this.body);
      const chunks = [];
      let current = "";
      let bytes = 0;
      for (const character of text) {
        const size = Buffer.byteLength(character);
        if (bytes + size > MARKDOWN_CHUNK) { chunks.push(current); current = ""; bytes = 0; }
        current += character;
        bytes += size;
      }
      chunks.push(current);
      this.markdownCache = { version, chunks, digest: createHash("sha256").update(text, "utf8").digest("hex") };
    }
    const { chunks, digest } = this.markdownCache;
    if (chunkIndex >= chunks.length) throw packageError("documents.invalid_chunk");
    return { documentId: DOCUMENT, digest, chunks: chunks.length, chunkIndex, chunk: chunks[chunkIndex] };
  }

  /** A port-style `document.revise`: one merge from a fresh peer, appended as one record. */
  revise(markdown) {
    const update = this.workspace.authority.revise(this.body, markdown, freshPeer(this.workspace.allocated));
    if (update.byteLength > MAX_UPDATE) throw new Error("revision_too_large");
    this.adopt(update, null);
    this.applied.add(this.workspace.records.length);
    this.workspace.append(this, update, null);
  }

  markdown() {
    return this.workspace.authority.export(this.body);
  }
}
