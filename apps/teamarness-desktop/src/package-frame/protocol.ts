import {
  TEAMARNESS_THEME_TOKENS,
  type PackagePresentation,
  type PackageValue,
  type TeamarnessThemeToken,
  type WorkspacePresentation,
} from "@teamarness/package-ui-sdk";

/** The private Package UI bridge protocol. Both halves ship in one Shell build. */
export const FRAME_BRIDGE_PROTOCOL = 2;
export const CONNECT_KIND = "teamarness.bridge.connect";

/** Matches the Package Host bound `MAX_UI_PAYLOAD_BYTES`. */
export const MAX_PAYLOAD_BYTES = 256 * 1024;
/** Matches the Package Host bound `MAX_SCHEMA_DEPTH`. */
export const MAX_VALUE_DEPTH = 64;
export const MAX_OPERATION_LENGTH = 256;
export const MAX_PENDING_REQUESTS = 64;
/**
 * Matches the Package Host bound `MAX_UI_SUBSCRIPTIONS_PER_MOUNT`.
 *
 * The Host refuses the ninth open subscription on one mount as `overloaded`,
 * so a Shell that admitted more would accept subscriptions the Host then
 * rejects. `check-protocol-bounds.mjs` reads both so neither can move alone.
 */
export const MAX_OPEN_SUBSCRIPTIONS = 8;

/**
 * Events the frame client acknowledges with one `ack`.
 *
 * `port-client.ts` posts one `ack` for every event it hands its listener, so a
 * subscription at a full credit window makes the Package Host owe a whole
 * window of `credit` frames at once — not one. That number is what the Host's
 * control lane must be sized against, and until this constant existed it lived
 * only in this file, in another language, with nothing in the Host naming it.
 * The Host mirrors it as `EVENTS_PER_ACKNOWLEDGEMENT` in `protocol.rs` and
 * `check-protocol-bounds.mjs` reads both, so a Shell that started batching
 * acknowledgements has to move the Host's worst case with it.
 * `port-client.test.ts` asserts the client really acknowledges at this
 * granularity.
 */
export const EVENTS_PER_ACKNOWLEDGEMENT = 1;

/**
 * Ids the Shell keeps after it stops serving them.
 *
 * Requests and subscriptions share one id space, so they share one tombstone
 * set. A stream ends, or a request is answered, while the frame's own
 * acknowledgement, `unsubscribe` or `cancel` for it is still queued, so that
 * message names an entry this side has already released. An id that was never
 * issued has no tombstone and stays a fault.
 *
 * The window is one round trip, so the most ids that can need a tombstone at
 * once is every pending request plus every open subscription: the set is
 * exactly that, derived rather than chosen, because an id evicted from it is a
 * fault again.
 */
export const MAX_RETIRED_IDS = MAX_PENDING_REQUESTS + MAX_OPEN_SUBSCRIPTIONS;

const THEME_VALUE = /^[^;{}<>\\\r\n]{0,256}$/;
const PACKAGE_ERROR_CODE = /^[a-z][a-z0-9_.-]{0,127}$/;
/** Matches the Package Host bound `MAX_PACKAGE_ERROR_DETAILS_BYTES`. */
export const MAX_PACKAGE_ERROR_DETAILS_BYTES = 4096;
const DECIMAL_SEQUENCE = /^(?:0|[1-9][0-9]{0,38})$/;
const FAULT_SOURCES = ["load", "mount", "unmount", "listener", "error", "unhandledrejection"] as const;

export type FrameFaultSource = (typeof FAULT_SOURCES)[number];

export type ThemeSnapshot = Readonly<{
  colorScheme: "light" | "dark";
  values: Readonly<Record<TeamarnessThemeToken, string>>;
}>;

export type BridgePackageError = Readonly<{
  code: string;
  retryable: boolean;
  details: { readonly [key: string]: PackageValue };
}>;

export type BridgeErrorEnvelope = Readonly<{
  version: number;
  namespace: string;
  code: string;
  stage: string;
  affectedPackage: string | null;
  affectedInstance: Readonly<{ packageIdentity: string; workspaceId: string }> | null;
  retryable: boolean;
  recovery: string;
  diagnosticId: string;
  /** The Package Backend's own error, or `null` when the host owns the failure. */
  packageError: BridgePackageError | null;
}>;

export type FrameToShellMessage =
  | Readonly<{ kind: "hello"; protocol: number }>
  | Readonly<{ kind: "mounted" }>
  | Readonly<{ kind: "request"; id: number; operation: string; payload: unknown }>
  | Readonly<{ kind: "cancel"; id: number }>
  | Readonly<{ kind: "subscribe"; id: number; operation: string; payload: unknown }>
  | Readonly<{ kind: "ack"; id: number; sequence: string }>
  | Readonly<{ kind: "unsubscribe"; id: number }>
  | Readonly<{ kind: "fault"; source: FrameFaultSource }>
  | Readonly<{ kind: "unmounted"; ok: boolean }>;

export type ShellToFrameMessage =
  | Readonly<{
    kind: "mount";
    entrypoint: string;
    package: PackagePresentation;
    workspace: WorkspacePresentation;
    uiHostVersion: number;
    theme: ThemeSnapshot;
  }>
  | Readonly<{ kind: "theme"; theme: ThemeSnapshot }>
  | Readonly<{ kind: "response"; id: number; ok: true; value: PackageValue }>
  | Readonly<{ kind: "response"; id: number; ok: false; error: BridgeErrorEnvelope }>
  | Readonly<{ kind: "subscribed"; id: number }>
  | Readonly<{ kind: "subscribe_failed"; id: number; error: BridgeErrorEnvelope }>
  | Readonly<{ kind: "unsubscribed"; id: number }>
  | Readonly<{ kind: "event"; id: number; sequence: string; payload: PackageValue }>
  | Readonly<{ kind: "stream_terminated"; id: number; error: BridgeErrorEnvelope | null }>
  | Readonly<{ kind: "unmount" }>;

function isPlainObject(value: unknown): value is Record<string, unknown> {
  if (typeof value !== "object" || value === null) return false;
  const prototype = Object.getPrototypeOf(value) as unknown;
  return prototype === Object.prototype || prototype === null;
}

function hasExactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  const own = Object.keys(value);
  if (own.length !== keys.length) return false;
  return keys.every((key) => Object.hasOwn(value, key));
}

function isMessageId(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0;
}

function isOperationName(value: unknown): value is string {
  return typeof value === "string";
}

/**
 * A structured value the Package Host accepts: JSON shapes only, bounded in depth.
 * Cycles are rejected because they always exceed the depth bound.
 */
export function isPackageValue(value: unknown, depth = 0): value is PackageValue {
  if (depth > MAX_VALUE_DEPTH) return false;
  if (value === null) return true;
  switch (typeof value) {
    case "boolean":
    case "string":
      return true;
    case "number":
      return Number.isFinite(value);
    case "object":
      break;
    default:
      return false;
  }
  if (Array.isArray(value)) return value.every((entry) => isPackageValue(entry, depth + 1));
  if (!isPlainObject(value)) return false;
  return Object.values(value).every((entry) => isPackageValue(entry, depth + 1));
}

/** The UTF-8 length of the value's JSON text, which is what the Package Host bounds. */
export function packageValueByteLength(value: PackageValue): number {
  const text = JSON.stringify(value);
  if (text === undefined) return 0;
  let bytes = 0;
  for (let index = 0; index < text.length; index += 1) {
    const point = text.codePointAt(index) ?? 0;
    if (point > 0xffff) index += 1;
    if (point < 0x80) bytes += 1;
    else if (point < 0x800) bytes += 2;
    else if (point < 0x10000) bytes += 3;
    else bytes += 4;
  }
  return bytes;
}

export function isThemeValue(value: unknown): value is string {
  return typeof value === "string" && THEME_VALUE.test(value);
}

function parseThemeSnapshot(value: unknown): ThemeSnapshot | null {
  if (!isPlainObject(value) || !hasExactKeys(value, ["colorScheme", "values"])) return null;
  const { colorScheme, values } = value;
  if (colorScheme !== "light" && colorScheme !== "dark") return null;
  if (!isPlainObject(values) || !hasExactKeys(values, [...TEAMARNESS_THEME_TOKENS])) return null;
  const rebuilt: Record<string, string> = {};
  for (const token of TEAMARNESS_THEME_TOKENS) {
    const entry = values[token];
    if (!isThemeValue(entry)) return null;
    rebuilt[token] = entry;
  }
  return Object.freeze({
    colorScheme,
    values: Object.freeze(rebuilt) as Readonly<Record<TeamarnessThemeToken, string>>,
  });
}

/** Rebuilds a Package-owned error within the host's bounds, or `null` when it is not one. */
function parsePackageError(value: unknown): BridgePackageError | null {
  if (typeof value !== "object" || value === null) return null;
  const { code, retryable, details } = value as Record<string, unknown>;
  if (typeof code !== "string" || !PACKAGE_ERROR_CODE.test(code)) return null;
  if (typeof retryable !== "boolean") return null;
  if (!isPlainObject(details) || !isPackageValue(details)) return null;
  if (packageValueByteLength(details) > MAX_PACKAGE_ERROR_DETAILS_BYTES) return null;
  const rebuilt: Record<string, PackageValue> = {};
  for (const [key, entry] of Object.entries(details)) rebuilt[key] = entry as PackageValue;
  return Object.freeze({ code, retryable, details: Object.freeze(rebuilt) });
}

/** Rebuilds an error envelope field by field so no foreign prototype or extra field crosses. */
export function parseBridgeErrorEnvelope(value: unknown): BridgeErrorEnvelope | null {
  if (typeof value !== "object" || value === null) return null;
  const {
    version, namespace, code, stage, affectedPackage, affectedInstance, retryable, recovery,
    diagnosticId, packageError,
  } = value as Record<string, unknown>;
  if (typeof version !== "number" || !Number.isSafeInteger(version)) return null;
  if (typeof namespace !== "string" || typeof code !== "string" || typeof stage !== "string") return null;
  if (typeof retryable !== "boolean" || typeof recovery !== "string" || typeof diagnosticId !== "string") return null;
  if (affectedPackage !== null && typeof affectedPackage !== "string") return null;
  let instance: Readonly<{ packageIdentity: string; workspaceId: string }> | null = null;
  if (affectedInstance !== null && affectedInstance !== undefined) {
    if (typeof affectedInstance !== "object") return null;
    const { packageIdentity, workspaceId } = affectedInstance as Record<string, unknown>;
    if (typeof packageIdentity !== "string" || typeof workspaceId !== "string") return null;
    instance = Object.freeze({ packageIdentity, workspaceId });
  }
  let owned: BridgePackageError | null = null;
  if (packageError !== null && packageError !== undefined) {
    owned = parsePackageError(packageError);
    if (owned === null) return null;
  }
  return Object.freeze({
    version,
    namespace,
    code,
    stage,
    affectedPackage: affectedPackage ?? null,
    affectedInstance: instance,
    retryable,
    recovery,
    diagnosticId,
    packageError: owned,
  });
}

export function parseFrameMessage(data: unknown): FrameToShellMessage | null {
  if (!isPlainObject(data) || typeof data.kind !== "string") return null;
  switch (data.kind) {
    case "hello":
      if (!hasExactKeys(data, ["kind", "protocol"])) return null;
      if (data.protocol !== FRAME_BRIDGE_PROTOCOL) return null;
      return Object.freeze({ kind: "hello", protocol: FRAME_BRIDGE_PROTOCOL });
    case "mounted":
      if (!hasExactKeys(data, ["kind"])) return null;
      return Object.freeze({ kind: "mounted" });
    case "request":
    case "subscribe": {
      if (!hasExactKeys(data, ["kind", "id", "operation", "payload"])) return null;
      if (!isMessageId(data.id) || !isOperationName(data.operation)) return null;
      return Object.freeze({ kind: data.kind, id: data.id, operation: data.operation, payload: data.payload });
    }
    case "cancel":
    case "unsubscribe": {
      if (!hasExactKeys(data, ["kind", "id"])) return null;
      if (!isMessageId(data.id)) return null;
      return Object.freeze({ kind: data.kind, id: data.id });
    }
    case "ack": {
      if (!hasExactKeys(data, ["kind", "id", "sequence"])) return null;
      if (!isMessageId(data.id)) return null;
      if (typeof data.sequence !== "string" || !DECIMAL_SEQUENCE.test(data.sequence)) return null;
      return Object.freeze({ kind: "ack", id: data.id, sequence: data.sequence });
    }
    case "fault": {
      if (!hasExactKeys(data, ["kind", "source"])) return null;
      const source = FAULT_SOURCES.find((candidate) => candidate === data.source);
      if (source === undefined) return null;
      return Object.freeze({ kind: "fault", source });
    }
    case "unmounted": {
      if (!hasExactKeys(data, ["kind", "ok"])) return null;
      if (typeof data.ok !== "boolean") return null;
      return Object.freeze({ kind: "unmounted", ok: data.ok });
    }
    default:
      return null;
  }
}

function parsePresentation(value: unknown): PackagePresentation | null {
  if (!isPlainObject(value)) return null;
  const keys = ["identity", "version", "displayName", "source", "trust", "lifecycle", "processGeneration"];
  if (!hasExactKeys(value, keys)) return null;
  const { identity, version, displayName, source, trust, lifecycle, processGeneration } = value;
  if (typeof identity !== "string" || typeof version !== "string" || typeof displayName !== "string") return null;
  if (source !== "bundled" && source !== "local") return null;
  if (trust !== "application_bundled" && trust !== "trusted_native_code_v1") return null;
  if (lifecycle !== "active") return null;
  if (typeof processGeneration !== "string" || !DECIMAL_SEQUENCE.test(processGeneration)) return null;
  return Object.freeze({ identity, version, displayName, source, trust, lifecycle, processGeneration });
}

function parseWorkspace(value: unknown): WorkspacePresentation | null {
  if (!isPlainObject(value) || !hasExactKeys(value, ["id", "name"])) return null;
  const { id, name } = value;
  if (typeof id !== "string" || typeof name !== "string") return null;
  return Object.freeze({ id, name });
}

export function parseShellMessage(data: unknown): ShellToFrameMessage | null {
  if (!isPlainObject(data) || typeof data.kind !== "string") return null;
  switch (data.kind) {
    case "mount": {
      const keys = ["kind", "entrypoint", "package", "workspace", "uiHostVersion", "theme"];
      if (!hasExactKeys(data, keys)) return null;
      if (typeof data.entrypoint !== "string" || data.entrypoint.length === 0) return null;
      if (typeof data.uiHostVersion !== "number" || !Number.isSafeInteger(data.uiHostVersion)) return null;
      const presentation = parsePresentation(data.package);
      const workspace = parseWorkspace(data.workspace);
      const theme = parseThemeSnapshot(data.theme);
      if (presentation === null || workspace === null || theme === null) return null;
      return Object.freeze({
        kind: "mount",
        entrypoint: data.entrypoint,
        package: presentation,
        workspace,
        uiHostVersion: data.uiHostVersion,
        theme,
      });
    }
    case "theme": {
      if (!hasExactKeys(data, ["kind", "theme"])) return null;
      const theme = parseThemeSnapshot(data.theme);
      if (theme === null) return null;
      return Object.freeze({ kind: "theme", theme });
    }
    case "response": {
      if (!isMessageId(data.id)) return null;
      if (data.ok === true) {
        if (!hasExactKeys(data, ["kind", "id", "ok", "value"])) return null;
        if (!isPackageValue(data.value)) return null;
        return Object.freeze({ kind: "response", id: data.id, ok: true, value: data.value });
      }
      if (data.ok === false) {
        if (!hasExactKeys(data, ["kind", "id", "ok", "error"])) return null;
        const error = parseBridgeErrorEnvelope(data.error);
        if (error === null) return null;
        return Object.freeze({ kind: "response", id: data.id, ok: false, error });
      }
      return null;
    }
    case "subscribed":
    case "unsubscribed": {
      if (!hasExactKeys(data, ["kind", "id"])) return null;
      if (!isMessageId(data.id)) return null;
      return Object.freeze({ kind: data.kind, id: data.id });
    }
    case "stream_terminated": {
      if (!hasExactKeys(data, ["kind", "id", "error"])) return null;
      if (!isMessageId(data.id)) return null;
      if (data.error === null) return Object.freeze({ kind: "stream_terminated", id: data.id, error: null });
      const error = parseBridgeErrorEnvelope(data.error);
      if (error === null) return null;
      return Object.freeze({ kind: "stream_terminated", id: data.id, error });
    }
    case "subscribe_failed": {
      if (!hasExactKeys(data, ["kind", "id", "error"])) return null;
      if (!isMessageId(data.id)) return null;
      const error = parseBridgeErrorEnvelope(data.error);
      if (error === null) return null;
      return Object.freeze({ kind: "subscribe_failed", id: data.id, error });
    }
    case "event": {
      if (!hasExactKeys(data, ["kind", "id", "sequence", "payload"])) return null;
      if (!isMessageId(data.id)) return null;
      if (typeof data.sequence !== "string" || !DECIMAL_SEQUENCE.test(data.sequence)) return null;
      if (!isPackageValue(data.payload)) return null;
      return Object.freeze({ kind: "event", id: data.id, sequence: data.sequence, payload: data.payload });
    }
    case "unmount":
      if (!hasExactKeys(data, ["kind"])) return null;
      return Object.freeze({ kind: "unmount" });
    default:
      return null;
  }
}
