import { adoptPackageStyles, installPackageUiKit, typedPackageClient } from "@teamarness/package-ui-sdk";
import backlogCss from "./styles.css?raw";
// Retain the CSS asset declared by the Package manifest; the Frame adopts the raw CSS below.
import "./styles.css";
import type { PackageMountContext, PackageUnmount } from "@teamarness/package-ui-sdk";
import type { BacklogContract } from "./contract.generated";
import { BacklogController } from "./controller";
import { RememberedStateSync, loadRemembered } from "./remembered";
import { BacklogView } from "./view";

export type { BacklogContract };

/**
 * Mounts the Backlog: the Epic tree with its filters, and the selected Ticket with its attribution.
 * Selection, filters, expansion and drafts are restored from the Backend's remembered state.
 */
export async function mount(context: PackageMountContext): Promise<PackageUnmount> {
  const client = typedPackageClient<BacklogContract>(context.client);
  const cleanupKit = installPackageUiKit(context.root);
  let cleanupStyles: () => void;
  try {
    cleanupStyles = adoptPackageStyles(context.root, backlogCss);
  } catch (error) {
    cleanupKit();
    throw error;
  }
  let disposed = false;
  let nameRetry: ReturnType<typeof setTimeout> | undefined;
  let nameSubscription: Awaited<ReturnType<typeof client.subscribe>> | null = null;
  let nameRetryDelay = 1000;
  let view: BacklogView | undefined;
  // The sync reads the controller only when it sends, after the controller exists.
  const remembered: RememberedStateSync = new RememberedStateSync(client, () => controller.rememberedState());
  const controller: BacklogController = new BacklogController(client, () => {
    if (!disposed) view?.render();
  }, remembered);
  try {
    view = new BacklogView(context.root, controller, context.package.displayName);
    view.render();
    const state = await loadRemembered(client);
    if (state !== null && !disposed) controller.restore(state);
    // The Backend sends the current snapshot at once, then a new one after every change.
    const subscription = await client.subscribe("backlog.changes", {}, (snapshot) => {
      if (!disposed) controller.applySnapshot(snapshot);
    }, { onTerminated: () => {
      if (!disposed) controller.updatesStopped();
    } });

    // Names are optional presentation: a name stream failure must not prevent the Backlog.
    const scheduleNameRetry = () => {
      if (disposed || nameRetry !== undefined) return;
      nameSubscription = null;
      nameRetry = setTimeout(() => {
        nameRetry = undefined;
        void openNames();
      }, nameRetryDelay);
      nameRetryDelay = Math.min(nameRetryDelay * 2, 30_000);
    };
    const openNames = async (): Promise<void> => {
      let terminated = false;
      try {
        const opened = await client.subscribe("backlog.names", {}, (names) => {
          if (!disposed) {
            nameRetryDelay = 1000;
            controller.applyNames(names);
          }
        }, { onTerminated: () => {
          terminated = true;
          scheduleNameRetry();
        } });
        if (disposed || terminated) await opened.close();
        else nameSubscription = opened;
      } catch {
        scheduleNameRetry();
      }
    };
    await openNames();

    return async () => {
      if (disposed) return;
      disposed = true;
      if (nameRetry) clearTimeout(nameRetry);
      // Send a pending change before the controller stops, so a Package switch keeps the draft.
      await remembered.close();
      controller.dispose();
      view?.dispose();
      try {
        try {
          await subscription.close();
        } finally {
          await nameSubscription?.close();
        }
      } finally {
        cleanupStyles();
        cleanupKit();
      }
    };
  } catch (error) {
    disposed = true;
    if (nameRetry) clearTimeout(nameRetry);
    void remembered.close();
    controller.dispose();
    view?.dispose();
    cleanupStyles();
    cleanupKit();
    throw error;
  }
}
