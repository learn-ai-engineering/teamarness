import type {
  PackageHostSnapshot,
  PackageHostStateNotification,
  PackageInstanceKey,
  PackageUiView,
} from "./package-contracts";
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
  subscribeHostState(
    afterRevision: number,
    listener: (event: PackageHostStateNotification) => void,
  ): Promise<void>;
}

export type PackageSnapshotObserver = Readonly<{
  update(snapshot: PackageHostSnapshot): void;
  fail(): void;
}>;

export type PackageLifecycleFailure = "operation" | "refresh" | "operation-and-refresh";

export type PackageLifecycleResult = Readonly<{
  snapshot: PackageHostSnapshot | null;
  failure: PackageLifecycleFailure | null;
}>;

interface MountController {
  mount(instance: PackageInstanceKey, target: unknown, onFailure: () => void, view: PackageUiView): Promise<unknown>;
  unmount(): Promise<void>;
  suspendBackend(instance: PackageInstanceKey): Promise<void>;
  recoverBackend(instance: PackageInstanceKey): Promise<void>;
  disposeBackend(instance: PackageInstanceKey): Promise<void>;
}

export class PackageShell {
  private selected: PackageInstanceKey | null = null;
  private selectedView: PackageUiView = "main";
  private acceptedSnapshot: PackageHostSnapshot | null = null;
  private snapshotObserver: PackageSnapshotObserver | null = null;
  private snapshotStartup: Promise<void> | null = null;
  private latestSnapshotRequest = 0;
  private subscriptionEstablished = false;
  private subscriptionAttempt: Promise<void> | null = null;
  private notificationRefreshRunning = false;
  private pendingNotificationRevision = 0;
  private pendingRevisionGap = false;

  public constructor(
    private readonly bridge: PackageSnapshotBridge,
    private readonly mounts: MountController,
    private readonly view: PackageShellView,
  ) {}

  public startSnapshotUpdates(observer: PackageSnapshotObserver): Promise<void> {
    this.snapshotObserver = observer;
    if (this.snapshotStartup) return this.snapshotStartup;
    const startup = this.refresh().then(() => undefined);
    this.snapshotStartup = startup;
    void startup.catch(() => {
      if (this.snapshotStartup === startup) this.snapshotStartup = null;
    });
    return startup;
  }

  public async refresh(): Promise<PackageHostSnapshot> {
    const request = ++this.latestSnapshotRequest;
    const acceptedRevisionAtStart = this.acceptedSnapshot?.revision ?? -1;
    let snapshot: PackageHostSnapshot;
    try {
      snapshot = await this.bridge.snapshot();
    } catch (error) {
      const accepted = this.acceptedSnapshot;
      if (
        accepted
        && (
          request !== this.latestSnapshotRequest
          || accepted.revision > acceptedRevisionAtStart
        )
      ) {
        await this.ensureHostStateSubscription(accepted.revision);
        return accepted;
      }
      if (request === this.latestSnapshotRequest) this.snapshotObserver?.fail();
      throw error;
    }
    if (!this.acceptedSnapshot || snapshot.revision > this.acceptedSnapshot.revision) {
      this.acceptedSnapshot = snapshot;
      this.snapshotObserver?.update(snapshot);
    }
    await this.ensureHostStateSubscription(this.acceptedSnapshot.revision);
    return this.acceptedSnapshot;
  }

  public async select(instance: PackageInstanceKey, view: PackageUiView = "main"): Promise<void> {
    this.selected = instance;
    this.selectedView = view;
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

  private ensureHostStateSubscription(afterRevision: number): Promise<void> {
    if (!this.snapshotObserver || this.subscriptionEstablished) return Promise.resolve();
    if (this.subscriptionAttempt) return this.subscriptionAttempt;
    const attempt = this.bridge.subscribeHostState(afterRevision, (event) => {
      this.queueNotificationRefresh(event);
    }).then(
      () => { this.subscriptionEstablished = true; },
      (error: unknown) => {
        this.snapshotObserver?.fail();
        throw error;
      },
    );
    this.subscriptionAttempt = attempt;
    void attempt.then(
      () => { if (this.subscriptionAttempt === attempt) this.subscriptionAttempt = null; },
      () => { if (this.subscriptionAttempt === attempt) this.subscriptionAttempt = null; },
    );
    return attempt;
  }

  private queueNotificationRefresh(event: PackageHostStateNotification): void {
    if (event.revision <= (this.acceptedSnapshot?.revision ?? 0) && !event.revisionGap) return;
    this.pendingNotificationRevision = Math.max(this.pendingNotificationRevision, event.revision);
    this.pendingRevisionGap ||= event.revisionGap;
    if (this.notificationRefreshRunning) return;
    this.notificationRefreshRunning = true;
    void this.runNotificationRefreshes();
  }

  private async runNotificationRefreshes(): Promise<void> {
    try {
      while (
        this.pendingRevisionGap
        || this.pendingNotificationRevision > (this.acceptedSnapshot?.revision ?? 0)
      ) {
        this.pendingNotificationRevision = 0;
        this.pendingRevisionGap = false;
        try {
          await this.refresh();
        } catch {
          // The observer owns refresh failure presentation. A later notification retries.
        }
      }
    } finally {
      this.notificationRefreshRunning = false;
      if (
        this.pendingRevisionGap
        || this.pendingNotificationRevision > (this.acceptedSnapshot?.revision ?? 0)
      ) {
        this.queueNotificationRefresh({
          revision: this.pendingNotificationRevision,
          revisionGap: this.pendingRevisionGap,
        });
      }
    }
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
      await this.mounts.mount(this.selected, this.view.mountTarget, showFailure, this.selectedView);
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
