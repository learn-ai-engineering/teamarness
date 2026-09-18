import { PACKAGE_REQUEST_ERROR_KIND, type PackageClient, type PackageValue } from "@teamarness/package-ui-sdk";
import { describe, expect, it } from "vitest";

import {
  createRepoAwareController,
  type RepoAwareState,
  type Registration,
  type Snapshot,
} from "./index";

const makeRegistrations = (count: number): Registration[] =>
  Array.from({ length: count }, (_, i) => ({
    id: `reg-${i + 1}`,
    path: `/Users/test/repo-${i + 1}`,
    addedAt: `2026-09-16T${String(10 + i).padStart(2, "0")}:00:00Z`,
  }));

const makeSnapshot = (regId: string, clean = true): Snapshot => ({
  registrationId: regId,
  path: `/Users/test/repo-1`,
  originRemote: "https://github.com/owner/repo.git",
  githubUrl: "https://github.com/owner/repo",
  head: { kind: "branch", name: "main" },
  commit: "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0",
  isClean: clean,
  fetchedAt: "2026-09-16T12:00:00Z",
});

class FakeClient implements PackageClient {
  readonly requests: Array<{ operation: string; payload: PackageValue }> = [];
  private registrations: Registration[] = [];
  failNext = false;
  rejectNextWith: unknown = null;
  snapshotOverride: Snapshot | null = null;

  constructor(registrations?: Registration[]) {
    if (registrations) this.registrations = registrations;
  }

  async request<TResponse extends PackageValue>(operation: string, payload: PackageValue): Promise<TResponse> {
    this.requests.push({ operation, payload });
    if (this.rejectNextWith !== null) {
      const rejection = this.rejectNextWith;
      this.rejectNextWith = null;
      throw rejection;
    }
    if (this.failNext) {
      this.failNext = false;
      throw new Error("Backend unavailable");
    }
    if (operation === "repo_aware.list") {
      return { registrations: this.registrations } as unknown as TResponse;
    }
    if (operation === "repo_aware.register") {
      const { path } = payload as { path: string };
      const newReg: Registration = {
        id: `reg-${this.registrations.length + 1}`,
        path,
        addedAt: "2026-09-16T12:00:00Z",
      };
      this.registrations = [...this.registrations, newReg];
      return { registration: newReg } as unknown as TResponse;
    }
    if (operation === "repo_aware.snapshot") {
      const { registrationId } = payload as { registrationId: string };
      if (this.snapshotOverride) return { registrationId, snapshot: this.snapshotOverride } as unknown as TResponse;
      return {
        registrationId,
        snapshot: makeSnapshot(registrationId, true),
      } as unknown as TResponse;
    }
    if (operation === "repo_aware.remove") {
      const { registrationId } = payload as { registrationId: string };
      this.registrations = this.registrations.filter((r) => r.id !== registrationId);
      return { removed: true } as unknown as TResponse;
    }
    throw new Error(`Unexpected operation ${operation}`);
  }

  async subscribe(): Promise<never> {
    throw new Error("No subscriptions");
  }
}

describe("Repo-aware Package UI", () => {
  it("exports mount function", async () => {
    const mod = await import("./index");
    expect(typeof mod.mount).toBe("function");
  });

  it("shows onboarding when no registrations exist", async () => {
    const client = new FakeClient();
    let state: RepoAwareState | undefined;
    const controller = createRepoAwareController(client, (next) => { state = next; });

    await controller.start();

    expect(client.requests).toEqual([{ operation: "repo_aware.list", payload: {} }]);
    expect(state?.registrations).toEqual([]);
    expect(state?.selectedRegistrationId).toBeNull();
    expect(state?.snapshot).toBeNull();
  });

  it("selects first registration and requests snapshot when registrations exist", async () => {
    const regs = makeRegistrations(2);
    const client = new FakeClient(regs);
    let state: RepoAwareState | undefined;
    const controller = createRepoAwareController(client, (next) => { state = next; });

    await controller.start();

    expect(client.requests).toHaveLength(2);
    expect(client.requests[0]).toEqual({ operation: "repo_aware.list", payload: {} });
    expect(client.requests[1]).toEqual({ operation: "repo_aware.snapshot", payload: { registrationId: "reg-1" } });
    expect(state?.registrations).toHaveLength(2);
    expect(state?.selectedRegistrationId).toBe("reg-1");
    expect(state?.snapshot).not.toBeNull();
    expect(state?.snapshot?.registrationId).toBe("reg-1");
  });

  it("register adds a registration and selects it", async () => {
    const client = new FakeClient();
    let state: RepoAwareState | undefined;
    const controller = createRepoAwareController(client, (next) => { state = next; });

    await controller.start();
    await controller.register("/Users/test/new-repo");

    // First list, then register, then list again, then snapshot
    const listCalls = client.requests.filter((r) => r.operation === "repo_aware.list");
    expect(listCalls).toHaveLength(2);
    const regCall = client.requests.find((r) => r.operation === "repo_aware.register");
    expect(regCall).toEqual({ operation: "repo_aware.register", payload: { path: "/Users/test/new-repo" } });
    const regs = state?.registrations ?? [];
    expect(regs).toHaveLength(1);
    expect(regs[0]?.path).toBe("/Users/test/new-repo");
    expect(state?.selectedRegistrationId).toBe(regs[0]?.id);
    expect(state?.snapshot).not.toBeNull();
  });

  it("selectRegistration requests a new snapshot", async () => {
    const regs = makeRegistrations(2);
    const client = new FakeClient(regs);
    let state: RepoAwareState | undefined;
    const controller = createRepoAwareController(client, (next) => { state = next; });

    await controller.start();
    const requestCount = client.requests.length;
    await controller.selectRegistration("reg-2");

    expect(client.requests).toHaveLength(requestCount + 1);
    expect(client.requests[requestCount]).toEqual({ operation: "repo_aware.snapshot", payload: { registrationId: "reg-2" } });
    expect(state?.selectedRegistrationId).toBe("reg-2");
  });

  it("refresh re-requests snapshot for selected registration", async () => {
    const regs = makeRegistrations(1);
    const client = new FakeClient(regs);
    let state: RepoAwareState | undefined;
    const controller = createRepoAwareController(client, (next) => { state = next; });

    await controller.start();
    const requestCount = client.requests.length;

    // Change the snapshot result
    const dirtySnapshot = makeSnapshot("reg-1", false);
    client.snapshotOverride = dirtySnapshot;
    await controller.refresh();

    expect(client.requests).toHaveLength(requestCount + 1);
    expect(client.requests[requestCount]).toEqual({ operation: "repo_aware.snapshot", payload: { registrationId: "reg-1" } });
    expect(state?.snapshot?.isClean).toBe(false);
  });

  it("remove deletes registration and selects next or shows onboarding", async () => {
    const regs = makeRegistrations(2);
    const client = new FakeClient(regs);
    let state: RepoAwareState | undefined;
    const controller = createRepoAwareController(client, (next) => { state = next; });

    await controller.start();

    controller.requestRemove();
    expect(state?.showRemoveConfirm).toBe(true);

    await controller.confirmRemove();

    expect(state?.registrations).toHaveLength(1);
    expect(state?.selectedRegistrationId).toBe("reg-2");
    // Should have: remove snapshot for reg-2
    const snapshotCalls = client.requests.filter((r) => r.operation === "repo_aware.snapshot");
    const lastSnapshotPayload = snapshotCalls[snapshotCalls.length - 1]?.payload as { registrationId: string } | undefined;
    expect(lastSnapshotPayload?.registrationId).toBe("reg-2");
  });

  it("remove last registration shows onboarding", async () => {
    const regs = makeRegistrations(1);
    const client = new FakeClient(regs);
    let state: RepoAwareState | undefined;
    const controller = createRepoAwareController(client, (next) => { state = next; });

    await controller.start();
    controller.requestRemove();
    await controller.confirmRemove();

    expect(state?.registrations).toHaveLength(0);
    expect(state?.selectedRegistrationId).toBeNull();
    expect(state?.snapshot).toBeNull();
  });

  it("presents request errors", async () => {
    const client = new FakeClient();
    let state: RepoAwareState | undefined;
    const controller = createRepoAwareController(client, (next) => { state = next; });
    await controller.start();

    client.failNext = true;
    await controller.register("/Users/test/fail-repo");

    expect(state?.error).toBe("Backend unavailable");
    expect(state?.pending).toBe("none");
  });

  it("shows the Package error code for a rejected registration", async () => {
    const client = new FakeClient();
    let state: RepoAwareState | undefined;
    const controller = createRepoAwareController(client, (next) => { state = next; });
    await controller.start();

    client.rejectNextWith = Object.freeze({
      kind: PACKAGE_REQUEST_ERROR_KIND,
      version: 2,
      namespace: "teamarness.package-host",
      code: "backend_error",
      stage: "ui",
      retryable: false,
      recovery: "none",
      diagnosticId: "diag-1",
      packageError: Object.freeze({
        code: "repo_aware.not_git_repository",
        retryable: false,
        details: Object.freeze({}),
      }),
    });
    await controller.register("relative/path");

    expect(state?.error).toBe("repo_aware.not_git_repository");
    expect(state?.pending).toBe("none");
  });

  it("dispose prevents further state updates", async () => {
    const client = new FakeClient();
    const states: RepoAwareState[] = [];
    const controller = createRepoAwareController(client, (s) => { states.push(s); });
    await controller.start();

    controller.dispose();
    const stateCount = states.length;

    // These should not trigger updates
    await controller.register("/Users/test/repo");
    await controller.refresh();
    controller.requestRemove();
    await controller.confirmRemove();

    expect(states).toHaveLength(stateCount);
  });

  it("cancelRemove hides confirmation", async () => {
    const regs = makeRegistrations(1);
    const client = new FakeClient(regs);
    let state: RepoAwareState | undefined;
    const controller = createRepoAwareController(client, (next) => { state = next; });

    await controller.start();
    controller.requestRemove();
    expect(state?.showRemoveConfirm).toBe(true);

    controller.cancelRemove();
    expect(state?.showRemoveConfirm).toBe(false);
  });
});