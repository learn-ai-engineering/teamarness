import assert from "node:assert/strict";
import { once } from "node:events";
import { readdir, readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";
import test from "node:test";

import {
  ContractState,
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

function observeSequence(entries) {
  const state = new ContractState();
  for (const { direction, frame } of entries) {
    state.observe(direction, parsePayload(Buffer.from(JSON.stringify(frame))));
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
  assert.equal(goldenPaths.length, 8);
  const kinds = new Set();
  for (const path of goldenPaths) {
    const entries = (await readFile(path, "utf8")).trim().split("\n").map(JSON.parse);
    entries.forEach(({ frame }) => kinds.add(frame.kind));
    observeSequence(entries);
  }
  assert.deepEqual([...kinds].sort(), [
    "cancel", "credit", "error", "event", "initialize", "initialized", "ping", "pong",
    "request", "response", "shutdown", "shutdown_ack", "stream_close", "stream_open",
  ]);
});

test("Node consumes every malformed fixture with its stable error", async () => {
  const paths = await files("invalid", ".json");
  assert.equal(paths.length, 14);
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
