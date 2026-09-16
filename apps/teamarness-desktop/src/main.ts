import "./styles.css";
import { bridge } from "./bridge";
import { packageBridge } from "./package-bridge";
import { PackageMountController } from "./package-mount";
import { createPackageShell } from "./package-shell";
import { Shell, shellFrameFromDocument } from "./shell";
import { createInstallationThemeController } from "./theme";
import { ShellController } from "./view";

const theme = createInstallationThemeController();
const frame = shellFrameFromDocument();
const packageShell = createPackageShell(
  packageBridge,
  new PackageMountController(packageBridge),
  frame.packageMount,
);

const shell = new Shell(frame, {
  packageShell,
  theme,
  workspaceActions: {
    selectWorkspace: (workspaceId) => { controller.selectWorkspace(workspaceId); },
    createWorkspace: async (name) => { await controller.createWorkspace(name); },
    joinWorkspace: async (invitation) => { await controller.joinWorkspace(invitation); },
    copyInvitation: async (workspaceId) => { await controller.copyInvitation(workspaceId); },
    refreshMembership: async (workspaceId) => { await controller.refreshMembership(workspaceId); },
    retry: async () => { await controller.load(); },
  },
});

const controller = new ShellController(bridge, (model) => {
  shell.updateWorkspace(model);
  void packageShell.refresh()
    .then((snapshot) => { shell.updatePackages(snapshot); })
    .catch(() => { shell.updatePackageSnapshotFailure(); });
});

void controller.load();
