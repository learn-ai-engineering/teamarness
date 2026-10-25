import type { PackageValue } from "@teamarness/package-ui-sdk";

export type PackageInstanceKey = Readonly<{
  packageIdentity: string;
  workspaceId: string;
}>;

/** A Package UI view. Every Package has `main`; `settings` only when its manifest declares it. */
export type PackageUiView = "main" | "settings";

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
  /** The optional UI views the manifest declares beside `main`; absent when none are declared. */
  views?: readonly PackageUiView[];
}>;

export type PackageInstance = Readonly<{
  key: PackageInstanceKey;
  lifecycle: "admitted" | "activating" | "active" | "suspending" | "suspended" | "recovering" | "failed" | "crash_loop" | "disposing" | "disposed";
  processGeneration: number | null;
  mountState: "opening" | "mounted" | "failed" | "closing" | "closed" | null;
}>;

/** One installation registration, with its admission tier and result (ADR 0031). */
export type PackageRegistration = Readonly<{
  directory: string;
  expectedIdentity: string;
  override: boolean;
  tier: "team" | "personal";
  status: Readonly<{ kind: "admitted" }> | Readonly<{ kind: "refused"; code: string }>;
}>;

export type PackageHostSnapshot = Readonly<{
  revision: number;
  state: "opening" | "running" | "shutting_down" | "stopped" | "failed";
  packages: readonly PackageCatalogEntry[];
  instances: readonly PackageInstance[];
  diagnostics: readonly PackageBridgeError[];
  registrations: readonly PackageRegistration[];
}>;

/** A Package Directory read for the install preview. Inspecting runs no Package code. */
export type InspectedPackage = Readonly<{
  directory: string;
  identity: string;
  version: string;
  displayName: string;
  description?: string;
  author?: string;
  homepage?: string;
  capabilities: readonly Readonly<{ identity: string; major: number; minMinor: number; maxMinor: number }>[];
  provided: readonly Readonly<{ identity: string; major: number; minor: number; description: string }>[];
  consumed: readonly Readonly<{ identity: string; major: number; minMinor: number; maxMinor: number; required: boolean }>[];
  kind: "new" | "reload" | "replaces_registration" | "overrides_team";
}>;

/** The result of an install, reload, remove, or override write. */
export type PackageChange = Readonly<{
  /** The Host revision that shows a live change; `null` when the change waits for a restart. */
  revision: number | null;
  restartRequired: boolean;
}>;

/** One admitted Package's declared Semantic Capabilities and Ports, for its Package page. */
export type PackageAuthoringEntry = Readonly<{
  identity: string;
  capabilities: readonly Readonly<{ identity: string; major: number; minMinor: number; maxMinor: number }>[];
  provided: readonly Readonly<{ identity: string; major: number; minor: number; description: string }>[];
  consumed: readonly Readonly<{
    identity: string;
    major: number;
    minMinor: number;
    maxMinor: number;
    required: boolean;
    /** The Package that provides it, when one is selected. */
    provider: string | null;
    minor: number | null;
    /** Why it cannot be routed, as a Package Host error code. */
    unavailable: string | null;
  }>[];
}>;

export type PackageRegistrationInput = Readonly<{
  directory: string;
  expectedIdentity: string;
  acceptedRisks: readonly string[];
}>;

/** The Shell-only commands that add and manage Personal Packages on this device. */
export interface PackageManagementBridge {
  chooseDirectory(): Promise<string | null>;
  inspectDirectory(directory: string): Promise<InspectedPackage>;
  install(input: PackageRegistrationInput): Promise<PackageChange>;
  reload(identity: string): Promise<PackageChange>;
  remove(identity: string): Promise<PackageChange>;
  writeOverride(input: PackageRegistrationInput): Promise<PackageChange>;
  /** Restarts Teamarness. */
  restart(): Promise<void>;
  /** Replaces the process of one Personal Package Instance. A Team Instance is refused. */
  restartPackage(instance: PackageInstanceKey): Promise<void>;
  /** Declared Semantic Capabilities and Ports of every admitted Package. */
  authoringCatalog(): Promise<readonly PackageAuthoringEntry[]>;
}

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
  view: PackageUiView;
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
