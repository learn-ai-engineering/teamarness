import { expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { applicationUpdateBridge } from "./application-update-bridge";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => ({})) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }));

// Verifies: TEAM.SYS.SHELL-R07
it("maps only four main-Shell controls and an opaque restart generation across the real bridge boundary", async () => {
  await applicationUpdateBridge.status();
  await applicationUpdateBridge.check();
  await applicationUpdateBridge.restart("opaque-generation");
  await applicationUpdateBridge.quit();
  expect(vi.mocked(invoke).mock.calls).toEqual([
    ["application_update_status"], ["application_update_check"],
    ["application_update_restart", { generation: "opaque-generation" }], ["application_update_quit"],
  ]);
  const listener = vi.fn();
  const unlisten = await applicationUpdateBridge.subscribe(listener);
  expect(listen).toHaveBeenCalledWith("application-update", expect.any(Function));
  expect(typeof unlisten).toBe("function");
});
