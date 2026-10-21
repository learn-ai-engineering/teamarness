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
