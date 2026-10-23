#!/usr/bin/env node

import { pathToFileURL } from "node:url";

export const MAX_FRAME_BYTES = 1_048_576;
export const MAX_DECODED_FRAME_BYTES = 1_048_576;
export const MAX_IN_FLIGHT_EACH_DIRECTION = 32;
export const MAX_WRITER_FRAMES = 64;
export const MAX_WRITER_BYTES = 8_388_608;
export const INITIAL_STREAM_CREDIT = 32;
const immutableCapability = `teamarness.synchro${"nization"}.immutable-record-log`;
const KNOWN_KINDS = new Set([
  "initialize", "initialized", "request", "response", "error", "cancel", "stream_open",
  "event", "credit", "stream_close", "ping", "pong", "shutdown", "shutdown_ack",
]);

export class ProtocolFailure extends Error {
  constructor(code) {
    super(code);
    this.code = code;
  }
}

export function parsePayload(payload) {
  let text;
  try {
    text = new TextDecoder("utf-8", { fatal: true }).decode(payload);
  } catch {
    throw new ProtocolFailure("invalid_utf8");
  }
  try {
    rejectDuplicateKeys(text);
    const frame = JSON.parse(text);
    if (Buffer.byteLength(JSON.stringify(frame)) > MAX_DECODED_FRAME_BYTES) {
      throw new ProtocolFailure("frame_too_large");
    }
    if (frame === null || Array.isArray(frame) || typeof frame !== "object") {
      throw new ProtocolFailure("malformed_frame");
    }
    if (!KNOWN_KINDS.has(frame.kind)) throw new ProtocolFailure("unknown_kind");
    validateFrame(frame);
    return frame;
  } catch (error) {
    if (error instanceof ProtocolFailure) throw error;
    throw new ProtocolFailure("malformed_json");
  }
}

function validateFrame(frame) {
  const allowed = {
    initialize: ["kind", "nonce", "package", "uiContractDigest", "processGeneration", "workspace", "installation", "dataDirectory", "protocol", "capabilities", "limits"],
    initialized: ["kind", "nonce", "package", "uiContractDigest", "processGeneration", "protocol"],
    request: ["kind", "id", "operation", "grant", "mountId", "deadlineUnixMs", "payload"],
    response: ["kind", "id", "payload"], error: ["kind", "id", "code", "retryable", "outcomeUncertain", "details"],
    cancel: ["kind", "id", "targetId"], stream_open: ["kind", "id", "topic", "mountId", "initialCredit", "payload"],
    event: ["kind", "streamId", "sequence", "payload"], credit: ["kind", "streamId", "amount"],
    stream_close: ["kind", "streamId", "code"], ping: ["kind", "nonce"], pong: ["kind", "nonce"],
    shutdown: ["kind", "reason"], shutdown_ack: ["kind"],
  }[frame.kind];
  if (Object.keys(frame).some((key) => !allowed.includes(key))) throw new ProtocolFailure("malformed_frame");
  const object = (value) => value !== null && !Array.isArray(value) && typeof value === "object";
  const requiredString = (name) => {
    if (typeof frame[name] !== "string" || frame[name].length === 0) throw new ProtocolFailure("malformed_frame");
    return frame[name];
  };
  const base64url = (value) => {
    if (typeof value !== "string" || !/^[A-Za-z0-9_-]+$/.test(value)) throw new ProtocolFailure("invalid_base64url");
  };
  const id = (value) => {
    if (typeof value !== "string" || !/^(?:h|p):[A-Za-z0-9._~-]{1,126}$/.test(value)) throw new ProtocolFailure("invalid_id_namespace");
  };
  const payload = () => { if (!object(frame.payload)) throw new ProtocolFailure("malformed_frame"); };
  if (frame.kind === "initialize" || frame.kind === "initialized") {
    base64url(frame.nonce); decimal(frame.processGeneration);
    if (!object(frame.package) || !object(frame.protocol)) throw new ProtocolFailure("malformed_frame");
    if (frame.kind === "initialize" && (!object(frame.workspace) || !object(frame.installation) || !object(frame.dataDirectory) || !Array.isArray(frame.capabilities) || !object(frame.limits))) throw new ProtocolFailure("malformed_frame");
  } else if (frame.kind === "request") {
    id(frame.id); requiredString("operation"); decimal(frame.deadlineUnixMs); payload();
    if (frame.grant !== undefined) base64url(frame.grant);
    if (frame.mountId !== undefined) base64url(frame.mountId);
  } else if (frame.kind === "response") { id(frame.id); payload();
  } else if (frame.kind === "error") {
    id(frame.id); requiredString("code"); if (typeof frame.retryable !== "boolean") throw new ProtocolFailure("malformed_frame");
  } else if (frame.kind === "cancel") { id(frame.id); id(frame.targetId);
  } else if (frame.kind === "stream_open") {
    id(frame.id); requiredString("topic"); payload(); if (!Number.isInteger(frame.initialCredit)) throw new ProtocolFailure("malformed_frame");
  } else if (frame.kind === "event") { id(frame.streamId); decimal(frame.sequence); payload();
  } else if (frame.kind === "credit") { id(frame.streamId); if (!Number.isInteger(frame.amount)) throw new ProtocolFailure("malformed_frame");
  } else if (frame.kind === "stream_close") { id(frame.streamId);
  } else if (frame.kind === "ping" || frame.kind === "pong") base64url(frame.nonce);
  else if (frame.kind === "shutdown") requiredString("reason");
}

export function decodeFrame(bytes) {
  if (bytes.length < 4) throw new ProtocolFailure("truncated_frame");
  const length = bytes.readUInt32BE(0);
  if (length === 0) throw new ProtocolFailure("zero_length");
  if (length > MAX_FRAME_BYTES) throw new ProtocolFailure("frame_too_large");
  if (bytes.length !== length + 4) throw new ProtocolFailure("truncated_frame");
  return parsePayload(bytes.subarray(4));
}

export function encodeFrame(frame) {
  const payload = Buffer.from(JSON.stringify(frame));
  if (payload.length === 0 || payload.length > MAX_FRAME_BYTES) {
    throw new ProtocolFailure("frame_too_large");
  }
  const prefix = Buffer.alloc(4);
  prefix.writeUInt32BE(payload.length);
  return Buffer.concat([prefix, payload]);
}

/** Tombstone entries kept per originating direction, bounded by two contract windows. */
export const TOMBSTONE_CAPACITY = 2 * MAX_IN_FLIGHT_EACH_DIRECTION;

/** The frame is in contract and names live state. */
export const DELIVER = "deliver";
/** The frame is in contract but names state this side has already retired. */
export const IGNORE = "ignore";

/** A bounded FIFO set of IDs, one queue per originating namespace. */
class Tombstones {
  #queues = { "h:": [], "p:": [] };
  #sets = { "h:": new Set(), "p:": new Set() };

  #side(id) {
    return id.startsWith("h:") ? "h:" : "p:";
  }

  add(id) {
    const side = this.#side(id);
    if (this.#sets[side].has(id)) return;
    this.#sets[side].add(id);
    this.#queues[side].push(id);
    while (this.#queues[side].length > TOMBSTONE_CAPACITY) {
      this.#sets[side].delete(this.#queues[side].shift());
    }
  }

  has(id) {
    return this.#sets[this.#side(id)].has(id);
  }

  delete(id) {
    const side = this.#side(id);
    if (!this.#sets[side].delete(id)) return false;
    this.#queues[side] = this.#queues[side].filter((value) => value !== id);
    return true;
  }
}

/**
 * The one classifier both peers run over every frame, in both directions.
 *
 * `observe` returns `DELIVER` for a frame that names live state and `IGNORE`
 * for one a conforming peer sent before it could see this side retire the
 * state it names. Everything else throws and is fatal to the generation.
 */
export class ContractState {
  #pending = new Map();
  #answered = new Tombstones();
  #abandoned = new Tombstones();
  #closedStreams = new Tombstones();
  #streams = new Map();

  /** Retires a request this side has stopped waiting for. */
  retireRequest(id) {
    if (this.#answered.has(id)) return;
    this.#pending.delete(id);
    this.#abandoned.add(id);
  }

  /** Retires a stream this side has stopped serving. */
  retireStream(id) {
    const stream = this.#streams.get(id);
    if (stream) stream.retired = true;
  }

  observe(direction, frame) {
    validateDirection(direction, frame);
    if ((frame.kind === "initialize" || frame.kind === "initialized") && frame.protocol?.major !== 1) {
      throw new ProtocolFailure("incompatible_version");
    }
    if (frame.kind === "request") {
      if (this.#pending.has(frame.id) || this.#answered.has(frame.id) || this.#abandoned.has(frame.id)) {
        throw new ProtocolFailure("duplicate_id");
      }
      let inFlight = 0;
      for (const pendingDirection of this.#pending.values()) {
        if (pendingDirection === direction) inFlight += 1;
      }
      if (inFlight >= MAX_IN_FLIGHT_EACH_DIRECTION) throw new ProtocolFailure("overloaded");
      this.#pending.set(frame.id, direction);
    } else if (frame.kind === "response" || frame.kind === "error") {
      if (this.#answered.has(frame.id)) throw new ProtocolFailure("second_terminal_response");
      // A late first terminal for a cancelled or timed-out ID is ignored
      // without accepting a second one.
      if (this.#abandoned.delete(frame.id)) {
        this.#answered.add(frame.id);
        return IGNORE;
      }
      const requestDirection = this.#pending.get(frame.id);
      if (!requestDirection || requestDirection === direction) {
        throw new ProtocolFailure("unknown_response");
      }
      this.#pending.delete(frame.id);
      this.#answered.add(frame.id);
    } else if (frame.kind === "cancel") {
      if (this.#answered.has(frame.targetId) || this.#abandoned.has(frame.targetId)) return IGNORE;
      const requestDirection = this.#pending.get(frame.targetId);
      if (!requestDirection || requestDirection !== direction) {
        throw new ProtocolFailure("unknown_request");
      }
      this.#pending.delete(frame.targetId);
      this.#abandoned.add(frame.targetId);
    } else if (frame.kind === "stream_open") {
      if (this.#streams.has(frame.id) || this.#closedStreams.has(frame.id)) {
        throw new ProtocolFailure("duplicate_id");
      }
      if (frame.initialCredit !== INITIAL_STREAM_CREDIT) throw new ProtocolFailure("credit_violation");
      this.#streams.set(frame.id, { sender: direction === "host" ? "package" : "host", credit: frame.initialCredit, sequence: 0n, retired: false });
    } else if (frame.kind === "event") {
      if (this.#closedStreams.has(frame.streamId)) return IGNORE;
      const stream = this.#streams.get(frame.streamId);
      if (!stream || stream.sender !== direction) throw new ProtocolFailure("unknown_stream");
      if (stream.retired) return IGNORE;
      if (stream.credit === 0) throw new ProtocolFailure("credit_violation");
      const sequence = decimal(frame.sequence);
      if (sequence !== stream.sequence + 1n) throw new ProtocolFailure("event_sequence");
      stream.credit -= 1;
      stream.sequence = sequence;
    } else if (frame.kind === "credit") {
      // The per-frame range is out of contract wherever the stream stands, so
      // it is read before the tombstone: "credit outside `1..=32` is fatal".
      if (frame.amount < 1 || frame.amount > INITIAL_STREAM_CREDIT) {
        throw new ProtocolFailure("credit_violation");
      }
      // Credit released for events the receiver had already accepted is
      // ordinary in-flight traffic, even when the stream has since closed.
      if (this.#closedStreams.has(frame.streamId)) return IGNORE;
      const stream = this.#streams.get(frame.streamId);
      if (!stream || stream.sender === direction) throw new ProtocolFailure("unknown_stream");
      if (stream.retired) return IGNORE;
      // The window is a ceiling, not a starting balance: credit pays for
      // events the receiving adapter has already accepted, so a grant past the
      // window the stream was opened with pays for work that never happened.
      if (stream.credit + frame.amount > INITIAL_STREAM_CREDIT) {
        throw new ProtocolFailure("credit_violation");
      }
      stream.credit += frame.amount;
    } else if (frame.kind === "stream_close") {
      if (this.#streams.delete(frame.streamId)) {
        this.#closedStreams.add(frame.streamId);
      } else if (this.#closedStreams.has(frame.streamId)) {
        return IGNORE;
      } else {
        throw new ProtocolFailure("unknown_stream");
      }
    }
    return DELIVER;
  }
}

function validateDirection(direction, frame) {
  if (frame.id && ["request", "cancel", "stream_open"].includes(frame.kind)) {
    const expected = direction === "host" ? "h:" : "p:";
    if (!frame.id.startsWith(expected)) throw new ProtocolFailure("invalid_id_namespace");
  }
}

/** The largest value a decimal string may carry: both peers parse into 64 bits. */
const MAX_DECIMAL = 2n ** 64n - 1n;

function decimal(value) {
  if (typeof value !== "string" || !/^(0|[1-9][0-9]*)$/.test(value)) {
    throw new ProtocolFailure("invalid_decimal");
  }
  const parsed = BigInt(value);
  // A decimal string above the 64-bit range is not a number either peer can
  // carry, and the Rust adapter, which parses into `u64`, calls that
  // `invalid_decimal`. An unbounded BigInt survived to the sequence
  // comparison instead and was reported as `event_sequence`, so the two
  // implementations gave the same frame two different stable codes.
  if (parsed > MAX_DECIMAL) throw new ProtocolFailure("invalid_decimal");
  return parsed;
}

function rejectDuplicateKeys(text) {
  let index = 0;
  const whitespace = () => { while (/\s/.test(text[index] ?? "")) index += 1; };
  const string = () => {
    const start = index;
    if (text[index++] !== '"') throw new SyntaxError("string");
    while (index < text.length) {
      if (text[index] === "\\") { index += 2; continue; }
      if (text[index++] === '"') return JSON.parse(text.slice(start, index));
    }
    throw new SyntaxError("unterminated string");
  };
  const value = () => {
    whitespace();
    if (text[index] === "{") return object();
    if (text[index] === "[") return array();
    if (text[index] === '"') { string(); return; }
    const match = /^(?:-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?|true|false|null)/.exec(text.slice(index));
    if (!match) throw new SyntaxError("value");
    index += match[0].length;
  };
  const object = () => {
    index += 1;
    whitespace();
    const keys = new Set();
    if (text[index] === "}") { index += 1; return; }
    while (true) {
      whitespace();
      const key = string();
      if (keys.has(key)) throw new ProtocolFailure("duplicate_key");
      keys.add(key);
      whitespace();
      if (text[index++] !== ":") throw new SyntaxError("colon");
      value();
      whitespace();
      if (text[index] === "}") { index += 1; return; }
      if (text[index++] !== ",") throw new SyntaxError("comma");
    }
  };
  const array = () => {
    index += 1;
    whitespace();
    if (text[index] === "]") { index += 1; return; }
    while (true) {
      value();
      whitespace();
      if (text[index] === "]") { index += 1; return; }
      if (text[index++] !== ",") throw new SyntaxError("comma");
    }
  };
  value();
  whitespace();
  if (index !== text.length) throw new SyntaxError("trailing input");
}

async function serve() {
  let buffer = Buffer.alloc(0);
  let delayedRequest;
  let packageIdentity;
  let capabilitySequence = 0;
  const capabilityGrants = new Map();
  const pendingCapabilityCalls = new Map();
  const membershipEvents = [];
  const principalEvents = [];
  let principalStream;
  let membershipStream;
  // Events consumed on the current membership stream that credit has already
  // been released for. The contract lets a sender release credit only for work
  // the receiving adapter has accepted, so this is the ceiling on what
  // `fixture.membership_credit` may put on the wire.
  let membershipCredited = 0;
  const immutableEvents = [];
  /** Events recorded before the open immutable stream, which it never credits. */
  let immutableCreditBase = 0;
  /** Events of the open immutable stream this fixture has already credited. */
  let immutableCredited = 0;
  let immutableStream;
  let immutableCloseCode = null;
  let portQueryStream;
  const portEvents = [];
  let portCloseCode = null;
  let portEventStream;
  const portEventEvents = [];
  let portEventClose = null;
  const providerEventStreams = new Map();
  let providerEventOpens = 0;
  let providerEventCloses = 0;
  const uiStreams = new Map();
  const send = (frame) => process.stdout.write(encodeFrame(frame));
  for await (const chunk of process.stdin) {
    buffer = Buffer.concat([buffer, chunk]);
    while (buffer.length >= 4) {
      const length = buffer.readUInt32BE(0);
      if (length === 0 || length > MAX_FRAME_BYTES) process.exit(2);
      if (buffer.length < length + 4) break;
      const frame = decodeFrame(buffer.subarray(0, length + 4));
      buffer = buffer.subarray(length + 4);
      if (frame.kind === "initialize") {
        packageIdentity = frame.package.identity;
        capabilityGrants.clear();
        for (const capability of frame.capabilities) capabilityGrants.set(capability.identity, capability.grant);
        send({ kind: "initialized", nonce: frame.nonce, package: frame.package, uiContractDigest: frame.uiContractDigest, processGeneration: frame.processGeneration, protocol: frame.protocol });
      } else if (frame.kind === "ping") {
        send({ kind: "pong", nonce: frame.nonce });
      } else if (frame.kind === "request" && frame.operation.startsWith("lifecycle.")) {
        const operation = frame.operation.slice("lifecycle.".length);
        if (["activate", "recover"].includes(operation) && packageIdentity?.endsWith(".crash")) process.abort();
        if (packageIdentity?.endsWith(".lifecycle-error")) {
          send({ kind: "error", id: frame.id, code: "fixture.lifecycle", retryable: true, details: {} });
        } else {
          send({ kind: "response", id: frame.id, payload: { state: operation === "recover" ? "active" : operation } });
          if (operation === "activate" && capabilityGrants.has("teamarness.workspace-context")) {
            send({ kind: "request", id: "p:fixture-cap", operation: "capability.call", grant: capabilityGrants.get("teamarness.workspace-context"), deadlineUnixMs: frame.deadlineUnixMs, payload: { method: "get" } });
          }
        }
      } else if ((frame.kind === "response" || frame.kind === "error") && pendingCapabilityCalls.has(frame.id)) {
        const requestId = pendingCapabilityCalls.get(frame.id);
        pendingCapabilityCalls.delete(frame.id);
        send({ kind: "response", id: requestId, payload: frame.kind === "response" ? { result: frame.payload } : { error: frame.code, retryable: frame.retryable ?? false, outcomeUncertain: frame.outcomeUncertain ?? false, details: frame.details ?? {} } });
      } else if (frame.kind === "event" && frame.streamId === principalStream) {
        principalEvents.push(frame.payload);
      } else if (frame.kind === "event" && frame.streamId === membershipStream) {
        membershipEvents.push(frame.payload);
      } else if (frame.kind === "event" && frame.streamId === immutableStream) {
        immutableEvents.push(frame.payload);
      } else if (frame.kind === "event" && frame.streamId === portQueryStream) {
        portEvents.push(frame.payload);
      } else if (frame.kind === "event" && frame.streamId === portEventStream) {
        portEventEvents.push(frame.payload);
      } else if (frame.kind === "stream_close" && frame.streamId === membershipStream) {
        membershipStream = undefined;
      } else if (frame.kind === "stream_close" && frame.streamId === immutableStream) {
        immutableCloseCode = frame.code ?? null;
        immutableStream = undefined;
      } else if (frame.kind === "stream_close" && frame.streamId === portQueryStream) {
        portCloseCode = frame.code ?? null;
        portQueryStream = undefined;
      } else if (frame.kind === "stream_close" && frame.streamId === portEventStream) {
        portEventClose = frame.code ?? null;
        portEventStream = undefined;
      } else if (frame.kind === "stream_open") {
        uiStreams.set(frame.id, { topic: frame.topic, sequence: 0 });
        if (frame.topic === "teamarness.port.event.v1") {
          providerEventOpens += 1;
          providerEventStreams.set(frame.id, { sequence: 0, credit: frame.initialCredit });
        } else if (frame.topic === "teamarness.port.query.v1") {
          if (frame.payload.port === "message.title_watch") {
            send({ kind: "event", streamId: frame.id, sequence: "1", payload: { $teamarnessPortEnvelope: 1, $teamarnessPortValue: [1, 2] } });
          } else if (frame.payload.port === "message.marker_watch") {
            send({ kind: "event", streamId: frame.id, sequence: "1", payload: { $teamarnessPortEnvelope: 1,
              $teamarnessPortValue: { $teamarnessPortEnvelope: 1, $teamarnessPortValue: "literal" } } });
          } else if (frame.payload.port === "message.large_watch") {
            send({ kind: "event", streamId: frame.id, sequence: "1", payload: { $teamarnessPortEnvelope: 1,
              $teamarnessPortValue: "x".repeat(262142) } });
          } else {
            send({ kind: "event", streamId: frame.id, sequence: "1", payload: { order: packageIdentity === "com.example.invalid" ? "invalid" : 1 } });
            if (packageIdentity !== "com.example.invalid") send({ kind: "event", streamId: frame.id, sequence: "2", payload: { order: 2 } });
          }
          send({ kind: "stream_close", streamId: frame.id, code: "completed" });
          uiStreams.delete(frame.id);
        }
        else if (frame.topic === "fixture.close") {
          send({ kind: "stream_close", streamId: frame.id, code: "closed" });
          uiStreams.delete(frame.id);
        } else if (frame.topic === "fixture.rejected_stream") {
          send({ kind: "stream_close", streamId: frame.id, code: "fixture.stream_rejected" });
          uiStreams.delete(frame.id);
        } else if (frame.topic === "fixture.complete") {
          // Events, then an ordinary completion: the subscriber's
          // acknowledgement for the last event is still in flight when the
          // stream ends.
          for (let sequence = 1; sequence <= 2; sequence += 1) {
            send({ kind: "event", streamId: frame.id, sequence: String(sequence), payload: { order: sequence } });
          }
          send({ kind: "stream_close", streamId: frame.id, code: "completed" });
          uiStreams.delete(frame.id);
        } else if (frame.topic === "fixture.invalid_event") {
          send({ kind: "event", streamId: frame.id, sequence: "1", payload: { order: "invalid" } });
        } else if (frame.topic === "fixture.duplicate") {
          send({ kind: "event", streamId: frame.id, sequence: "1", payload: { order: 1 } });
          send({ kind: "event", streamId: frame.id, sequence: "1", payload: { order: 1 } });
        } else if (frame.topic === "fixture.gap") {
          send({ kind: "event", streamId: frame.id, sequence: "2", payload: { order: 2 } });
        } else {
          const count = ["fixture.saturation", "fixture.overflow"].includes(frame.topic) ? 32 : 2;
          for (let sequence = 1; sequence <= count; sequence += 1) {
            send({ kind: "event", streamId: frame.id, sequence: String(sequence), payload: { order: sequence } });
          }
          uiStreams.get(frame.id).sequence = count;
          if (frame.topic === "fixture.overflow") {
            send({ kind: "event", streamId: frame.id, sequence: "33", payload: { order: 33 } });
          }
        }
      } else if (frame.kind === "credit" && uiStreams.has(frame.streamId)) {
        const stream = uiStreams.get(frame.streamId);
        if (stream.topic === "teamarness.port.event.v1") {
          const providerStream = providerEventStreams.get(frame.streamId);
          if (providerStream) providerStream.credit += frame.amount;
        }
        if (stream.topic === "fixture.saturation") {
          for (let index = 0; index < frame.amount; index += 1) {
            stream.sequence += 1;
            send({ kind: "event", streamId: frame.streamId, sequence: String(stream.sequence), payload: { order: stream.sequence } });
          }
        }
      } else if (frame.kind === "stream_close" && uiStreams.has(frame.streamId)) {
        if (providerEventStreams.delete(frame.streamId)) providerEventCloses += 1;
        uiStreams.delete(frame.streamId);
      } else if (frame.kind === "request" && frame.operation === "fixture.principals_open") {
        capabilitySequence += 1;
        principalStream = `p:principals-${capabilitySequence}`;
        send({ kind: "stream_open", id: principalStream, topic: "capability.stream", initialCredit: 32,
          payload: { grant: capabilityGrants.get("teamarness.principals"), method: "subscribe", afterCursor: frame.payload.afterCursor } });
        send({ kind: "response", id: frame.id, payload: { streamId: principalStream } });
      } else if (frame.kind === "request" && frame.operation === "fixture.principals_close") {
        if (principalStream) send({ kind: "stream_close", streamId: principalStream, code: "closed" });
        principalStream = undefined;
        send({ kind: "response", id: frame.id, payload: { closed: true } });
      } else if (frame.kind === "request" && frame.operation === "fixture.principals_events") {
        send({ kind: "response", id: frame.id, payload: { events: principalEvents } });
      } else if (frame.kind === "request" && frame.operation === "fixture.membership_open") {
        capabilitySequence += 1;
        membershipStream = `p:membership-${capabilitySequence}`;
        // A new stream carries a fresh window: only events this stream
        // delivers can be credited back to it.
        membershipCredited = membershipEvents.length;
        send({
          kind: "stream_open",
          id: membershipStream,
          topic: "capability.stream",
          initialCredit: 32,
          payload: {
            grant: capabilityGrants.get("teamarness.membership"),
            method: "subscribe",
            afterEpoch: frame.payload.afterEpoch,
          },
        });
        send({ kind: "response", id: frame.id, payload: { streamId: membershipStream } });
      } else if (frame.kind === "request" && frame.operation === "fixture.membership_credit") {
        // What crosses the wire is what this stream has consumed and not yet
        // credited, never what the request asked for: credit released for
        // events that were never accepted would widen the window past the 32
        // the stream was opened with.
        const amount = Math.min(frame.payload.amount, membershipEvents.length - membershipCredited);
        if (amount > 0) {
          membershipCredited += amount;
          send({ kind: "credit", streamId: membershipStream, amount });
        }
        send({ kind: "response", id: frame.id, payload: { credited: amount } });
      } else if (frame.kind === "request" && frame.operation === "fixture.membership_close") {
        if (membershipStream) send({ kind: "stream_close", streamId: membershipStream, code: "closed" });
        membershipStream = undefined;
        send({ kind: "response", id: frame.id, payload: { closed: true } });
      } else if (frame.kind === "request" && frame.operation === "fixture.membership_events") {
        send({ kind: "response", id: frame.id, payload: { events: membershipEvents } });
      } else if (frame.kind === "request" && ["fixture.immutable_append", "fixture.record_append"].includes(frame.operation)) {
        capabilitySequence += 1;
        const id = `p:immutable-append-${capabilitySequence}`;
        pendingCapabilityCalls.set(id, frame.id);
        send({
          kind: "request", id, operation: "capability.call",
          grant: frame.payload.grant ?? capabilityGrants.get(immutableCapability),
          deadlineUnixMs: frame.deadlineUnixMs,
          payload: { method: "append", purpose: frame.payload.purpose, canonical: frame.payload.canonical,
            ...(frame.payload.principal === undefined ? {} : { principal: frame.payload.principal }) },
        });
      } else if (frame.kind === "request" && ["fixture.immutable_open", "fixture.record_open"].includes(frame.operation)) {
        capabilitySequence += 1;
        const id = `p:immutable-open-${capabilitySequence}`;
        pendingCapabilityCalls.set(id, frame.id);
        send({
          kind: "request", id, operation: "capability.call",
          grant: frame.payload.grant ?? capabilityGrants.get(immutableCapability),
          deadlineUnixMs: frame.deadlineUnixMs,
          payload: { method: "open", sealedRecords: frame.payload.sealedRecords },
        });
      } else if (frame.kind === "request" && ["fixture.immutable_subscribe", "fixture.record_subscribe"].includes(frame.operation)) {
        capabilitySequence += 1;
        immutableStream = `p:immutable-${capabilitySequence}`;
        immutableCreditBase = immutableEvents.length;
        immutableCredited = 0;
        send({
          kind: "stream_open", id: immutableStream, topic: "capability.stream", initialCredit: 32,
          payload: {
            grant: frame.payload.grant ?? capabilityGrants.get(immutableCapability),
            method: "subscribe", afterCursor: frame.payload.afterCursor ?? null,
          },
        });
        send({ kind: "response", id: frame.id, payload: { streamId: immutableStream } });
      } else if (frame.kind === "request" && ["fixture.immutable_events", "fixture.record_events"].includes(frame.operation)) {
        send({ kind: "response", id: frame.id, payload: { events: immutableEvents, closeCode: immutableCloseCode } });
      } else if (frame.kind === "request" && ["fixture.immutable_credit", "fixture.record_credit"].includes(frame.operation)) {
        // `frames` returns the window one event at a time, the way a consumer
        // that commits before crediting does; the default is a single frame.
        //
        // What crosses the wire is what this stream has consumed and not yet
        // credited, never what the request asked for. The 32 events are a
        // ceiling on the window, so a grant that would carry the stream past
        // it is `credit_violation` and fatal to the generation — two frames of
        // one is enough on an empty scope, which emits a single state event.
        const frames = frame.payload.frames ?? 1;
        let credited = 0;
        for (let index = 0; index < frames; index += 1) {
          const consumed = immutableEvents.length - immutableCreditBase - immutableCredited;
          const amount = Math.min(frame.payload.amount, consumed);
          if (amount <= 0) break;
          immutableCredited += amount;
          credited += amount;
          send({ kind: "credit", streamId: immutableStream, amount });
        }
        send({ kind: "response", id: frame.id, payload: { credited } });
      } else if (frame.kind === "request" && ["fixture.immutable_close", "fixture.record_close"].includes(frame.operation)) {
        if (immutableStream) send({ kind: "stream_close", streamId: immutableStream, code: "closed" });
        immutableStream = undefined;
        send({ kind: "response", id: frame.id, payload: { closed: true } });
      } else if (frame.kind === "request" && ["teamarness.port.command.v1", "teamarness.port.query.v1"].includes(frame.operation)) {
        if (frame.payload.request.text === "stall") {
          delayedRequest = frame;
        } else if (frame.payload.request.text === "exit") {
          await import("node:fs/promises").then((fs) => fs.appendFile("port-command-attempts", "1\n"));
          process.abort();
        } else if (frame.payload.port === "message.title") {
          send({ kind: "response", id: frame.id, payload: { $teamarnessPortEnvelope: 1, $teamarnessPortValue: "hello" } });
        } else if (frame.payload.port === "message.marker") {
          send({ kind: "response", id: frame.id, payload: { $teamarnessPortEnvelope: 1,
            $teamarnessPortValue: { $teamarnessPortEnvelope: 1, $teamarnessPortValue: "literal" } } });
        } else if (frame.payload.request.text === "bad_response") {
          send({ kind: "response", id: frame.id, payload: { accepted: "not a boolean" } });
        } else if (frame.payload.request.text === "provider_error") {
          send({ kind: "error", id: frame.id, code: "fixture.rejected", retryable: false, details: { reason: "fixture" } });
        } else {
          send({ kind: "response", id: frame.id, payload: frame.operation === "teamarness.port.query.v1"
            ? { accepted: true, actor: frame.payload.actor } : { accepted: true } });
        }
      } else if (frame.kind === "request" && frame.operation === "fixture.port_cancel") {
        capabilitySequence += 1;
        const id = `p:port-cancel-${capabilitySequence}`;
        send({ kind: "request", id, operation: "capability.call",
          grant: capabilityGrants.get("teamarness.package-ports"), deadlineUnixMs: frame.deadlineUnixMs,
          payload: { method: "invoke", contract: "com.example.messages", port: "message.post",
            kind: "command", request: { text: "stall" } } });
        send({ kind: "cancel", id: `p:cancel-port-${capabilitySequence}`, targetId: id });
        send({ kind: "response", id: frame.id, payload: { cancelled: true } });
      } else if (frame.kind === "request" && frame.operation === "fixture.port_open") {
        capabilitySequence += 1;
        portQueryStream = `p:port-query-${capabilitySequence}`;
        portCloseCode = null;
        portEvents.length = 0;
        send({ kind: "stream_open", id: portQueryStream, topic: "capability.stream", initialCredit: 32,
          payload: { grant: capabilityGrants.get("teamarness.package-ports"),
            contract: "com.example.messages", port: frame.payload.port ?? "message.watch", kind: "query", request: {} } });
        send({ kind: "response", id: frame.id, payload: { opened: true } });
      } else if (frame.kind === "request" && frame.operation === "fixture.port_events") {
        send({ kind: "response", id: frame.id, payload: frame.payload.summary
          ? { count: portEvents.length, firstLength: portEvents[0]?.$teamarnessPortValue?.length ?? null, closeCode: portCloseCode }
          : { events: portEvents, closeCode: portCloseCode } });
      } else if (frame.kind === "request" && frame.operation === "fixture.port_event_open") {
        capabilitySequence += 1;
        portEventStream = `p:port-event-${capabilitySequence}`;
        portEventEvents.length = 0;
        portEventClose = null;
        send({ kind: "stream_open", id: portEventStream, topic: "capability.stream", initialCredit: 32,
          payload: { grant: frame.payload.grant ?? capabilityGrants.get("teamarness.package-ports"),
            contract: "com.example.messages", port: frame.payload.port ?? "message.created", kind: "event",
            ...(frame.payload.extra ?? {}) } });
        send({ kind: "response", id: frame.id, payload: { opened: true } });
      } else if (frame.kind === "request" && frame.operation === "fixture.port_event_events") {
        send({ kind: "response", id: frame.id, payload: frame.payload.summary
          ? { count: portEventEvents.length, lastOrder: portEventEvents.at(-1)?.order ?? null,
              paddingLength: portEventEvents.at(-1)?.padding?.length ?? null, closeCode: portEventClose }
          : { events: portEventEvents, closeCode: portEventClose } });
      } else if (frame.kind === "request" && frame.operation === "fixture.port_event_credit") {
        if (portEventStream && frame.payload.amount > 0) {
          send({ kind: "credit", streamId: portEventStream, amount: frame.payload.amount });
        }
        send({ kind: "response", id: frame.id, payload: { credited: true } });
      } else if (frame.kind === "request" && frame.operation === "fixture.port_event_close") {
        if (portEventStream) send({ kind: "stream_close", streamId: portEventStream, code: "closed" });
        portEventStream = undefined;
        send({ kind: "response", id: frame.id, payload: { closed: true } });
      } else if (frame.kind === "request" && frame.operation === "fixture.port_event_emit") {
        let emitted = 0;
        for (const [streamId, stream] of providerEventStreams) {
          if (stream.credit === 0) continue;
          stream.credit -= 1;
          stream.sequence += 1;
          send({ kind: "event", streamId, sequence: String(stream.sequence), payload: {
            order: frame.payload.order ?? stream.sequence,
            ...(frame.payload.invalid ? { order: "invalid" } : {}),
            ...(frame.payload.padding ? { padding: frame.payload.padding } : {})
          } });
          emitted += 1;
        }
        send({ kind: "response", id: frame.id, payload: { emitted } });
      } else if (frame.kind === "request" && frame.operation === "fixture.port_event_stats") {
        send({ kind: "response", id: frame.id, payload: { opens: providerEventOpens, closes: providerEventCloses,
          active: providerEventStreams.size } });
      } else if (frame.kind === "request" && frame.operation === "fixture.grant") {
        send({ kind: "response", id: frame.id, payload: { grant: capabilityGrants.get(frame.payload.capability) ?? "missing" } });
      } else if (frame.kind === "request" && frame.operation === "fixture.capability") {
        capabilitySequence += 1;
        const id = `p:fixture-cap-${capabilitySequence}`;
        pendingCapabilityCalls.set(id, frame.id);
        send({
          kind: "request",
          id,
          operation: "capability.call",
          grant: frame.payload.grant ?? capabilityGrants.get(frame.payload.capability) ?? "missing",
          deadlineUnixMs: frame.deadlineUnixMs,
          payload: { method: frame.payload.method, ...(frame.payload.arguments ?? {}) },
        });
      } else if (frame.kind === "request" && frame.operation === "fixture.typed") {
        send({ kind: "response", id: frame.id, payload: { accepted: true } });
      } else if (frame.kind === "request" && frame.operation === "fixture.invalid_response") {
        send({ kind: "response", id: frame.id, payload: { value: false } });
      } else if (frame.kind === "request" && frame.operation === "fixture.error") {
        send({ kind: "error", id: frame.id, code: "fixture.error", retryable: false, details: { field: "value" } });
      } else if (frame.kind === "request" && frame.operation === "fixture.abort") {
        await import("node:fs/promises").then((fs) => fs.appendFile("mutation-attempts", "1\n"));
        process.abort();
      } else if (frame.kind === "request" && frame.operation === "fixture.malformed") {
        process.stdout.write(Buffer.from([0, 0, 0, 1, 0x7b]));
      } else if (frame.kind === "request" && frame.operation === "fixture.unknown_response") {
        send({ kind: "response", id: "h:unknown", payload: {} });
      } else if (frame.kind === "request" && frame.operation === "fixture.stderr") {
        for (let index = 0; index < 100; index += 1) process.stderr.write(`${"x".repeat(20_000)}\n`);
        send({ kind: "response", id: frame.id, payload: { written: true } });
      } else if (frame.kind === "request" && frame.operation === "fixture.delayed") {
        delayedRequest = frame;
      } else if (frame.kind === "request" && frame.operation === "fixture.immediate") {
        send({ kind: "response", id: frame.id, payload: { order: 2 } });
        if (delayedRequest) {
          send({ kind: "response", id: delayedRequest.id, payload: { order: 1 } });
          delayedRequest = undefined;
        }
      } else if (frame.kind === "cancel") {
        if (delayedRequest?.id === frame.targetId) delayedRequest = undefined;
        send({ kind: "error", id: frame.targetId, code: "cancelled", retryable: true });
      } else if (frame.kind === "shutdown") {
        send({ kind: "shutdown_ack" });
        return;
      }
    }
  }
}

if (process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url) {
  serve().catch(() => process.exit(2));
}
