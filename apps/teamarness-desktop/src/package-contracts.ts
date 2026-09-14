import type { PackageValue } from "@teamarness/package-ui-sdk";

export type PackageInstanceKey = Readonly<{
  packageIdentity: string;
  workspaceId: string;
}>;

export type PackageCatalogEntry = Readonly<{
  identity: string;
  version: string;
  displayName: string;
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
  package: Readonly<{ identity: string; displayName: string }>;
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
}>;

export type PackageResponse = PackageValue;
