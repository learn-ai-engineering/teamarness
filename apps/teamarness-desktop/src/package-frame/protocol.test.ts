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
      { kind: "request", id: 1, operation: "reference.read", payload: {} },
      { kind: "cancel", id: 2 },
      { kind: "subscribe", id: 3, operation: "reference.changes", payload: null },
      { kind: "ack", id: 3, sequence: "12" },
      { kind: "unsubscribe", id: 3 },
      { kind: "fault", source: "listener" },
      { kind: "unmounted", ok: true },
    ];
    expect(accepted.map((message) => parseFrameMessage(message)?.kind)).toEqual([
      "hello", "mounted", "request", "cancel", "subscribe", "ack", "unsubscribe", "fault", "unmounted",
    ]);

    for (const message of [
      { kind: CONNECT_KIND, protocol: 1 },
      { kind: "hello", protocol: 2 },
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
      processGeneration: 1,
    };
    const accepted: unknown[] = [
      {
        kind: "mount",
        entrypoint: "ui/index.js",
        package: presentation,
        workspace: { id: "ff".repeat(16), name: "Workspace" },
        uiHostVersion: 1,
        theme: themeSnapshot(),
      },
      { kind: "theme", theme: themeSnapshot() },
      { kind: "response", id: 1, ok: true, value: { a: 1 } },
      { kind: "response", id: 1, ok: false, error: envelope },
      { kind: "subscribed", id: 2 },
      { kind: "subscribe_failed", id: 2, error: envelope },
      { kind: "unsubscribed", id: 2 },
      { kind: "event", id: 2, sequence: "0", payload: [] },
      { kind: "stream_terminated", id: 2 },
      { kind: "unmount" },
    ];
    expect(accepted.map((message) => parseShellMessage(message)?.kind)).toEqual([
      "mount", "theme", "response", "response", "subscribed", "subscribe_failed",
      "unsubscribed", "event", "stream_terminated", "unmount",
    ]);

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

  it("rebuilds a bridge error envelope field by field and rejects partial envelopes", () => {
    const hostile = Object.assign(Object.create({ inherited: true }), envelope, { extra: "dropped" });
    const rebuilt = parseBridgeErrorEnvelope(hostile);
    expect(rebuilt).not.toBeNull();
    expect(Object.keys(rebuilt ?? {}).sort()).toEqual([
      "affectedInstance", "affectedPackage", "code", "diagnosticId",
      "namespace", "recovery", "retryable", "stage", "version",
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
