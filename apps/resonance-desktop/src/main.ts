import "./styles.css";
import { bridge } from "./bridge";
import { ShellController, renderShell } from "./view";

const root = document.querySelector<HTMLElement>("#app");
if (!root) throw new Error("Shell mount is unavailable");

const controller = new ShellController(bridge, (model) => {
  root.innerHTML = renderShell(model);
  bindActions();
});

function bindActions(): void {
  document.querySelector<HTMLFormElement>("#create-workspace")?.addEventListener("submit", (event) => {
    event.preventDefault();
    const input = document.querySelector<HTMLInputElement>("#workspace-name");
    if (input) void controller.createWorkspace(input.value);
  });
  document.querySelector<HTMLFormElement>("#join-workspace")?.addEventListener("submit", (event) => {
    event.preventDefault();
    const input = document.querySelector<HTMLTextAreaElement>("#invitation-value");
    if (input) void controller.joinWorkspace(input.value);
  });
  document.querySelectorAll<HTMLButtonElement>("[data-invite]").forEach((button) => {
    button.addEventListener("click", () => void controller.copyInvitation(button.dataset.invite ?? ""));
  });
  document.querySelector<HTMLButtonElement>("#retry")?.addEventListener("click", () => void controller.load());
}

void controller.load();
