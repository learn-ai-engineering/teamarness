import "./styles.css";
import { applicationUpdateBridge } from "./application-update-bridge";
import { ApplicationUpdateController, BrowserApplicationUpdateView } from "./application-update";
import { bridge } from "./bridge";
import { createTauriPackageManagementBridge, packageBridge } from "./package-bridge";
import { browserFrameDom, browserThemeSource, PackageMountController } from "./package-mount";
import { createPackageShell } from "./package-shell";
import { createInstallationPackageVisibility } from "./package-visibility";
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
  packageManagement: createTauriPackageManagementBridge(),
  theme,
  packageVisibility: createInstallationPackageVisibility(),
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

const updateView = new BrowserApplicationUpdateView(
  document.getElementById("application-update-notice")!,
  document.getElementById("application-update-terminal")!,
  (blocked) => { shell.setApplicationUpdating(blocked); document.getElementById("app")!.hidden = blocked; },
);
const updates = new ApplicationUpdateController(applicationUpdateBridge, (view) => { updateView.render(view, updates); });
void updates.attach();
window.addEventListener("pageshow", () => { void updates.attach(); });
window.addEventListener("beforeunload", () => { updates.detach(); }, { once: true });
