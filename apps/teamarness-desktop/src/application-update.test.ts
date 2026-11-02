import { describe, expect, it, vi } from "vitest";
import { ApplicationUpdateController, renderApplicationUpdate, renderApplicationUpdateFooter, type ApplicationUpdateView } from "./application-update";
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
  it.each(["idle", "checking"] as const)("renders no top updater panel while %s", (state) => {
    const base = snapshot(2, state);
    const value = { ...base, update: { ...base.update, candidateVersion: null, pendingGeneration: null, notes: null, downloadedBytes: 0 } };
    expect(renderApplicationUpdate({ snapshot: value, updating: false, dismissed: false, unavailable: false })).toBe("");
  });
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
    expect(renderApplicationUpdate(f.views.at(-1)!)).toBe("");
    expect(renderApplicationUpdateFooter(f.views.at(-1)!)).toContain('data-update-action="menu-update"');
    expect(f.bridge.restart).not.toHaveBeenCalled();
    f.controller.showReady();
    expect(f.views.at(-1)?.dismissed).toBe(false);
    await f.controller.check();
    expect(f.bridge.check).toHaveBeenCalledOnce();
  });
  // Verifies: TEAM.SYS.SHELL-R07
  it("renders safe notes/progress and nonmodal failures", () => {
    const html = renderApplicationUpdateFooter({ snapshot: snapshot(), updating: false, dismissed: false, unavailable: false }, true);
    expect(html).toContain("&lt;img"); expect(html).not.toContain("<img");
    expect(html).toContain("Unsaved");
    const downloading = renderApplicationUpdateFooter({ snapshot: snapshot(2, "downloading"), updating: false, dismissed: false, unavailable: false }, true);
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

describe("Quiet footer updates", () => {
  function view(state: ApplicationUpdateSnapshot["update"]["state"] = "idle"): ApplicationUpdateView {
    const base = snapshot(2, state);
    return { snapshot: { ...base, update: { ...base.update, installedVersion: "0.1.1", ...(state === "idle" ? { candidateVersion: null, pendingGeneration: null, notes: null, downloadedBytes: 0 } : {}) } }, updating: false, dismissed: false, unavailable: false };
  }

  it("shows only the installed version while healthy, with status and manual checking behind it", () => {
    const current = view();
    expect(renderApplicationUpdate(current)).toBe("");
    const closed = renderApplicationUpdateFooter(current);
    expect(closed).toContain(">v0.1.1</button>");
    expect(closed).not.toContain('data-update-action="menu-update"');
    expect(closed).not.toContain("Check now");
    const open = renderApplicationUpdateFooter(current, true);
    expect(open).toContain("Up to date.");
    expect(open).toContain("Check now");
    expect(open).not.toContain('aria-modal="true"');
  });

  it("does not claim a completed check before the first check", () => {
    const current = view();
    const initial = { ...current, snapshot: { ...current.snapshot!, update: { ...current.snapshot!.update, revision: 0 } } };
    expect(renderApplicationUpdateFooter(initial, true)).toContain("Waiting for the first update check");
    expect(renderApplicationUpdateFooter(initial, true)).not.toContain("Up to date");
  });

  it("keeps routine checking silent and disables redundant manual checks", () => {
    const current = view("checking");
    expect(renderApplicationUpdate(current)).toBe("");
    expect(renderApplicationUpdateFooter(current)).not.toContain('data-update-action="menu-update"');
    const open = renderApplicationUpdateFooter(current, true);
    expect(open).toContain("Checking for an application update");
    expect(open).toContain('data-update-action="check" disabled');
  });

  it.each(["downloading", "verifying", "ready"] as const)("keeps %s in the footer, not a top panel", (state) => {
    const current = view(state);
    expect(renderApplicationUpdate(current)).toBe("");
    expect(renderApplicationUpdateFooter(current)).toContain('data-update-action="menu-update"');
    expect(renderApplicationUpdateFooter(current)).not.toContain('data-update-action="restart"');
    const open = renderApplicationUpdateFooter(current, true);
    expect(open).toContain('role="status"');
    expect(open).not.toContain("limit 100");
    if (state === "ready") {
      expect(open).toContain("Restart now");
      expect(open).toContain("Later");
      expect(open).toContain("Save your work");
    }
  });

  it("retains a ready icon after Later without exposing a restart shortcut", () => {
    const current = { ...view("ready"), dismissed: true };
    expect(renderApplicationUpdate(current)).toBe("");
    expect(renderApplicationUpdateFooter(current)).toContain("Review and restart");
    expect(renderApplicationUpdateFooter(current)).not.toContain('data-update-action="restart"');
    expect(renderApplicationUpdateFooter(current, true)).toContain("Restart now");
  });

  it("can dismiss a check-error row without erasing the error from the popover", () => {
    const base = view();
    const failed = { ...base, snapshot: { ...base.snapshot!, update: { ...base.snapshot!.update, error: "network" as const } } };
    expect(renderApplicationUpdate(failed)).toContain("Dismiss update notification");
    expect(renderApplicationUpdate(failed)).toContain("network");
    expect(renderApplicationUpdate(failed, true)).toBe("");
    expect(renderApplicationUpdateFooter(failed, true)).toContain("network");
    expect(renderApplicationUpdateFooter(failed, true)).toContain("Check now");
  });

  it("keeps recovery visible even after a notification was dismissed, and removes all footer actions", () => {
    const base = view("ready");
    const failed: ApplicationUpdateView = { ...base, updating: true, snapshot: { ...base.snapshot!, lifecycle: { ...base.snapshot!.lifecycle, phase: "failed", failure: "install" } } };
    expect(renderApplicationUpdate(failed, true)).toContain("Update recovery");
    expect(renderApplicationUpdate(failed, true)).toContain("Quit");
    expect(renderApplicationUpdate(failed, true)).not.toContain("dismiss-error");
    expect(renderApplicationUpdateFooter(failed, true)).toBe("");
  });

  it("lets an unavailable status be retried instead of trusting an old busy snapshot", () => {
    const unavailable = { ...view("checking"), unavailable: true };
    const open = renderApplicationUpdateFooter(unavailable, true);
    expect(open).toContain("status unavailable");
    expect(open).toContain("Check now");
    expect(open).not.toContain('data-update-action="check" disabled');
  });

  it("shows the version in disabled builds without offering production checking", () => {
    const current = view("disabled");
    expect(renderApplicationUpdate(current)).toBe("");
    const open = renderApplicationUpdateFooter(current, true);
    expect(open).toContain("v0.1.1");
    expect(open).toContain("Updates are disabled");
    expect(open).toContain('data-update-action="check" disabled');
  });
});
