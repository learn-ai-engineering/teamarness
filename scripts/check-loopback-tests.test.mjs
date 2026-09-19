import assert from "node:assert/strict";
import { test } from "node:test";

import {
  checkLoopbackTests,
  findProductionBindings,
  isCompileFailFixture,
  repositoryRoot,
} from "./check-loopback-tests.mjs";

test("every test Installation in the repository binds loopback addressing", async () => {
  assert.deepEqual(await checkLoopbackTests(repositoryRoot), []);
});

test("a production binding in an integration test fails", () => {
  const source = `#[tokio::test]
async fn an_installation_opens() {
    let runtime = CoreRuntime::open(CoreRuntimeConfig::new(&directory)).await.unwrap();
}
`;
  const problems = findProductionBindings("crates/x/tests/a.rs", source);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /crates\/x\/tests\/a\.rs:3/);
});

test("a production binding inside a cfg(test) module fails", () => {
  const source = `pub async fn open_runtime() {
    CoreRuntime::open(config).await
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn a_thing() {
        let runtime = CoreRuntime::open(config).await.unwrap();
    }
}
`;
  const problems = findProductionBindings("crates/x/src/lib.rs", source);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /crates\/x\/src\/lib\.rs:9/);
});

test("the loopback binding passes", () => {
  const source = `#[tokio::test]
async fn an_installation_opens() {
    let runtime = CoreRuntime::open_loopback_for_tests(config).await.unwrap();
}
`;
  assert.deepEqual(findProductionBindings("crates/x/tests/a.rs", source), []);
});

test("an ignored scenario may bind the production way", () => {
  const source = `/// Qualifies relay-mediated addressing.
#[tokio::test]
#[ignore = "needs the public internet"]
async fn a_relay_scenario() {
    let runtime = CoreRuntime::open(config).await.unwrap();
}
`;
  assert.deepEqual(findProductionBindings("crates/x/tests/a.rs", source), []);
});

test("the marker excuses the one production-addressing seam", () => {
  const source = `pub async fn open_core(addressing: Addressing) {
    match addressing {
        // production-addressing: the one seam that binds public relays.
        Addressing::ProductionRelay => CoreRuntime::open(config).await,
    }
}
`;
  assert.deepEqual(findProductionBindings("tests/system/tests/support/harness.rs", source), []);
});

test("a compile-fail fixture is not a test that runs", () => {
  assert.equal(isCompileFailFixture("crates/teamarness-core/tests/ui/sqlite_connection.rs"), true);
  assert.equal(isCompileFailFixture("crates/teamarness-core/tests/migrations.rs"), false);
});
