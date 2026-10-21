import { adoptPackageStyles, installPackageUiKit, typedPackageClient } from "@teamarness/package-ui-sdk";
import documentsCss from "./styles.css?raw";
// Retain the CSS asset declared by the Package manifest; the Frame adopts the raw CSS below.
import "./styles.css";
import type { PackageMountContext, PackageUnmount } from "@teamarness/package-ui-sdk";
import type { DocumentsContract, Snapshot } from "./contract.generated";

export type { DocumentsContract };
export type DocumentsSnapshot = Snapshot;

const AVAILABILITY_LABEL: Record<Snapshot["availability"], string> = {
  current: "Up to date",
  catching_up: "Catching up",
  offline: "Offline",
  waiting_for_history_access: "Waiting for history access",
};

/** Builds the Documents DOM once and patches the listing in place. */
class DocumentsView {
  private readonly page: HTMLElement;
  private readonly status: HTMLElement;
  private readonly body: HTMLElement;

  constructor(root: ShadowRoot, title: string) {
    const page = document.createElement("div");
    page.className = "teamarness-page documents-page";
    const header = document.createElement("header");
    header.className = "teamarness-page-header";
    const heading = document.createElement("h1");
    heading.className = "teamarness-page-title";
    heading.textContent = title;
    this.status = document.createElement("p");
    this.status.className = "availability";
    this.status.setAttribute("role", "status");
    header.append(heading, this.status);
    this.body = document.createElement("section");
    this.body.setAttribute("aria-label", "Documents");
    page.append(header, this.body);
    root.append(page);
    this.page = page;
  }

  dispose(): void {
    this.page.remove();
  }

  render(snapshot: Snapshot): void {
    this.status.textContent = AVAILABILITY_LABEL[snapshot.availability];
    this.body.replaceChildren();
    if (snapshot.documents.length === 0) {
      const empty = document.createElement("p");
      empty.className = "documents-empty";
      empty.textContent = "No documents yet.";
      this.body.append(empty);
    }
  }
}

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
  try {
    const view = new DocumentsView(context.root, context.package.displayName);
    view.render(await client.request("documents.snapshot", {}));
    const subscription = await client.subscribe("documents.changes", {}, (snapshot) => {
      if (!disposed) view.render(snapshot);
    });
    return async () => {
      if (disposed) return;
      disposed = true;
      view.dispose();
      try {
        await subscription.close();
      } finally {
        cleanupStyles();
        cleanupKit();
      }
    };
  } catch (error) {
    disposed = true;
    cleanupStyles();
    cleanupKit();
    throw error;
  }
}
