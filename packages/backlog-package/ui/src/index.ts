import { adoptPackageStyles, installPackageUiKit, typedPackageClient } from "@teamarness/package-ui-sdk";
import backlogCss from "./styles.css?raw";
// Retain the CSS asset declared by the Package manifest; the Frame adopts the raw CSS below.
import "./styles.css";
import type { PackageMountContext, PackageUnmount } from "@teamarness/package-ui-sdk";
import type { BacklogContract } from "./contract.generated";
import { BacklogController } from "./controller";
import { BacklogView } from "./view";

export type { BacklogContract };

/** Mounts the Backlog: the Epic tree with its filters, and the selected Ticket. */
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
  let view: BacklogView | undefined;
  const controller = new BacklogController(client, () => {
    if (!disposed) view?.render();
  });
  try {
    view = new BacklogView(context.root, controller, context.package.displayName);
    view.render();
    // The Backend sends the current snapshot at once, then a new one after every change.
    const subscription = await client.subscribe("backlog.changes", {}, (snapshot) => {
      if (!disposed) controller.applySnapshot(snapshot);
    }, { onTerminated: () => {
      if (!disposed) controller.updatesStopped();
    } });

    return async () => {
      if (disposed) return;
      disposed = true;
      controller.dispose();
      view?.dispose();
      try {
        await subscription.close();
      } finally {
        cleanupStyles();
        cleanupKit();
      }
    };
  } catch (error) {
    disposed = true;
    controller.dispose();
    view?.dispose();
    cleanupStyles();
    cleanupKit();
    throw error;
  }
}
