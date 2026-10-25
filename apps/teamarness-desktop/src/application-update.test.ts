import { describe, expect, it, vi } from "vitest";
import { ApplicationUpdateController, renderApplicationUpdate } from "./application-update";
import type { ApplicationUpdateBridge, ApplicationUpdateSnapshot } from "./application-update-contracts";

export const snapshot = (revision = 1, state: ApplicationUpdateSnapshot["update"]["state"] = "ready"): ApplicationUpdateSnapshot => ({
  revision,
  update: { revision, state, installedVersion: "0.1.0", candidateVersion: "1.2.3", downloadedBytes: 20, archiveLimit: 100, error: null, pendingGeneration: "opaque-1", notes: "<img src=x onerror=alert(1)>" },
  lifecycle: { revision, shutdown: { host: false, core: false, uncertain: false }, phase: "running", failure: null, previousApp: null, guidance: null },
});
function fixture() {
  let listener: (value: ApplicationUpdateSnapshot) => void = () => undefined;
  const order: string[] = [];
  const bridge: ApplicationUpdateBridge = {
    subscribe: vi.fn(async (next) => { order.push("subscribe"); listener = next; return () => { order.push("detach"); }; }),
    status: vi.fn(async () => { order.push("status"); return snapshot(); }),
    check: vi.fn(async () => snapshot(2, "checking")),
    restart: vi.fn(async () => ({ ...snapshot(3), lifecycle: { ...snapshot().lifecycle, phase: "stopping" as const } })),
    quit: vi.fn(async () => undefined),
  };
  const views: Parameters<ConstructorParameters<typeof ApplicationUpdateController>[1]>[0][] = [];
  const controller = new ApplicationUpdateController(bridge, (view) => { views.push(view); order.push(view.updating ? "updating" : "view"); });
  return { controller, bridge, views, order, emit: (value: ApplicationUpdateSnapshot) => { listener(value); } };
}

describe("Installation update consent", () => {
  // Verifies: TEAM.SYS.SHELL-R07
  it("subscribes before status, rejects older revisions, and queries again on reattachment", async () => {
    const f = fixture();
    await f.controller.attach();
    expect(f.order.slice(0, 2)).toEqual(["subscribe", "status"]);
    f.emit(snapshot(5, "downloading"));
    f.emit(snapshot(2));
    expect(f.views.at(-1)?.snapshot?.revision).toBe(5);
    f.controller.detach();
    vi.mocked(f.bridge.status).mockResolvedValue(snapshot(6));
    await f.controller.attach();
    expect(f.bridge.status).toHaveBeenCalledTimes(2);
    expect(f.views.at(-1)?.snapshot?.revision).toBe(6);
  });
  // Verifies: TEAM.SYS.SHELL-R07
  it("keeps Later session-local with a retained ready affordance and no native action", async () => {
    const f = fixture(); await f.controller.attach();
    f.controller.later();
    expect(f.views.at(-1)?.dismissed).toBe(true);
    expect(renderApplicationUpdate(f.views.at(-1)!)).toContain("Update ready");
    expect(f.bridge.restart).not.toHaveBeenCalled();
    f.controller.showReady();
    expect(f.views.at(-1)?.dismissed).toBe(false);
    await f.controller.check();
    expect(f.bridge.check).toHaveBeenCalledOnce();
  });
  // Verifies: TEAM.SYS.SHELL-R07
  it("renders safe notes/progress and nonmodal failures", () => {
    const html = renderApplicationUpdate({ snapshot: snapshot(), updating: false, dismissed: false, unavailable: false });
    expect(html).toContain("&lt;img"); expect(html).not.toContain("<img");
    expect(html).toContain("Unsaved");
    const downloading = renderApplicationUpdate({ snapshot: snapshot(2, "downloading"), updating: false, dismissed: false, unavailable: false });
    expect(downloading).toContain("20"); expect(downloading).toContain('role="status"');
  });
  // Verifies: TEAM.SYS.SHELL-R07
  it("renders Updating synchronously before invoke, sends only generation, and coalesces duplicate clicks", async () => {
    const f = fixture(); await f.controller.attach();
    vi.mocked(f.bridge.restart).mockImplementation(async () => { expect(f.views.at(-1)?.updating).toBe(true); return { ...snapshot(4), lifecycle: { ...snapshot().lifecycle, phase: "installing" } }; });
    await Promise.all([f.controller.restart(), f.controller.restart()]);
    expect(f.bridge.restart).toHaveBeenCalledExactlyOnceWith("opaque-1");
    expect(f.views.at(-1)?.updating).toBe(true);
    f.emit(snapshot(100));
    expect(f.views.at(-1)?.updating).toBe(true);
  });
  // Verifies: TEAM.SYS.SHELL-R07
  it("queries authoritative status on stale rejection, restoring only if still running", async () => {
    const f = fixture(); await f.controller.attach();
    vi.mocked(f.bridge.restart).mockRejectedValue({ code: "stale_candidate" });
    vi.mocked(f.bridge.status).mockResolvedValue(snapshot(3, "idle"));
    await f.controller.restart();
    expect(f.bridge.status).toHaveBeenCalledTimes(2);
    expect(f.views.at(-1)?.updating).toBe(false);
    expect(f.views.at(-1)?.snapshot?.update.state).toBe("idle");
  });
  // Verifies: TEAM.SYS.SHELL-R07
  it("stays fenced on unknown admission and renders recovery with Quit but no install retry", async () => {
    const f = fixture(); await f.controller.attach();
    vi.mocked(f.bridge.restart).mockRejectedValue({}); vi.mocked(f.bridge.status).mockRejectedValue({});
    await f.controller.restart(); expect(f.views.at(-1)?.updating).toBe(true);
    f.emit({ ...snapshot(7), lifecycle: { ...snapshot().lifecycle, phase: "failed", failure: "install", previousApp: "/local/<old>", guidance: "Reinstall" } });
    const html = renderApplicationUpdate(f.views.at(-1)!);
    expect(html).toContain("Quit"); expect(html).toContain("trusted disk image"); expect(html).toContain("&lt;old&gt;");
    expect(html).not.toContain("Restart now"); expect(html).not.toContain("Check again");
    await f.controller.quit(); expect(f.bridge.quit).toHaveBeenCalledOnce();
  });
});

// Verifies: TEAM.SYS.SHELL-R07
it("an event received before the initial read wins over its older response", async () => {
  const f = fixture();
  let release!: (value: ApplicationUpdateSnapshot) => void;
  vi.mocked(f.bridge.status).mockImplementation(() => new Promise((resolve) => { release = resolve; }));
  const attached = f.controller.attach();
  await Promise.resolve();
  f.emit(snapshot(8, "verifying")); release(snapshot(1)); await attached;
  expect(f.views.at(-1)?.snapshot?.revision).toBe(8);
});

// Verifies: TEAM.SYS.SHELL-R07
it("reattachment resolves unknown rejected admission only from a fresh authoritative query", async () => {
  const f = fixture(); await f.controller.attach();
  vi.mocked(f.bridge.restart).mockRejectedValue({}); vi.mocked(f.bridge.status).mockRejectedValue({});
  await f.controller.restart();
  f.emit(snapshot(4)); expect(f.views.at(-1)?.updating).toBe(true);
  vi.mocked(f.bridge.status).mockResolvedValue(snapshot(5, "idle"));
  await f.controller.attach(); expect(f.views.at(-1)?.updating).toBe(false);
});
