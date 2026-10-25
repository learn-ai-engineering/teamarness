export interface ApplicationUpdateSnapshot {
  readonly revision: number;
  readonly update: Readonly<{
    revision: number;
    state: "disabled" | "idle" | "checking" | "downloading" | "verifying" | "ready" | "stopped";
    installedVersion: string;
    candidateVersion: string | null;
    downloadedBytes: number;
    archiveLimit: number;
    error: "metadata" | "network" | "too_large" | "timeout" | "verification" | "cancelled" | "stale_candidate" | null;
    pendingGeneration: string | null;
    notes: string | null;
  }>;
  readonly lifecycle: Readonly<{
    revision: number;
    shutdown: Readonly<{ host: boolean; core: boolean; uncertain: boolean }>;
    phase: "running" | "stopping" | "installing" | "restarting" | "stopped" | "failed";
    failure: "shutdown" | "shutdown_timeout" | "install" | "uncertain" | null;
    previousApp: string | null;
    guidance: string | null;
  }>;
}

export interface ApplicationUpdateBridge {
  subscribe(listener: (snapshot: ApplicationUpdateSnapshot) => void): Promise<() => void>;
  status(): Promise<ApplicationUpdateSnapshot>;
  check(): Promise<ApplicationUpdateSnapshot>;
  restart(generation: string): Promise<ApplicationUpdateSnapshot>;
  quit(): Promise<void>;
}
