import assert from "node:assert/strict";
import { test } from "node:test";

import {
  checkTestSleeps,
  findBareSleeps,
  isTestFile,
  repositoryRoot,
} from "./check-test-sleeps.mjs";

test("every sleep in the repository's tests is anchored or explained", async () => {
  assert.deepEqual(await checkTestSleeps(repositoryRoot), []);
});

test("recognizes test files wherever they live", () => {
  assert.equal(isTestFile("crates/x/tests/a.rs"), true);
  assert.equal(isTestFile("packages/x/backend/tests/support/harness.rs"), true);
  assert.equal(isTestFile("tests/system/tests/a.rs"), true);
  assert.equal(isTestFile("crates/x/src/onboarding/tests/a.rs"), true);
  assert.equal(isTestFile("crates/x/src/runtime/runtime_tests/a.rs"), true);
  assert.equal(isTestFile("crates/x/src/runtime/mod.rs"), false);
  assert.equal(isTestFile("scripts/tests/a.mjs"), false);
});

test("a bare sleep before an assertion fails", () => {
  const source = `#[tokio::test]
async fn a_thing_happens() {
    start().await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(happened());
}
`;
  const problems = findBareSleeps("crates/x/tests/a.rs", source);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /crates\/x\/tests\/a\.rs:4/);
});

test("a sleep inside a waiting helper passes", () => {
  const source = `async fn eventually(condition: impl Fn() -> bool) {
    while !condition() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_marker() {
    tokio::time::sleep(Duration::from_millis(10)).await;
}

async fn poll_until() {
    tokio::time::sleep(Duration::from_millis(10)).await;
}
`;
  assert.deepEqual(findBareSleeps("crates/x/tests/a.rs", source), []);
});

test("each marker excuses the sleep beside it", () => {
  for (const marker of ["poll-interval", "negative-wait", "settle", "simulated-delay"]) {
    const source = `#[tokio::test]
async fn a_thing() {
    // ${marker}: because of a stated reason.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(true);
}
`;
    assert.deepEqual(findBareSleeps("crates/x/tests/a.rs", source), [], marker);
  }
});

test("an unrelated comment does not excuse a sleep", () => {
  const source = `#[tokio::test]
async fn a_thing() {
    // give it a moment
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(true);
}
`;
  assert.equal(findBareSleeps("crates/x/tests/a.rs", source).length, 1);
});
