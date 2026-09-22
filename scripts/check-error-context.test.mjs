import assert from "node:assert/strict";
import { test } from "node:test";

import {
  checkErrorContext,
  compareCounts,
  countErasures,
  repositoryRoot,
} from "./check-error-context.mjs";

test("the repository holds its error-context baseline", async () => {
  assert.deepEqual(await checkErrorContext(repositoryRoot), []);
});

test("counts only the closures that throw the cause away", () => {
  const source = `let a = one().map_err(|_| StorageError::Database)?;
let b = two().map_err(fault!(StorageError::Database))?;
let c = three().map_err(|error| other(error))?;
let d = four().map_err(|_| StorageError::Io)?;
`;
  assert.equal(countErasures(source), 2);
});

test("a new erasure in a scoped file fails the ratchet", () => {
  const problems = compareCounts({ "a.rs": 3 }, { "a.rs": 4 });
  assert.equal(problems.length, 1);
  assert.match(problems[0], /above its baseline of 3/);
});

test("the same count passes", () => {
  assert.deepEqual(compareCounts({ "a.rs": 3 }, { "a.rs": 3 }), []);
});

test("a lowered count must be recorded in the same commit", () => {
  const problems = compareCounts({ "a.rs": 3 }, { "a.rs": 1 });
  assert.equal(problems.length, 1);
  assert.match(problems[0], /below its baseline of 3/);
});

test("a scoped file that vanished must lose its entry", () => {
  const problems = compareCounts({ "gone.rs": 0 }, {});
  assert.equal(problems.length, 1);
  assert.match(problems[0], /is not in the repository/);
});
