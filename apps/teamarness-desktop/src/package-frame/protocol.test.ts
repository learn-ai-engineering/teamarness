import { describe, expect, it } from "vitest";
import { TEAMARNESS_THEME_TOKENS } from "@teamarness/package-ui-sdk";
import {
  CONNECT_KIND,
  FRAME_BRIDGE_PROTOCOL,
  MAX_PAYLOAD_BYTES,
  MAX_VALUE_DEPTH,
  isPackageValue,
  isThemeValue,
  packageValueByteLength,
  parseBridgeErrorEnvelope,
  parseFrameMessage,
  parseShellMessage,
  type BridgeErrorEnvelope,
  type ThemeSnapshot,
  MAX_FRAME_EXTENT_PX,
} from "./protocol";

function themeSnapshot(overrides: Record<string, string> = {}): ThemeSnapshot {
  const values: Record<string, string> = {};
  for (const token of TEAMARNESS_THEME_TOKENS) values[token] = "#101010";
  return { colorScheme: "dark", values: { ...values, ...overrides } } as ThemeSnapshot;
}

function nest(levels: number): unknown {
  let value: unknown = null;
  for (let index = 0; index < levels; index += 1) value = { a: value };
  return value;
}

function admits(value: unknown): boolean {
  return isPackageValue(value) && packageValueByteLength(value) <= MAX_PAYLOAD_BYTES;
}

const envelope: BridgeErrorEnvelope = {
  version: 1,
  namespace: "teamarness.package-host",
  code: "request_rejected",
  stage: "request",
  affectedPackage: "dev.teamarness.reference",
  affectedInstance: { packageIdentity: "dev.teamarness.reference", workspaceId: "ff".repeat(16) },
  retryable: false,
  recovery: "none",
  diagnosticId: "host-1",
  packageError: null,
};

describe("Package UI bridge protocol", () => {
  it("accepts only JSON-shaped Package values within the host's size and depth bounds", () => {
    for (const value of [null, true, 1.5, "text", [1, "two", null], { a: { b: [] } }, {}, []]) {
      expect(admits(value)).toBe(true);
    }
    const cyclic: Record<string, unknown> = {};
    cyclic.self = cyclic;
    for (const value of [
      Number.NaN,
      Number.POSITIVE_INFINITY,
      undefined,
      10n,
      new Map(),
      new Date(),
      new ArrayBuffer(8),
      new (class Holder { public readonly a = 1; })(),
      () => undefined,
      Symbol("x"),
      cyclic,
    ]) {
      expect(admits(value)).toBe(false);
    }
    expect(admits(nest(MAX_VALUE_DEPTH))).toBe(true);
    expect(admits(nest(MAX_VALUE_DEPTH + 1))).toBe(false);

    const oversized = "a".repeat(MAX_PAYLOAD_BYTES);
    expect(isPackageValue(oversized)).toBe(true);
    expect(packageValueByteLength(oversized)).toBeGreaterThan(MAX_PAYLOAD_BYTES);
    expect(admits(oversized)).toBe(false);
    expect(packageValueByteLength("é")).toBe(4);
    expect(packageValueByteLength("🌊")).toBe(6);
  });

  it("parses every frame message kind and rejects unknown kinds, extra keys and non-integer ids", () => {
    const accepted: unknown[] = [
      { kind: "hello", protocol: FRAME_BRIDGE_PROTOCOL },
      { kind: "mounted" },
      { kind: "open_settings" },
      { kind: "request", id: 1, operation: "reference.read", payload: {} },
      { kind: "cancel", id: 2 },
      { kind: "subscribe", id: 3, operation: "reference.changes", payload: null },
      { kind: "ack", id: 3, sequence: "12" },
      { kind: "unsubscribe", id: 3 },
      { kind: "fault", source: "listener" },
      { kind: "unmounted", ok: true },
    ];
    expect(accepted.map((message) => parseFrameMessage(message))).toEqual(accepted);

    for (const message of [
      { kind: CONNECT_KIND, protocol: FRAME_BRIDGE_PROTOCOL },
      { kind: "hello", protocol: FRAME_BRIDGE_PROTOCOL + 1 },
      { kind: "hello" },
      { kind: "mounted", extra: 1 },
      { kind: "request", id: 1.5, operation: "a", payload: {} },
      { kind: "request", id: 0, operation: "a", payload: {} },
      { kind: "request", id: -1, operation: "a", payload: {} },
      { kind: "request", id: "1", operation: "a", payload: {} },
      { kind: "request", id: 1, operation: 7, payload: {} },
      { kind: "request", id: 1, operation: "a", payload: {}, mount: "mount-1" },
      { kind: "request", id: 1, operation: "a" },
      { kind: "ack", id: 1, sequence: 12 },
      { kind: "ack", id: 1, sequence: "01" },
      { kind: "fault", source: "navigation" },
      { kind: "unmounted", ok: "yes" },
      { kind: "unknown", id: 1 },
      Object.assign(Object.create({ kind: "mounted" }), {}),
      null,
      "mounted",
      [],
    ]) {
      expect(parseFrameMessage(message), JSON.stringify(message) ?? "unserializable").toBeNull();
    }
  });

  it("parses Shell messages and rejects theme names outside the token list or unsafe values", () => {
    const presentation = {
      identity: "dev.teamarness.reference",
      version: "1.0.0",
      displayName: "Reference Package",
      source: "bundled",
      trust: "application_bundled",
      lifecycle: "active",
      processGeneration: "1",
    };
    const accepted: unknown[] = [
      {
        kind: "mount",
        entrypoint: "ui/index.js",
        package: presentation,
        workspace: { id: "ff".repeat(16), name: "Workspace" },
        view: "main",
        uiHostVersion: 3,
        openSettings: false,
        theme: themeSnapshot(),
      },
      { kind: "theme", theme: themeSnapshot() },
      { kind: "response", id: 1, ok: true, value: { a: 1 } },
      { kind: "response", id: 1, ok: false, error: envelope },
      { kind: "subscribed", id: 2 },
      { kind: "subscribe_failed", id: 2, error: envelope },
      { kind: "unsubscribed", id: 2 },
      { kind: "event", id: 2, sequence: "0", payload: [] },
      { kind: "stream_terminated", id: 2, error: null },
      { kind: "stream_terminated", id: 2, error: { ...envelope, packageError: { code: "fixture.rejected", retryable: false, details: { field: "value" } } } },
      { kind: "unmount" },
    ];
    expect(accepted.map((message) => parseShellMessage(message))).toEqual(accepted);

    const extraToken = { ...themeSnapshot(), values: { ...themeSnapshot().values, "--rogue": "red" } };
    const missingToken = { ...themeSnapshot(), values: { "--teamarness-color-text": "#fff" } };
    for (const theme of [
      extraToken,
      missingToken,
      themeSnapshot({ "--teamarness-color-accent": "red; } :root { display: none" }),
      themeSnapshot({ "--teamarness-font-body": "a{b}" }),
      themeSnapshot({ "--teamarness-color-text": "a\nb" }),
      themeSnapshot({ "--teamarness-color-text": "x".repeat(257) }),
      { colorScheme: "sepia", values: themeSnapshot().values },
      { values: themeSnapshot().values },
    ]) {
      expect(parseShellMessage({ kind: "theme", theme })).toBeNull();
    }
    expect(isThemeValue("#101010")).toBe(true);
    expect(isThemeValue("")).toBe(true);
    expect(isThemeValue("<script>")).toBe(false);

    expect(parseShellMessage({ kind: "response", id: 1, ok: true, value: undefined })).toBeNull();
    expect(parseShellMessage({ kind: "event", id: 1, sequence: "1", payload: undefined })).toBeNull();
    expect(parseShellMessage({ kind: "unmount", mount: "mount-1" })).toBeNull();
  });

  it("carries the mount view and rejects a mount without a known view", () => {
    const mount = {
      kind: "mount",
      entrypoint: "ui/index.js",
      package: {
        identity: "dev.teamarness.reference",
        version: "1.0.0",
        displayName: "Reference Package",
        source: "bundled",
        trust: "application_bundled",
        lifecycle: "active",
        processGeneration: "1",
      },
      workspace: { id: "ff".repeat(16), name: "Workspace" },
      view: "settings",
      uiHostVersion: 3,
      openSettings: false,
      theme: themeSnapshot(),
    };
    const parsed = parseShellMessage(mount);
    expect(parsed).toEqual(mount);
    expect(parseShellMessage({ ...mount, openSettings: "true" })).toBeNull();
    const withoutAvailability: Record<string, unknown> = { ...mount };
    delete withoutAvailability.openSettings;
    expect(parseShellMessage(withoutAvailability)).toBeNull();
    const withoutView: Record<string, unknown> = { ...mount };
    delete withoutView.view;
    expect(parseShellMessage(withoutView)).toBeNull();
    for (const view of ["preferences", "Settings", "", 1, null]) {
      expect(parseShellMessage({ ...mount, view })).toBeNull();
    }
  });

  it("rebuilds a bridge error envelope field by field and rejects partial envelopes", () => {
    const hostile = Object.assign(Object.create({ inherited: true }), envelope, { extra: "dropped" });
    const rebuilt = parseBridgeErrorEnvelope(hostile);
    expect(rebuilt).not.toBeNull();
    expect(Object.keys(rebuilt ?? {}).sort()).toEqual([
      "affectedInstance", "affectedPackage", "code", "diagnosticId",
      "namespace", "packageError", "recovery", "retryable", "stage", "version",
    ]);
    expect(rebuilt).toEqual(envelope);
    expect(Object.isFrozen(rebuilt)).toBe(true);
    expect(Object.getPrototypeOf(rebuilt)).toBe(Object.prototype);

    expect(parseBridgeErrorEnvelope({ ...envelope, affectedPackage: null })?.affectedPackage).toBeNull();
    expect(parseBridgeErrorEnvelope({ ...envelope, affectedInstance: null })?.affectedInstance).toBeNull();
    for (const partial of [
      { ...envelope, version: "1" },
      { ...envelope, code: 7 },
      { ...envelope, retryable: "no" },
      { ...envelope, affectedInstance: { packageIdentity: "a" } },
      { ...envelope, affectedPackage: 3 },
      { code: "x" },
      null,
      "error",
    ]) {
      expect(parseBridgeErrorEnvelope(partial)).toBeNull();
    }
  });
});

// Verifies: TEAM.SYS.SHELL-R05, TEAM.SYS.PKG-R19.
it("accepts only the exact target-free settings intent on the private bridge", () => {
  expect(FRAME_BRIDGE_PROTOCOL).toBe(5);
  expect(parseFrameMessage({ kind: "open_settings" })).toEqual({ kind: "open_settings" });
  for (const key of ["url", "route", "target", "workspaceId", "packageIdentity", "mount", "id"]) {
    expect(parseFrameMessage({ kind: "open_settings", [key]: "foreign" })).toBeNull();
  }
});

it("accepts only bounded whole-pixel settings sizing messages on private bridge 5", () => {
  expect(parseFrameMessage({ kind: "content_height", height: 0 })).toEqual({ kind: "content_height", height: 0 });
  expect(parseFrameMessage({ kind: "content_height", height: MAX_FRAME_EXTENT_PX })).toEqual({ kind: "content_height", height: MAX_FRAME_EXTENT_PX });
  for (const height of [-1, 1.5, MAX_FRAME_EXTENT_PX + 1, Number.NaN, "10", null]) {
    expect(parseFrameMessage({ kind: "content_height", height })).toBeNull();
  }
  expect(parseFrameMessage({ kind: "content_height", height: 10, mount: "m" })).toBeNull();
  expect(parseShellMessage({ kind: "visible_region", top: 40, height: 600 })).toEqual({ kind: "visible_region", top: 40, height: 600 });
  for (const region of [{ top: -1, height: 1 }, { top: 0, height: 0.5 }, { top: 0 }, { top: 0, height: 1, extra: 1 }]) {
    expect(parseShellMessage({ kind: "visible_region", ...region })).toBeNull();
  }
});
