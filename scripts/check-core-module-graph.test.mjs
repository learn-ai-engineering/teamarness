import assert from "node:assert/strict";
import { test } from "node:test";

import {
  checkCoreModuleGraph,
  findPackageHostDependencies,
  findDemotedBehindTestHooks,
  findModuleGraphBreaks,
  findTestHooksMisuse,
  repositoryRoot,
} from "./check-core-module-graph.mjs";

test("the repository holds the core boundary rules", async () => {
  assert.deepEqual(await checkCoreModuleGraph(repositoryRoot), []);
});

test("a dev-dependency may enable test-hooks and the feature may declare itself", () => {
  const manifest = `[features]
test-hooks = []

[dependencies]
serde = "=1.0.0"

[dev-dependencies]
teamarness-core = { path = "../teamarness-core", features = ["test-hooks"] }
`;
  assert.deepEqual(findTestHooksMisuse("Cargo.toml", manifest), []);
});

test("a normal or build dependency may not enable test-hooks", () => {
  const manifest = `[dependencies]
teamarness-core = { path = "../teamarness-core", features = ["test-hooks"] }

[build-dependencies]
teamarness-core = { path = "../teamarness-core", features = ["test-hooks"] }
`;
  const problems = findTestHooksMisuse("Cargo.toml", manifest);
  assert.equal(problems.length, 2);
  assert.match(problems[0], /Cargo\.toml:2 .*\[dependencies\]/);
  assert.match(problems[1], /Cargo\.toml:5 .*\[build-dependencies\]/);
});

test("a platform dev-dependency table is still a dev-dependency table", () => {
  const manifest = `[target.'cfg(unix)'.dev-dependencies]
teamarness-core = { path = "../teamarness-core", features = ["test-hooks"] }
`;
  assert.deepEqual(findTestHooksMisuse("Cargo.toml", manifest), []);
});

test("an observation hook behind test-hooks is allowed", () => {
  const source = `#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn records_decoded_for_tests(workspace: WorkspaceId) -> usize {
    decoded(workspace)
}
`;
  assert.deepEqual(findDemotedBehindTestHooks("lib.rs", source), []);
});

test("a demoted method behind test-hooks is refused, even inside a gated block", () => {
  const source = `fn unrelated() {}

#[cfg(feature = "test-hooks")]
impl CoreRuntime {
    pub async fn replicate_records_to(&self) -> usize {
        0
    }
}

#[cfg(feature = "test-hooks")]
pub use runtime::insert_remote_record;
`;
  const problems = findDemotedBehindTestHooks("lib.rs", source);
  assert.equal(problems.length, 2);
  assert.match(problems[0], /lib\.rs:3 .*replicate_records_to/);
  assert.match(problems[1], /lib\.rs:10 .*insert_remote_record/);
});

test("a forbidden module name in production code is reported", () => {
  const source = `use crate::network::Network;
use crate::onboarding::transport::Transport;

pub(crate) fn bind() {}
`;
  const problems = findModuleGraphBreaks("network", "network/mod.rs", source, [
    "crate::onboarding",
    "crate::synchronization",
  ]);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /network\/mod\.rs:2 .*crate::onboarding/);
});

test("the same name inside the file's own test module is allowed", () => {
  const source = `use crate::network::Network;

#[cfg(test)]
mod tests {
    use crate::onboarding::transport::Transport;
}
`;
  assert.deepEqual(
    findModuleGraphBreaks("network", "network/mod.rs", source, ["crate::onboarding"]),
    [],
  );
});

test("a Package manifest may not depend on Core or the Package Host", () => {
  const manifest = `[dependencies]
teamarness-package-sdk = { path = "../../../crates/teamarness-package-sdk" }

[dev-dependencies]
teamarness-core = { path = "../../../crates/teamarness-core" }
teamarness-package-host = { path = "../../../crates/teamarness-package-host" }
`;
  const problems = findPackageHostDependencies("packages/x/backend/Cargo.toml", manifest);
  assert.equal(problems.length, 2);
  assert.match(problems[0], /Cargo\.toml:5 .*teamarness-core/);
  assert.match(problems[1], /Cargo\.toml:6 .*teamarness-package-host/);
});

test("the SDK and the protocol are not the forbidden crates", () => {
  const manifest = `[dependencies]
teamarness-package-sdk = { path = "../../../crates/teamarness-package-sdk" }
teamarness-package-protocol = { path = "../../../crates/teamarness-package-protocol" }
`;
  assert.deepEqual(findPackageHostDependencies("packages/x/backend/Cargo.toml", manifest), []);
});
