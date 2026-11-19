import assert from "node:assert/strict";
import test from "node:test";
import { forbiddenHeadlessDependencies } from "./check-headless-dependencies.mjs";

test("headless runtime guard rejects GUI frameworks but permits Core platform libraries", () => {
  assert.deepEqual(forbiddenHeadlessDependencies("teamarness-core v0.1.0\nnotify v8.2.1\nobjc2-foundation v0.3.1\n"), []);
  assert.deepEqual(forbiddenHeadlessDependencies("tauri v2.12.5\nwry v0.1.0\nwebkit2gtk v1.0.0\nobjc2-app-kit v0.3.1\ngtk v0.1.0\n"), ["tauri", "wry", "webkit2gtk", "objc2-app-kit", "gtk"]);
});
