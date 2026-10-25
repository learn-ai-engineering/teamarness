import type { PackageValue } from "@teamarness/package-ui-sdk";

export type PackageInstanceKey = Readonly<{
  packageIdentity: string;
  workspaceId: string;
}>;

export type PackageCatalogEntry = Readonly<{
  identity: string;
  version: string;
  displayName: string;
  description?: string;
  author?: string;
  homepage?: string;
  source: "bundled" | "local";
  trust: "application_bundled" | "trusted_native_code_v1";
  uiContractDigest: string;
}>;

export type PackageInstance = Readonly<{
  key: PackageInstanceKey;
  lifecycle: "admitted" | "activating" | "active" | "suspending" | "suspended" | "recovering" | "failed" | "crash_loop" | "disposing" | "disposed";
  processGeneration: number | null;
  mountState: "opening" | "mounted" | "failed" | "closing" | "closed" | null;
}>;

export type PackageHostSnapshot = Readonly<{
  revision: number;
  state: "opening" | "running" | "shutting_down" | "stopped" | "failed";
  packages: readonly PackageCatalogEntry[];
  instances: readonly PackageInstance[];
  diagnostics: readonly PackageBridgeError[];
}>;

export type PackageLoader = Readonly<{
  token: string;
  entrypoint: string;
  uiHostVersion: number;
  contractDigest: string;
  package: Readonly<{
    identity: string;
    version: string;
    displayName: string;
    source: "bundled" | "local";
    trust: "application_bundled" | "trusted_native_code_v1";
    lifecycle: "active";
    processGeneration: number;
  }>;
  workspace: Readonly<{ id: string; name: string }>;
}>;

export type OpenPackageMount = Readonly<{ mount: string; loader: PackageLoader }>;
export type PackageHostStateNotification = Readonly<{ revision: number; revisionGap: boolean }>;

export type PackageBridgeError = Readonly<{
  version: number;
  namespace: "teamarness.package-host";
  code: string;
  stage: string;
  affectedPackage: string | null;
  affectedInstance: PackageInstanceKey | null;
  retryable: boolean;
  recovery: string;
  diagnosticId: string;
  packageError: Readonly<{ code: string; retryable: boolean; details: PackageValue }> | null;
}>;

export type PackageResponse = PackageValue;

/** How one Package UI stream ended: with the Package's own error, or normally. */
export type HostStreamOutcome = Readonly<{ error: unknown | null }>;

export type HostUiEvent = Readonly<{
  mount: string;
  subscription: string;
  sequence: string;
  payload: PackageValue;
}>;

export type HostRequestInput = Readonly<{
  mount: string;
  request: string;
  operation: string;
  payload: PackageValue;
}>;

export type HostSubscriptionInput = Readonly<{
  mount: string;
  subscription: string;
  operation: string;
  payload: PackageValue;
}>;

/** The desktop-owned Tauri command surface the Package UI bridge calls on behalf of one mount. */
export interface PackageHostTransport {
  request(input: HostRequestInput): Promise<PackageValue>;
  cancelRequest(input: Readonly<{ mount: string; request: string }>): Promise<void>;
  openSubscription(
    input: HostSubscriptionInput,
    deliver: (event: HostUiEvent) => void,
    terminate: (outcome: HostStreamOutcome) => void,
  ): Promise<void>;
  acknowledgeEvents(input: Readonly<{ mount: string; subscription: string; throughSequence: string }>): Promise<void>;
  closeSubscription(input: Readonly<{ mount: string; subscription: string }>): Promise<void>;
  reportUiFault(input: Readonly<{ mount: string; code: "ui_exception" }>): Promise<void>;
}
