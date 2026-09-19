import assert from "node:assert/strict";
import { once } from "node:events";
import { readdir, readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";
import test from "node:test";

import {
  ContractState,
  DELIVER,
  IGNORE,
  INITIAL_STREAM_CREDIT,
  MAX_DECODED_FRAME_BYTES,
  MAX_FRAME_BYTES,
  MAX_IN_FLIGHT_EACH_DIRECTION,
  MAX_WRITER_BYTES,
  MAX_WRITER_FRAMES,
  ProtocolFailure,
  decodeFrame,
  encodeFrame,
  parsePayload,
} from "./node-package-process.mjs";

const fixtureDirectory = dirname(fileURLToPath(import.meta.url));
const root = join(fixtureDirectory, "..");

async function files(directory, suffix) {
  return (await readdir(join(root, directory)))
    .filter((name) => name.endsWith(suffix))
    .sort()
    .map((name) => join(root, directory, name));
}

/**
 * Runs one corpus sequence through the classifier.
 *
 * An entry either retires state — `{"retire":{"request":id}}` or
 * `{"retire":{"stream":id}}` — or carries one frame and the verdict that frame
 * must receive. Without the declared verdict a sequence proves only "this is
 * not fatal", and would pass unchanged if this implementation delivered a
 * frame the Rust one ignored.
 */
function observeSequence(entries) {
  const state = new ContractState();
  for (const entry of entries) {
    if (entry.retire !== undefined) {
      if (entry.retire.request !== undefined) state.retireRequest(entry.retire.request);
      else if (entry.retire.stream !== undefined) state.retireStream(entry.retire.stream);
      else assert.fail("a retire entry names a request or a stream");
      continue;
    }
    const verdict = state.observe(
      entry.direction,
      parsePayload(Buffer.from(JSON.stringify(entry.frame))),
    );
    if (entry.verdict !== undefined) {
      assert.equal(
        verdict,
        { deliver: DELIVER, ignore: IGNORE }[entry.verdict],
        JSON.stringify(entry.frame),
      );
    }
  }
}

function errorCode(action) {
  try {
    action();
  } catch (error) {
    assert.ok(error instanceof ProtocolFailure);
    return error.code;
  }
  assert.fail("fixture did not fail");
}

function launchFixture(t) {
  const child = spawn(process.execPath, [join(fixtureDirectory, "node-package-process.mjs")], {
    stdio: ["pipe", "pipe", "pipe"],
  });
  t.after(() => child.kill("SIGKILL"));
  let buffered = Buffer.alloc(0);
  const waiting = [];
  const ready = [];
  child.stdout.on("data", (chunk) => {
    buffered = Buffer.concat([buffered, chunk]);
    while (buffered.length >= 4 && buffered.length >= buffered.readUInt32BE(0) + 4) {
      const length = buffered.readUInt32BE(0);
      const frame = decodeFrame(buffered.subarray(0, length + 4));
      buffered = buffered.subarray(length + 4);
      const resolve = waiting.shift();
      if (resolve) resolve(frame);
      else ready.push(frame);
    }
  });
  return {
    child,
    send: (frame) => child.stdin.write(encodeFrame(frame)),
    receive: () => ready.length > 0
      ? Promise.resolve(ready.shift())
      : new Promise((resolve) => waiting.push(resolve)),
  };
}

function initializeFixture(send) {
  const digest = "a".repeat(64);
  send({
    kind: "initialize", nonce: "bGF1bmNoX25vbmNl",
    package: { identity: "dev.teamarness.fixture", version: "1.0.0", manifestDigest: digest, backendDigest: "b".repeat(64) },
    uiContractDigest: "c".repeat(64), processGeneration: "1",
    workspace: { id: "w", name: "Workspace" }, installation: { identity: "i", displayName: "Installation" },
    dataDirectory: { path: "/tmp/fixture", continuityId: "Y29udGludWl0eQ" }, protocol: { major: 1, minor: 0 },
    capabilities: [{ grant: "Z3JhbnQx", identity: "teamarness.synchro" + "nization.immutable-record-log", major: 1, minor: 0 }],
    limits: { maxFrameBytes: 1048576, maxInFlightEachDirection: 32, maxWriterFrames: 64, maxWriterBytes: 8388608, initialStreamCredit: 32 },
  });
}

test("Node consumes every normative schema and golden sequence", async () => {
  const schemaPaths = await files("schemas", ".json");
  assert.equal(schemaPaths.length, 19);
  for (const path of schemaPaths) {
    const schema = JSON.parse(await readFile(path, "utf8"));
    assert.equal(schema.$schema, "https://json-schema.org/draft/2020-12/schema");
    for (const reference of JSON.stringify(schema).matchAll(/"\$ref":"([^"]+)"/g)) {
      assert.match(reference[1], /^(?:#|frame\.schema\.json#)/);
    }
  }

  const goldenPaths = await files("golden", ".jsonl");
  assert.equal(goldenPaths.length, 10);
  const kinds = new Set();
  let verdicts = 0;
  for (const path of goldenPaths) {
    const entries = (await readFile(path, "utf8")).trim().split("\n").map(JSON.parse);
    for (const entry of entries) {
      if (entry.frame === undefined) continue;
      kinds.add(entry.frame.kind);
      // Every frame the corpus carries declares the verdict it receives, so a
      // line cannot quietly stop asserting one.
      assert.ok(entry.verdict !== undefined, `${path}: ${JSON.stringify(entry.frame)}`);
      verdicts += 1;
    }
    observeSequence(entries);
  }
  assert.deepEqual([...kinds].sort(), [
    "cancel", "credit", "error", "event", "initialize", "initialized", "ping", "pong",
    "request", "response", "shutdown", "shutdown_ack", "stream_close", "stream_open",
  ]);
  assert.equal(verdicts, 44, "every frame in the golden corpus declares the verdict it receives");
});

test("the Node constants match the contract's own limits table", async () => {
  const readme = await readFile(join(root, "README.md"), "utf8");
  const published = (row) => {
    const line = readme.split("\n").find((entry) => entry.startsWith(`| ${row} |`));
    assert.ok(line, `the limits table has no \`${row}\` row`);
    return Number(line.split("|").at(-2).trim().replaceAll(",", ""));
  };
  assert.equal(published("Payload bytes per frame"), MAX_FRAME_BYTES);
  assert.equal(published("Payload bytes per frame"), MAX_DECODED_FRAME_BYTES);
  assert.equal(published("In-flight requests in each direction"), MAX_IN_FLIGHT_EACH_DIRECTION);
  assert.equal(published("Queued frames per writer"), MAX_WRITER_FRAMES);
  assert.equal(published("Queued payload bytes per writer"), MAX_WRITER_BYTES);
  assert.equal(published("Initial event credit per stream"), INITIAL_STREAM_CREDIT);
});

test("Node consumes every malformed fixture with its stable error", async () => {
  const paths = await files("invalid", ".json");
  assert.equal(paths.length, 17);
  for (const path of paths) {
    const fixture = JSON.parse(await readFile(path, "utf8"));
    const actual = errorCode(() => {
      if (fixture.mode === "payload") parsePayload(Buffer.from(fixture.payload));
      else if (fixture.mode === "frame_hex") decodeFrame(Buffer.from(fixture.hex, "hex"));
      else if (fixture.mode === "length") {
        const bytes = Buffer.alloc(4);
        bytes.writeUInt32BE(fixture.length);
        decodeFrame(bytes);
      } else if (fixture.mode === "sequence") observeSequence(fixture.frames);
      else assert.fail(`unsupported fixture mode ${fixture.mode}`);
    });
    assert.equal(actual, fixture.expectedError, path);
  }
});

test("Node immutable capability fixture restarts and resumes from a cursor", async (t) => {
  const cursor = "01" + "00".repeat(24);
  const first = launchFixture(t);
  initializeFixture(first.send);
  assert.equal((await first.receive()).kind, "initialized");
  first.send({ kind: "request", id: "h:append", operation: "fixture.immutable_append", deadlineUnixMs: "9999999999999", payload: { purpose: "message.post", canonical: "YQ" } });
  const appendCall = await first.receive();
  assert.equal(appendCall.kind, "request");
  first.send({ kind: "response", id: appendCall.id, payload: { recordId: "r", sealedRecord: "c2VhbGVk", durability: "committed" } });
  assert.equal((await first.receive()).id, "h:append");
  first.send({ kind: "request", id: "h:subscribe", operation: "fixture.immutable_subscribe", deadlineUnixMs: "9999999999999", payload: { afterCursor: null } });
  const stream = await first.receive();
  assert.equal(stream.kind, "stream_open");
  assert.equal((await first.receive()).id, "h:subscribe");
  first.send({ kind: "event", streamId: stream.id, sequence: "1", payload: { kind: "record", cursor } });
  first.send({ kind: "request", id: "h:events", operation: "fixture.immutable_events", deadlineUnixMs: "9999999999999", payload: {} });
  const firstEvents = await first.receive();
  assert.equal(firstEvents.kind, "response");
  assert.deepEqual(firstEvents.payload.events, [{ kind: "record", cursor }]);
  first.send({ kind: "shutdown", reason: "application_shutdown" });
  assert.equal((await first.receive()).kind, "shutdown_ack");
  first.child.stdin.end();
  await once(first.child, "exit");

  const second = launchFixture(t);
  initializeFixture(second.send);
  assert.equal((await second.receive()).kind, "initialized");
  second.send({ kind: "request", id: "h:resume", operation: "fixture.immutable_subscribe", deadlineUnixMs: "9999999999999", payload: { afterCursor: cursor } });
  const resumed = await second.receive();
  assert.equal(resumed.kind, "stream_open");
  assert.equal((await second.receive()).id, "h:resume");
  const nextCursor = "01" + "00".repeat(23) + "01";
  second.send({ kind: "event", streamId: resumed.id, sequence: "1", payload: { kind: "record", cursor: nextCursor } });
  second.send({ kind: "request", id: "h:resumed-events", operation: "fixture.immutable_events", deadlineUnixMs: "9999999999999", payload: {} });
  const events = (await second.receive()).payload.events;
  assert.deepEqual(events.map((event) => event.cursor), [nextCursor]);
  second.send({ kind: "shutdown", reason: "application_shutdown" });
  assert.equal((await second.receive()).kind, "shutdown_ack");
  second.child.stdin.end();
  await once(second.child, "exit");
});

test("direct Node stdio process completes the v1 control and data flow", async (t) => {
  const child = spawn(process.execPath, [join(fixtureDirectory, "node-package-process.mjs")], {
    stdio: ["pipe", "pipe", "pipe"],
  });
  t.after(() => child.kill("SIGKILL"));

  let buffered = Buffer.alloc(0);
  const waiting = [];
  const ready = [];
  child.stdout.on("data", (chunk) => {
    buffered = Buffer.concat([buffered, chunk]);
    while (buffered.length >= 4 && buffered.length >= buffered.readUInt32BE(0) + 4) {
      const length = buffered.readUInt32BE(0);
      const frame = decodeFrame(buffered.subarray(0, length + 4));
      buffered = buffered.subarray(length + 4);
      const resolve = waiting.shift();
      if (resolve) resolve(frame);
      else ready.push(frame);
    }
  });
  const receive = () => ready.length > 0
    ? Promise.resolve(ready.shift())
    : new Promise((resolve) => waiting.push(resolve));
  const send = (frame) => child.stdin.write(encodeFrame(frame));
  const digest = "a".repeat(64);
  const packageValue = { identity: "dev.teamarness.fixture", version: "1.0.0", manifestDigest: digest, backendDigest: "b".repeat(64) };
  const deadlineUnixMs = "9007199254740993";

  send({
    kind: "initialize", nonce: "bGF1bmNoX25vbmNl", package: packageValue, uiContractDigest: "c".repeat(64),
    processGeneration: "1", workspace: { id: "w", name: "Workspace" },
    installation: { identity: "i", displayName: "Installation" },
    dataDirectory: { path: "/tmp/fixture", continuityId: "Y29udGludWl0eQ" },
    protocol: { major: 1, minor: 0 }, capabilities: [{ grant: "Z3JhbnQx", identity: "teamarness.workspace-context", major: 1, minor: 0 }],
    limits: { maxFrameBytes: 1048576, maxInFlightEachDirection: 32, maxWriterFrames: 64, maxWriterBytes: 8388608, initialStreamCredit: 32 },
  });
  assert.equal((await receive()).kind, "initialized");

  send({ kind: "request", id: "h:activate", operation: "lifecycle.activate", deadlineUnixMs, payload: {} });
  assert.equal((await receive()).id, "h:activate");
  const capabilityCall = await receive();
  assert.equal(capabilityCall.id, "p:fixture-cap");
  send({ kind: "response", id: capabilityCall.id, payload: { workspace: "w" } });

  send({ kind: "request", id: "h:slow", operation: "fixture.delayed", deadlineUnixMs, payload: {} });
  send({ kind: "request", id: "h:fast", operation: "fixture.immediate", deadlineUnixMs, payload: {} });
  assert.deepEqual([(await receive()).id, (await receive()).id], ["h:fast", "h:slow"]);

  send({ kind: "stream_open", id: "h:events", topic: "fixture.events", initialCredit: 32, payload: {} });
  const events = [await receive(), await receive()];
  assert.deepEqual(events.map((event) => event.sequence), ["1", "2"]);
  send({ kind: "credit", streamId: "h:events", amount: 2 });

  send({ kind: "request", id: "h:wait", operation: "fixture.wait", deadlineUnixMs, payload: {} });
  send({ kind: "cancel", id: "h:cancel", targetId: "h:wait" });
  assert.equal((await receive()).code, "cancelled");

  for (const operation of ["suspend", "recover", "dispose"]) {
    const id = `h:${operation}`;
    send({ kind: "request", id, operation: `lifecycle.${operation}`, deadlineUnixMs, payload: {} });
    assert.equal((await receive()).id, id);
  }

  send({ kind: "ping", nonce: "aGVhbHRo" });
  assert.equal((await receive()).kind, "pong");
  send({ kind: "shutdown", reason: "application_shutdown" });
  assert.equal((await receive()).kind, "shutdown_ack");
  child.stdin.end();
  const [code] = await once(child, "exit");
  assert.equal(code, 0);
});
