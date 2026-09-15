import "./styles.css";
import { bridge } from "./bridge";
import { packageBridge } from "./package-bridge";
import { PackageMountController } from "./package-mount";
import { createPackageShell } from "./package-shell";
import { createInstallationThemeController } from "./theme";
import { ShellController, ShellRenderer } from "./view";

createInstallationThemeController();

const root = document.querySelector<HTMLElement>("#app");
if (!root) throw new Error("Shell mount is unavailable");

root.innerHTML = `<div class="shell-layout">
  <main id="shell-controls"></main>
  <aside id="package-pane" aria-label="Package interface">
    <nav id="package-navigation"></nav>
    <div id="package-mount"></div>
  </aside>
</div>`;
const controls = root.querySelector<HTMLElement>("#shell-controls");
const navigation = root.querySelector<HTMLElement>("#package-navigation");
const packageTarget = root.querySelector<HTMLElement>("#package-mount");
if (!controls || !navigation || !packageTarget) throw new Error("Shell layout is unavailable");
const controlsElement = controls;
const navigationElement = navigation;
const packageTargetElement = packageTarget;

const renderer = new ShellRenderer(controlsElement, packageTargetElement);
const packageShell = createPackageShell(
  packageBridge,
  new PackageMountController(packageBridge),
  navigationElement,
  packageTargetElement,
);
const controller = new ShellController(bridge, (model) => {
  renderer.render(model);
  bindActions();
  void packageShell.refresh();
});

function bindActions(): void {
  controlsElement.querySelector<HTMLFormElement>("#create-workspace")?.addEventListener("submit", (event) => {
    event.preventDefault();
    const input = controlsElement.querySelector<HTMLInputElement>("#workspace-name");
    if (input) void controller.createWorkspace(input.value);
  });
  controlsElement.querySelector<HTMLFormElement>("#join-workspace")?.addEventListener("submit", (event) => {
    event.preventDefault();
    const input = controlsElement.querySelector<HTMLTextAreaElement>("#invitation-value");
    if (input) void controller.joinWorkspace(input.value);
  });
  controlsElement.querySelectorAll<HTMLButtonElement>("[data-invite]").forEach((button) => {
    button.addEventListener("click", () => void controller.copyInvitation(button.dataset.invite ?? ""));
  });
  controlsElement.querySelectorAll<HTMLButtonElement>("[data-refresh-membership]").forEach((button) => {
    button.addEventListener("click", () => void controller.refreshMembership(button.dataset.refreshMembership ?? ""));
  });
  controlsElement.querySelector<HTMLButtonElement>("#retry")?.addEventListener("click", () => void controller.load());
}

void controller.load();
