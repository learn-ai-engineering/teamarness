#!/usr/bin/env node

import { pathToFileURL } from "node:url";

export const MAX_FRAME_BYTES = 1_048_576;
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

export class ContractState {
  #pending = new Map();
  #terminal = new Set();
  #streams = new Map();

  observe(direction, frame) {
    validateDirection(direction, frame);
    if (frame.kind === "initialize" && frame.protocol?.major !== 1) {
      throw new ProtocolFailure("incompatible_version");
    }
    if (frame.kind === "request") {
      if (this.#pending.has(frame.id) || this.#terminal.has(frame.id)) {
        throw new ProtocolFailure("duplicate_id");
      }
      this.#pending.set(frame.id, direction);
    } else if (frame.kind === "response" || frame.kind === "error") {
      if (this.#terminal.has(frame.id)) throw new ProtocolFailure("second_terminal_response");
      const requestDirection = this.#pending.get(frame.id);
      if (!requestDirection || requestDirection === direction) {
        throw new ProtocolFailure("unknown_response");
      }
      this.#pending.delete(frame.id);
      this.#terminal.add(frame.id);
    } else if (frame.kind === "cancel") {
      const requestDirection = this.#pending.get(frame.targetId);
      if (!requestDirection || requestDirection !== direction) {
        throw new ProtocolFailure("unknown_request");
      }
    } else if (frame.kind === "stream_open") {
      if (this.#streams.has(frame.id)) throw new ProtocolFailure("duplicate_id");
      this.#streams.set(frame.id, { sender: direction === "host" ? "package" : "host", credit: frame.initialCredit, sequence: 0n });
    } else if (frame.kind === "event") {
      const stream = this.#streams.get(frame.streamId);
      if (!stream || stream.sender !== direction) throw new ProtocolFailure("unknown_stream");
      if (stream.credit === 0) throw new ProtocolFailure("credit_violation");
      const sequence = decimal(frame.sequence);
      if (sequence !== stream.sequence + 1n) throw new ProtocolFailure("event_sequence");
      stream.credit -= 1;
      stream.sequence = sequence;
    } else if (frame.kind === "credit") {
      const stream = this.#streams.get(frame.streamId);
      if (!stream || stream.sender === direction) throw new ProtocolFailure("unknown_stream");
      stream.credit += frame.amount;
    } else if (frame.kind === "stream_close") {
      if (!this.#streams.delete(frame.streamId)) throw new ProtocolFailure("unknown_stream");
    }
  }
}

function validateDirection(direction, frame) {
  if (frame.id && ["request", "cancel", "stream_open"].includes(frame.kind)) {
    const expected = direction === "host" ? "h:" : "p:";
    if (!frame.id.startsWith(expected)) throw new ProtocolFailure("invalid_id_namespace");
  }
}

function decimal(value) {
  if (typeof value !== "string" || !/^(0|[1-9][0-9]*)$/.test(value)) {
    throw new ProtocolFailure("invalid_decimal");
  }
  return BigInt(value);
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
        send({ kind: "initialized", nonce: frame.nonce, package: frame.package, uiContractDigest: frame.uiContractDigest, processGeneration: frame.processGeneration, protocol: frame.protocol });
      } else if (frame.kind === "ping") {
        send({ kind: "pong", nonce: frame.nonce });
      } else if (frame.kind === "request" && frame.operation.startsWith("lifecycle.")) {
        const operation = frame.operation.slice("lifecycle.".length);
        send({ kind: "response", id: frame.id, payload: { state: operation === "recover" ? "active" : operation } });
        if (operation === "activate") {
          send({ kind: "request", id: "p:fixture-cap", operation: "capability.call", grant: "Z3JhbnQx", deadlineUnixMs: frame.deadlineUnixMs, payload: { method: "get" } });
        }
      } else if (frame.kind === "stream_open") {
        send({ kind: "event", streamId: frame.id, sequence: "1", payload: { order: 1 } });
        send({ kind: "event", streamId: frame.id, sequence: "2", payload: { order: 2 } });
      } else if (frame.kind === "request" && frame.operation === "fixture.delayed") {
        delayedRequest = frame;
      } else if (frame.kind === "request" && frame.operation === "fixture.immediate") {
        send({ kind: "response", id: frame.id, payload: { order: 2 } });
        send({ kind: "response", id: delayedRequest.id, payload: { order: 1 } });
      } else if (frame.kind === "cancel") {
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
