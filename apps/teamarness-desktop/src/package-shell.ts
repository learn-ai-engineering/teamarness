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

export type PackageLifecycleFailure = "operation" | "refresh" | "operation-and-refresh";

export type PackageLifecycleResult = Readonly<{
  snapshot: PackageHostSnapshot | null;
  failure: PackageLifecycleFailure | null;
}>;

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

  public async suspendBackend(instance: PackageInstanceKey): Promise<PackageLifecycleResult> {
    return this.runLifecycle(instance, (target) => this.mounts.suspendBackend(target));
  }

  public async recoverBackend(instance: PackageInstanceKey): Promise<PackageLifecycleResult> {
    return this.runLifecycle(instance, (target) => this.mounts.recoverBackend(target));
  }

  public async disposeBackend(instance: PackageInstanceKey): Promise<PackageLifecycleResult> {
    return this.runLifecycle(instance, (target) => this.mounts.disposeBackend(target));
  }

  private recoveryActions(): PackageShellActions {
    return Object.freeze({
      reloadUi: async () => { await this.reloadUi(); },
      recoverBackend: async () => {
        if (this.selected) await this.recoverBackend(this.selected);
      },
    });
  }

  private async runLifecycle(
    instance: PackageInstanceKey,
    operation: (target: PackageInstanceKey) => Promise<void>,
  ): Promise<PackageLifecycleResult> {
    let operationFailed = false;
    try {
      await operation(instance);
    } catch {
      operationFailed = true;
    }
    let snapshot: PackageHostSnapshot | null = null;
    let refreshFailed = false;
    try {
      snapshot = await this.refresh();
    } catch {
      refreshFailed = true;
    }
    const failure = operationFailed && refreshFailed
      ? "operation-and-refresh"
      : operationFailed ? "operation" : refreshFailed ? "refresh" : null;
    return Object.freeze({ snapshot, failure });
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
