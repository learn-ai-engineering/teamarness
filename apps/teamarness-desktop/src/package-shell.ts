import type { PackageHostSnapshot, PackageInstanceKey } from "./package-contracts";
import type { PackageMountController } from "./package-mount";

export type PackageShellActions = Readonly<{
  reloadUi(): Promise<void>;
  recoverBackend(): Promise<void>;
}>;

export interface PackageShellView {
  readonly mountTarget: unknown;
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

  public async refresh(): Promise<PackageHostSnapshot> {
    return this.bridge.snapshot();
  }

  public async select(instance: PackageInstanceKey): Promise<void> {
    this.selected = instance;
    await this.loadSelected();
  }

  public async hide(): Promise<void> {
    await this.mounts.unmount();
  }

  public async reloadUi(): Promise<void> {
    await this.loadSelected();
  }

  public async suspendBackend(): Promise<void> {
    await this.runLifecycle((instance) => this.mounts.suspendBackend(instance));
  }

  public async recoverBackend(): Promise<void> {
    await this.runLifecycle((instance) => this.mounts.recoverBackend(instance));
  }

  public async disposeBackend(): Promise<void> {
    await this.runLifecycle((instance) => this.mounts.disposeBackend(instance));
  }

  private recoveryActions(): PackageShellActions {
    return Object.freeze({
      reloadUi: async () => { await this.reloadUi(); },
      recoverBackend: async () => { await this.recoverBackend(); },
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
    if (failed) this.view.showFailure(this.recoveryActions());
  }

  private async loadSelected(): Promise<void> {
    if (!this.selected) return;
    let failureShown = false;
    const showFailure = (): void => {
      if (failureShown) return;
      failureShown = true;
      this.view.showFailure(this.recoveryActions());
    };
    try {
      await this.mounts.mount(this.selected, this.view.mountTarget, showFailure);
    } catch {
      showFailure();
    }
  }
}

export class BrowserPackageShellView implements PackageShellView {
  public constructor(public readonly mountTarget: HTMLElement) {}

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
  target: HTMLElement,
): PackageShell {
  return new PackageShell(bridge, mounts, new BrowserPackageShellView(target));
}
