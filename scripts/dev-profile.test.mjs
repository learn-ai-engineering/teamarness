import assert from "node:assert/strict";
import { mkdtemp, rm, stat } from "node:fs/promises";
import { test } from "node:test";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  buildDevelopmentCommand,
  buildInstanceCommand,
  ensurePrivateDirectory,
  parseLauncherArguments,
  requireDebugExecutable,
  resolveDevelopmentDataDir,
} from "./dev-profile.mjs";

const repositoryRoot = "/repo/teamarness";

test("accepts one supported launcher mode and one profile argument", () => {
  assert.deepEqual(parseLauncherArguments(["dev", "alice"]), {
    mode: "dev",
    value: "alice",
  });
  assert.deepEqual(parseLauncherArguments(["instance", "bob"]), {
    mode: "instance",
    value: "bob",
  });
  assert.throws(() => parseLauncherArguments([]), /Usage/);
  assert.throws(() => parseLauncherArguments(["build", "alice"]), /Usage/);
  assert.throws(() => parseLauncherArguments(["dev", "alice", "bob"]), /Usage/);
});

test("resolves a safe profile beneath the repository development-data root", () => {
  assert.equal(
    resolveDevelopmentDataDir("alice", repositoryRoot),
    join(repositoryRoot, ".teamarness-dev", "alice"),
  );
  assert.equal(
    resolveDevelopmentDataDir("team_2-local", repositoryRoot),
    join(repositoryRoot, ".teamarness-dev", "team_2-local"),
  );
});

test("accepts an absolute path as an expert override", () => {
  assert.equal(
    resolveDevelopmentDataDir("/private/tmp/teamarness alice", repositoryRoot),
    "/private/tmp/teamarness alice",
  );
});

test("rejects missing, traversal, separators, and unsafe profile names", () => {
  for (const value of [
    undefined,
    "",
    ".",
    "..",
    "../alice",
    "alice/bob",
    "alice\\bob",
    ".hidden",
    "alice profile",
    "alice.$HOME",
    "a".repeat(65),
  ]) {
    assert.throws(
      () => resolveDevelopmentDataDir(value, repositoryRoot),
      /profile name|Profile names/,
    );
  }
});

test("creates the selected data directory with private permissions", async () => {
  const root = await mkdtemp(join(tmpdir(), "teamarness-dev-profile-"));
  const dataDir = join(root, "profiles", "alice");
  try {
    await ensurePrivateDirectory(dataDir);
    const details = await stat(dataDir);
    assert.equal(details.isDirectory(), true);
    if (process.platform !== "win32") {
      assert.equal(details.mode & 0o777, 0o700);
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("constructs the Tauri development command with runner and app separators", () => {
  assert.deepEqual(buildDevelopmentCommand("/data/alice", "darwin"), {
    command: "npm",
    args: [
      "--workspace",
      "teamarness-desktop",
      "run",
      "tauri",
      "--",
      "dev",
      "--",
      "--",
      "--data-dir",
      "/data/alice",
    ],
  });
  assert.equal(buildDevelopmentCommand("C:\\data\\alice", "win32").command, "npm.cmd");
});

test("constructs the platform debug executable command without a shell", () => {
  assert.deepEqual(buildInstanceCommand("/data/bob", repositoryRoot, "darwin"), {
    command: join(repositoryRoot, "target", "debug", "teamarness-desktop"),
    args: ["--data-dir", "/data/bob"],
  });
  assert.equal(
    buildInstanceCommand("C:\\data\\bob", repositoryRoot, "win32").command,
    join(repositoryRoot, "target", "debug", "teamarness-desktop.exe"),
  );
});

test("explains how to build a missing debug executable", async () => {
  const root = await mkdtemp(join(tmpdir(), "teamarness-missing-binary-"));
  try {
    await assert.rejects(
      requireDebugExecutable(join(root, "teamarness-desktop")),
      /Run "npm run dev -- <profile>" once to build it/,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
