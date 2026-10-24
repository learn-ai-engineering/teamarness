import "./styles.css";
import { bridge } from "./bridge";
import { packageBridge } from "./package-bridge";
import { browserFrameDom, browserThemeSource, PackageMountController } from "./package-mount";
import { createPackageShell } from "./package-shell";
import { Shell, shellFrameFromDocument } from "./shell";
import { createInstallationThemeController } from "./theme";
import { ShellController } from "./view";

const theme = createInstallationThemeController();
const frame = shellFrameFromDocument();
const packageShell = createPackageShell(
  packageBridge,
  new PackageMountController(
    packageBridge,
    browserFrameDom,
    browserThemeSource({
      root: document.documentElement,
      getComputedStyle: (element) => getComputedStyle(element),
      matchMedia: (query) => window.matchMedia(query),
      preferences: theme,
    }),
  ),
  frame.packageMount,
);

const shell = new Shell(frame, {
  packageShell,
  theme,
  workspaceActions: {
    selectWorkspace: (workspaceId) => { controller.selectWorkspace(workspaceId); },
    createWorkspace: async (name, personName) => { await controller.createWorkspace(name, personName); },
    joinWorkspace: async (invitation, personName) => { await controller.joinWorkspace(invitation, personName); },
    rename: async (workspaceId, name) => { await controller.rename(workspaceId, name); },
    addAgent: async (workspaceId, name) => { await controller.addAgent(workspaceId, name); },
    removeAgent: async (workspaceId, agentId) => { await controller.removeAgent(workspaceId, agentId); },
    renameAgent: async (workspaceId, agentId, name) => { await controller.renameAgent(workspaceId, agentId, name); },
    copyInvitation: async (workspaceId) => { await controller.copyInvitation(workspaceId); },
    refreshMembership: async (workspaceId) => { await controller.refreshMembership(workspaceId); },
    retry: async () => { await controller.load(); },
  },
});

const controller = new ShellController(bridge, (model) => {
  shell.updateWorkspace(model);
});

void packageShell.startSnapshotUpdates({
  update: (snapshot) => { shell.updatePackages(snapshot); },
  fail: () => { shell.updatePackageSnapshotFailure(); },
}).catch(() => undefined);
void controller.load();
window.addEventListener("beforeunload", () => controller.dispose(), { once: true });
