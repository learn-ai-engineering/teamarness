import assert from "node:assert/strict";
import { test } from "node:test";

import { findLoosePins, findLoosePinsInManifest, repositoryRoot } from "./check-exact-pins.mjs";

const MANIFEST = `[package]
name = "example"
version = "0.1.0"
rust-version = "1.96"

[workspace.dependencies]
loose_string = "0.3"
loose_table = { version = "0.87", default-features = false }
pinned_string = "=1.0.0"
pinned_table = { version = "=2.1.0", features = ["std"] }
local = { path = "../other" }
inherited = { workspace = true, features = ["derive"] }
short.workspace = true
commented = "=1.2.3" # version = "9.9"

[target.'cfg(unix)'.dependencies]
platform = { version = "0.31", default-features = false }

[dev-dependencies]
harness = "0.2"

[dependencies.blocked]
version = "0.9"
default-features = false
`;

test("every tracked Cargo.toml pins its dependencies exactly", async () => {
  assert.deepEqual(await findLoosePins(repositoryRoot), []);
});

test("a loose string version is reported with its file, line, name and version", () => {
  const problems = findLoosePinsInManifest("Cargo.toml", MANIFEST);
  assert.ok(problems.includes("Cargo.toml:7 loose_string 0.3"), problems.join("\n"));
});

test("a loose table version is reported", () => {
  const problems = findLoosePinsInManifest("Cargo.toml", MANIFEST);
  assert.ok(problems.includes("Cargo.toml:8 loose_table 0.87"), problems.join("\n"));
});

test("exact pins, path and workspace dependencies are exempt", () => {
  const problems = findLoosePinsInManifest("Cargo.toml", MANIFEST).join("\n");
  for (const exempt of ["pinned_string", "pinned_table", "local", "inherited", "short", "commented"]) {
    assert.doesNotMatch(problems, new RegExp(` ${exempt} `), `${exempt} should be exempt`);
  }
});

test("the package version and rust-version are not dependency versions", () => {
  const problems = findLoosePinsInManifest("Cargo.toml", MANIFEST).join("\n");
  assert.doesNotMatch(problems, /rust-version/);
  assert.doesNotMatch(problems, /Cargo\.toml:3 /);
});

test("platform, dev and [dependencies.name] tables are all scanned", () => {
  const problems = findLoosePinsInManifest("Cargo.toml", MANIFEST);
  assert.ok(problems.includes("Cargo.toml:17 platform 0.31"), problems.join("\n"));
  assert.ok(problems.includes("Cargo.toml:20 harness 0.2"), problems.join("\n"));
  assert.ok(problems.includes("Cargo.toml:23 blocked 0.9"), problems.join("\n"));
});

test("the fixture reports exactly the five loose pins", () => {
  assert.equal(findLoosePinsInManifest("Cargo.toml", MANIFEST).length, 5);
});
