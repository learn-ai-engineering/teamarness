import assert from "node:assert/strict";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import {
  checkProtocolBounds,
  checkSharedAcknowledgementGranularity,
  checkSharedPayloadBound,
  checkSharedSubscriptionBound,
  findBareBounds,
  repositoryRoot,
} from "./check-protocol-bounds.mjs";

test("every protocol-facing bound in the repository is derived", async () => {
  assert.deepEqual(await checkProtocolBounds(repositoryRoot), []);
});

test("the Shell and the Host publish one subscription bound", async () => {
  assert.deepEqual(await checkSharedSubscriptionBound(repositoryRoot), []);
});

test("a bare channel or semaphore depth fails", () => {
  const source = `fn start() {
    let (tx, rx) = mpsc::channel(16);
    let bytes = Semaphore::new(1_048_576);
    let typed = mpsc::channel::<String>(48);
}
`;
  const problems = findBareBounds("crates/teamarness-package-host/src/protocol.rs", source);
  assert.equal(problems.length, 3);
  assert.match(problems[0], /protocol\.rs:2/);
  assert.match(problems[1], /protocol\.rs:3/);
  assert.match(problems[2], /protocol\.rs:4/);
});

test("a bare arithmetic depth fails too", () => {
  const source = "let bytes = Semaphore::new(8 * 1024 * 1024);\n";
  assert.equal(findBareBounds("crates/teamarness-package-sdk/src/a.rs", source).length, 1);
});

test("a named bound passes", () => {
  const source = `const CONTROL_QUEUE: usize = MAX_IN_FLIGHT_EACH_DIRECTION + CONTROL_RESERVE;

fn start() {
    let (tx, rx) = mpsc::channel(CONTROL_QUEUE);
    let bytes = Semaphore::new(MAX_WRITER_BYTES - CONTROL_BYTES);
    let typed = mpsc::channel::<String>(MAX_IN_FLIGHT_EACH_DIRECTION);
}
`;
  assert.deepEqual(findBareBounds("crates/teamarness-package-host/src/protocol.rs", source), []);
});

test("the Shell and the Host publish one acknowledgement granularity", async () => {
  assert.deepEqual(await checkSharedAcknowledgementGranularity(repositoryRoot), []);
});

test("the Shell and the contract publish one payload bound", async () => {
  assert.deepEqual(await checkSharedPayloadBound(repositoryRoot), []);
});

/** Builds a throwaway tree holding only the files the payload-bound check reads. */
async function payloadBoundTree(contractSource, shellSource) {
  const root = await mkdtemp(join(tmpdir(), "protocol-bounds-payload-"));
  const contract = join(root, "crates/teamarness-package-contract/src");
  const shell = join(root, "apps/teamarness-desktop/src/package-frame");
  await mkdir(contract, { recursive: true });
  await mkdir(shell, { recursive: true });
  await writeFile(join(contract, "ui_contract.rs"), contractSource);
  await writeFile(join(shell, "protocol.ts"), shellSource);
  return root;
}

test("a payload bound that moves in one language alone fails, in either direction", async () => {
  const shellGrew = await payloadBoundTree(
    "pub const MAX_UI_PAYLOAD_BYTES: usize = 256 * 1024;\n",
    "export const MAX_PAYLOAD_BYTES = 256 * 1024 * 2;\n",
  );
  assert.match(
    (await checkSharedPayloadBound(shellGrew))[0],
    /admits 524288-byte payloads while .* permits 262144/,
  );
  const shellShrank = await payloadBoundTree(
    "pub const MAX_UI_PAYLOAD_BYTES: usize = 512 * 1024;\n",
    "export const MAX_PAYLOAD_BYTES = 256 * 1024;\n",
  );
  assert.match(
    (await checkSharedPayloadBound(shellShrank))[0],
    /admits 262144-byte payloads while .* permits 524288/,
  );
});

test("the payload-bound check fails loudly when its constant is gone", async () => {
  const root = await payloadBoundTree("// nothing here\n", "// nothing here\n");
  const problems = await checkSharedPayloadBound(root);
  assert.match(problems[0], /no longer declares MAX_UI_PAYLOAD_BYTES/);
});

/**
 * Builds a throwaway tree holding only the two files a cross-language check
 * reads, so the check can be shown to fail.
 *
 * A guard that is only ever run against a passing tree says nothing about
 * whether it can fail, which is how three guards on this branch came to be
 * tautologies.
 */
async function twoLanguageTree(hostSource, shellSource) {
  const root = await mkdtemp(join(tmpdir(), "protocol-bounds-"));
  const host = join(root, "crates/teamarness-package-host/src");
  const shell = join(root, "apps/teamarness-desktop/src/package-frame");
  await mkdir(host, { recursive: true });
  await mkdir(shell, { recursive: true });
  await writeFile(join(host, "protocol.rs"), hostSource);
  await writeFile(join(host, "ui.rs"), hostSource);
  await writeFile(join(shell, "protocol.ts"), shellSource);
  return root;
}

test("an acknowledgement granularity that moves in one language alone fails", async () => {
  const root = await twoLanguageTree(
    "const EVENTS_PER_ACKNOWLEDGEMENT: usize = 1;\n",
    "export const EVENTS_PER_ACKNOWLEDGEMENT = 4;\n",
  );
  const problems = await checkSharedAcknowledgementGranularity(root);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /acknowledges every 4 event while .* sizes the control lane for every 1/);
});

test("a subscription bound that moves in one language alone fails", async () => {
  const root = await twoLanguageTree(
    "pub const MAX_UI_SUBSCRIPTIONS_PER_MOUNT: usize = 8;\n",
    "export const MAX_OPEN_SUBSCRIPTIONS = 32;\n",
  );
  const problems = await checkSharedSubscriptionBound(root);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /admits 32 open subscriptions per mount while .* serves 8/);
});

test("a cross-language check fails loudly when its constant is gone", async () => {
  const root = await twoLanguageTree("// nothing here\n", "// nothing here\n");
  assert.match(
    (await checkSharedAcknowledgementGranularity(root))[0],
    /no longer declares EVENTS_PER_ACKNOWLEDGEMENT/,
  );
  assert.match(
    (await checkSharedSubscriptionBound(root))[0],
    /no longer declares MAX_UI_SUBSCRIPTIONS_PER_MOUNT/,
  );
});
