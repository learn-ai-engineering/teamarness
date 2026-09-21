import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { promisify } from "node:util";
import { test } from "node:test";

import {
  checkLoopbackTests,
  findProductionBindings,
  findProductionReachability,
  isCompileFailFixture,
  repositoryRoot,
} from "./check-loopback-tests.mjs";

const execFileAsync = promisify(execFile);

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

// The twelve ways the substring scan was fooled, each one an executed evasion
// from the round-3 verification. Eleven of them passed; they are the reason
// the guard now reads reachability rather than a spelling.

test("whitespace inside the path does not hide the binding", () => {
  for (const spelling of [
    "CoreRuntime :: open(config)",
    "CoreRuntime::open (config)",
    "CoreRuntime\n        ::open(config)",
  ]) {
    const source = `#[tokio::test]\nasync fn a_thing() {\n    let runtime = ${spelling}.await;\n}\n`;
    assert.equal(
      findProductionBindings("crates/x/tests/a.rs", source).length,
      1,
      spelling,
    );
  }
});

test("`Self::open` inside the runtime's own impl block is the same call", () => {
  const source = `impl CoreRuntime {
    #[cfg(test)]
    #[tokio::test]
    async fn a_thing() {
        let runtime = Self::open(config).await;
    }
}
`;
  assert.equal(findProductionBindings("crates/x/src/runtime.rs", source).length, 1);
});

test("`Self::open` elsewhere is somebody else's method", () => {
  const source = `impl Storage {
    #[cfg(test)]
    fn a_thing() {
        let handle = Self::open(path);
    }
}
`;
  assert.deepEqual(findProductionBindings("crates/x/src/storage.rs", source), []);
});

test("a feature-gated or commented test cfg is still a test module", () => {
  for (const attribute of ['#[cfg(all(test, feature = "hooks"))]', "#[cfg(test)] // unit tests"]) {
    const source = `${attribute}\nmod tests {\n    fn a_thing() {\n        CoreRuntime::open(config);\n    }\n}\n`;
    assert.equal(findProductionBindings("crates/x/src/lib.rs", source).length, 1, attribute);
  }
});

test("prose about the call in a comment is not a call", () => {
  const source = `#[tokio::test]
async fn a_thing() {
    // Never use CoreRuntime::open( here.
    let runtime = CoreRuntime::open_loopback_for_tests(config).await;
}
`;
  assert.deepEqual(findProductionBindings("crates/x/tests/a.rs", source), []);
});

test("the marker is no pass for a helper that dispatches on nothing", () => {
  const source = `pub async fn open_anything(path: &Path) {
    // production-addressing: convenient.
    CoreRuntime::open(config).await
}
`;
  assert.equal(findProductionBindings("tests/system/tests/support/other.rs", source).length, 1);
});

test("a test that reaches the seam without #[ignore] fails, whatever it spells", () => {
  const harness = `pub async fn open_core(addressing: Addressing, path: &Path) {
    match addressing {
        // production-addressing: the one seam that binds public relays.
        Addressing::ProductionRelay => CoreRuntime::open(config).await,
        Addressing::Loopback => CoreRuntime::open_loopback_for_tests(config).await,
    }
}
`;
  const scenario = `#[tokio::test]
async fn a_relay_scenario() {
    let core = open_core(Addressing::ProductionRelay, &path).await;
}
`;
  const problems = findProductionReachability([
    ["tests/system/tests/support/harness.rs", harness],
    ["tests/system/tests/a.rs", scenario],
  ]);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /a_relay_scenario/);
});

test("the same reach through a local helper still fails", () => {
  // The test names no addressing itself; the helper does. Reading only the
  // test body would miss it, which is the shape the round-3 evasion used.
  const scenario = `async fn production_matrix() {
    let core = open_core(Addressing::ProductionRelay, &path).await;
}

#[tokio::test]
async fn a_relay_scenario() {
    production_matrix().await;
}
`;
  const problems = findProductionReachability([["tests/system/tests/a.rs", scenario]]);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /a_relay_scenario/);
});

test("an ignored scenario may reach production addressing", () => {
  const scenario = `#[tokio::test]
#[ignore = "needs the public internet"]
async fn a_relay_scenario() {
    let core = open_core(Addressing::ProductionRelay, &path).await;
}

#[tokio::test]
async fn the_hermetic_twin() {
    let core = open_core(Addressing::Loopback, &path).await;
}
`;
  assert.deepEqual(findProductionReachability([["tests/system/tests/a.rs", scenario]]), []);
});

// The composed guard, exercised the way the repository runs it: from `git
// ls-files` over a real tree. The one problem the fixture holds is one only
// the reachability walk can find — a non-ignored `#[tokio::test]` in a file
// that never spells addressing, calling a tainted helper declared in a file
// that does — so the test fails if the walk loses any file it needs: the
// corpus filter that hid the scenario's file, or the wiring that feeds the
// walk its sources at all. Both were executed evasions.
test("the composed check finds a reach whose caller names no addressing", async () => {
  const root = await mkdtemp(join(tmpdir(), "loopback-guard-"));
  await execFileAsync("git", ["init", "-q"], { cwd: root });
  const harness = `pub async fn open_core(addressing: Addressing, path: &Path) {
    match addressing {
        // production-addressing: the one seam that binds public relays.
        Addressing::ProductionRelay => CoreRuntime::open(config).await,
        Addressing::Loopback => CoreRuntime::open_loopback_for_tests(config).await,
    }
}

pub async fn production_matrix(path: &Path) {
    let core = open_core(Addressing::ProductionRelay, path).await;
}
`;
  // No `Addressing` spelling anywhere in this file: under the corpus filter
  // that once selected the reachability inputs, nothing here was ever read.
  const scenario = `#[tokio::test]
async fn a_relay_scenario() {
    let core = production_matrix(&path).await;
}
`;
  for (const [file, text] of [
    ["tests/system/tests/support/harness.rs", harness],
    ["tests/system/tests/a.rs", scenario],
  ]) {
    const target = join(root, file);
    await mkdir(dirname(target), { recursive: true });
    await writeFile(target, text);
  }
  await execFileAsync("git", ["add", "."], { cwd: root });

  const problems = await checkLoopbackTests(root);
  assert.equal(problems.length, 1, JSON.stringify(problems, null, 2));
  assert.match(problems[0], /a_relay_scenario/);
});
