export type Installation = Readonly<{
  identity: string;
  displayName: string;
}>;

export type Workspace = Readonly<{
  id: string;
  name: string;
  createdBy: string;
  createdAtUnixMs: number;
}>;

export type MemberRole = "member" | "manager";

export type Member = Readonly<{
  identity: string;
  role: MemberRole;
}>;

export type Membership = Readonly<{
  workspaceId: string;
  epoch: string;
  version: string;
  members: readonly Member[];
}>;

export type WorkspaceState = Readonly<{
  workspace: Workspace;
  membership: Membership;
}>;

export type Invitation = Readonly<{
  value: string;
  sensitivity: "bearer_admission";
}>;

export type BridgeErrorCode =
  | "invalid_input"
  | "authentication"
  | "expired"
  | "already_used"
  | "readiness"
  | "timeout"
  | "overload"
  | "invalid_bootstrap"
  | "local_conflict"
  | "admission_no_longer_current"
  | "uncertain_storage_outcome"
  | "package_reconciliation"
  | "shutdown";

export type BridgeError = Readonly<{
  code: BridgeErrorCode;
  retryable: boolean;
}>;

export function isBridgeError(value: unknown): value is BridgeError {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as Record<string, unknown>;
  return typeof candidate.code === "string" && typeof candidate.retryable === "boolean";
}
