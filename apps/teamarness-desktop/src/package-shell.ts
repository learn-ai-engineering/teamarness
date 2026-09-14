import type { PackageHostSnapshot, PackageInstanceKey } from "./package-contracts";
import type { PackageMountController } from "./package-mount";

export type PackageShellActions = Readonly<{
  select(instance: PackageInstanceKey): Promise<void>;
  reloadUi(): Promise<void>;
  recoverBackend(): Promise<void>;
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
  mount(instance: PackageInstanceKey, target: unknown): Promise<unknown>;
  unmount(): Promise<void>;
  recoverBackend(instance: PackageInstanceKey): Promise<void>;
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
      recoverBackend: async () => {
        if (this.selected) await this.mounts.recoverBackend(this.selected);
      },
    });
  }

  private async loadSelected(): Promise<void> {
    if (!this.selected) return;
    try {
      await this.mounts.mount(this.selected, this.view.mountTarget);
    } catch {
      this.view.showFailure(this.actions());
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
      button.textContent = `${catalog?.displayName ?? instance.key.packageIdentity} · ${instance.key.workspaceId}`;
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
    const reload = document.createElement("button");
    reload.type = "button";
    reload.textContent = "Reload UI";
    reload.addEventListener("click", () => { void actions.reloadUi(); });
    const recover = document.createElement("button");
    recover.type = "button";
    recover.textContent = "Recover Backend";
    recover.addEventListener("click", () => { void actions.recoverBackend(); });
    lifecycleActions.append(reload, recover);
    this.navigation.replaceChildren(title, list, lifecycleActions);
  }

  public showFailure(actions: PackageShellActions): void {
    const alert = document.createElement("p");
    alert.className = "error";
    alert.role = "alert";
    alert.textContent = "This Package interface failed.";
    const reload = document.createElement("button");
    reload.type = "button";
    reload.textContent = "Reload UI";
    reload.addEventListener("click", () => { void actions.reloadUi(); });
    const recover = document.createElement("button");
    recover.type = "button";
    recover.textContent = "Recover Backend";
    recover.addEventListener("click", () => { void actions.recoverBackend(); });
    this.mountTarget.replaceChildren(alert, reload, recover);
  }
}

export function createPackageShell(
  bridge: PackageSnapshotBridge,
  mounts: PackageMountController,
  navigation: HTMLElement,
  target: HTMLElement,
): PackageShell {
  return new PackageShell(bridge, mounts, new BrowserPackageShellView(navigation, target));
}
