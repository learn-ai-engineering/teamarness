import assert from "node:assert/strict";
import { test } from "node:test";

import {
  checkProtocolBounds,
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
