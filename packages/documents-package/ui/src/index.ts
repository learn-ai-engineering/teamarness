import { adoptPackageStyles, installPackageUiKit, typedPackageClient } from "@teamarness/package-ui-sdk";
import documentsCss from "./styles.css?raw";
// Retain the CSS asset declared by the Package manifest; the Frame adopts the raw CSS below.
import "./styles.css";
import type { PackageMountContext, PackageUnmount } from "@teamarness/package-ui-sdk";
import type { Collection, Document, DocumentsContract, Snapshot } from "./contract.generated";
import { DocumentsController } from "./controller";
import { DocumentsView } from "./view";

export type { DocumentsContract };
export type DocumentsSnapshot = Snapshot;
export type DocumentsCollection = Collection;
export type DocumentsDocument = Document;

export async function mount(context: PackageMountContext): Promise<PackageUnmount> {
  const client = typedPackageClient<DocumentsContract>(context.client);
  const cleanupKit = installPackageUiKit(context.root);
  let cleanupStyles: () => void;
  try {
    cleanupStyles = adoptPackageStyles(context.root, documentsCss);
  } catch (error) {
    cleanupKit();
    throw error;
  }
  let disposed = false;
  let nameRetry: ReturnType<typeof setTimeout> | undefined;
  let nameSubscription: Awaited<ReturnType<typeof client.subscribe>> | null = null;
  let nameRetryDelay = 1000;
  let view: DocumentsView | undefined;
  const controller = new DocumentsController(client, () => {
    if (!disposed) view?.render();
  });
  try {
    view = new DocumentsView(context.root, controller, context.package.displayName);
    await controller.initialize();
    view.render();
    const subscription = await client.subscribe("documents.changes", {}, (snapshot) => {
      if (!disposed) controller.applySnapshot(snapshot);
    });

    // Names are optional presentation: a name stream failure must not prevent documents.
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
        const opened = await client.subscribe("documents.names", {}, (names) => {
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
    controller.dispose();
    view?.dispose();
    cleanupStyles();
    cleanupKit();
    throw error;
  }
}
