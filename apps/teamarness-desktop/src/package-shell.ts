import type { PackageHostSnapshot, PackageInstanceKey } from "./package-contracts";
import type { PackageMountController } from "./package-mount";

export type PackageShellActions = Readonly<{
  select(instance: PackageInstanceKey): Promise<void>;
  reloadUi(): Promise<void>;
  suspendBackend(): Promise<void>;
  recoverBackend(): Promise<void>;
  disposeBackend(): Promise<void>;
}>;

export interface PackageShellView {
  readonly mountTarget: unknown;
  render(snapshot: PackageHostSnapshot, actions: PackageShellActions): void;
  showFailure(actions: PackageShellActions): void;
}

export interface PackageSnapshotBridge {
  snapshot(): Promise<PackageHostSnapshot>;
}

interface MountController {
  mount(instance: PackageInstanceKey, target: unknown, onFailure: () => void): Promise<unknown>;
  unmount(): Promise<void>;
  suspendBackend(instance: PackageInstanceKey): Promise<void>;
  recoverBackend(instance: PackageInstanceKey): Promise<void>;
  disposeBackend(instance: PackageInstanceKey): Promise<void>;
}

export class PackageShell {
  private selected: PackageInstanceKey | null = null;

  public constructor(
    private readonly bridge: PackageSnapshotBridge,
    private readonly mounts: MountController,
    private readonly view: PackageShellView,
  ) {}

  public async refresh(): Promise<void> {
    const snapshot = await this.bridge.snapshot();
    this.view.render(snapshot, this.actions());
  }

  private actions(): PackageShellActions {
    return Object.freeze({
      select: async (instance) => {
        this.selected = instance;
        await this.loadSelected();
      },
      reloadUi: async () => { await this.loadSelected(); },
      suspendBackend: async () => {
        await this.runLifecycle((instance) => this.mounts.suspendBackend(instance));
      },
      recoverBackend: async () => {
        await this.runLifecycle((instance) => this.mounts.recoverBackend(instance));
      },
      disposeBackend: async () => {
        await this.runLifecycle((instance) => this.mounts.disposeBackend(instance));
      },
    });
  }

  private async runLifecycle(operation: (instance: PackageInstanceKey) => Promise<void>): Promise<void> {
    if (!this.selected) return;
    let failed = false;
    try {
      await operation(this.selected);
    } catch {
      failed = true;
    }
    try {
      await this.refresh();
    } catch {
      failed = true;
    }
    if (failed) this.view.showFailure(this.actions());
  }

  private async loadSelected(): Promise<void> {
    if (!this.selected) return;
    let failureShown = false;
    const showFailure = (): void => {
      if (failureShown) return;
      failureShown = true;
      this.view.showFailure(this.actions());
    };
    try {
      await this.mounts.mount(this.selected, this.view.mountTarget, showFailure);
    } catch {
      showFailure();
    }
  }
}

export class BrowserPackageShellView implements PackageShellView {
  public constructor(
    private readonly navigation: HTMLElement,
    public readonly mountTarget: HTMLElement,
  ) {}

  public render(snapshot: PackageHostSnapshot, actions: PackageShellActions): void {
    const title = document.createElement("h2");
    title.textContent = "Package interfaces";
    const list = document.createElement("div");
    list.className = "package-navigation";
    for (const instance of snapshot.instances) {
      const catalog = snapshot.packages.find(({ identity }) => identity === instance.key.packageIdentity);
      const button = document.createElement("button");
      button.type = "button";
      const generation = instance.processGeneration === null ? "no generation" : `generation ${instance.processGeneration}`;
      button.textContent = `${catalog?.displayName ?? instance.key.packageIdentity} · ${instance.key.workspaceId} · ${instance.lifecycle} · ${generation}`;
      const source = document.createElement("span");
      source.className = "package-source";
      source.textContent = catalog?.source === "local" ? "Local" : "Bundled";
      source.setAttribute("aria-label", `Package source: ${source.textContent}`);
      button.append(source);
      button.dataset.packageIdentity = instance.key.packageIdentity;
      button.dataset.workspaceId = instance.key.workspaceId;
      button.addEventListener("click", () => { void actions.select(instance.key); });
      list.append(button);
    }
    if (snapshot.instances.length === 0) {
      const empty = document.createElement("p");
      empty.textContent = "No Package interfaces are available.";
      list.append(empty);
    }
    const lifecycleActions = document.createElement("div");
    lifecycleActions.className = "actions";
    const reload = actionButton("Reload UI", () => { void actions.reloadUi(); });
    const suspend = actionButton("Suspend", () => { void actions.suspendBackend(); });
    const recover = actionButton("Recover Backend", () => { void actions.recoverBackend(); });
    const dispose = actionButton("Dispose", () => { void actions.disposeBackend(); });
    lifecycleActions.append(reload, suspend, recover, dispose);
    this.navigation.replaceChildren(title, list, lifecycleActions);
  }

  public showFailure(actions: PackageShellActions): void {
    const alert = document.createElement("p");
    alert.className = "error";
    alert.role = "alert";
    alert.textContent = "This Package interface failed.";
    const reload = actionButton("Reload UI", () => { void actions.reloadUi(); });
    const recover = actionButton("Recover Backend", () => { void actions.recoverBackend(); });
    this.mountTarget.replaceChildren(alert, reload, recover);
  }
}

function actionButton(label: string, action: () => void): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.textContent = label;
  button.addEventListener("click", action);
  return button;
}

export function createPackageShell(
  bridge: PackageSnapshotBridge,
  mounts: PackageMountController,
  navigation: HTMLElement,
  target: HTMLElement,
): PackageShell {
  return new PackageShell(bridge, mounts, new BrowserPackageShellView(navigation, target));
}
