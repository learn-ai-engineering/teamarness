import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ApplicationUpdateBridge, ApplicationUpdateSnapshot } from "./application-update-contracts";

export const applicationUpdateBridge: ApplicationUpdateBridge = Object.freeze({
  subscribe: (listener: (snapshot: ApplicationUpdateSnapshot) => void) =>
    listen<ApplicationUpdateSnapshot>("application-update", (event) => { listener(event.payload); }),
  status: () => invoke<ApplicationUpdateSnapshot>("application_update_status"),
  check: () => invoke<ApplicationUpdateSnapshot>("application_update_check"),
  restart: (generation: string) => invoke<ApplicationUpdateSnapshot>("application_update_restart", { generation }),
  quit: () => invoke<void>("application_update_quit"),
});
