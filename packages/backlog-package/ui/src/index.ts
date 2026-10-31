import { adoptPackageStyles, installPackageUiKit } from "@teamarness/package-ui-sdk";
import backlogCss from "./styles.css?raw";
// Retain the CSS asset declared by the Package manifest; the Frame adopts the raw CSS below.
import "./styles.css";
import type { PackageMountContext, PackageUnmount } from "@teamarness/package-ui-sdk";
import type { BacklogContract } from "./contract.generated";

export type { BacklogContract };

/** Mounts the Backlog placeholder view. The Epic tree and Ticket detail replace it. */
export async function mount(context: PackageMountContext): Promise<PackageUnmount> {
  const cleanupKit = installPackageUiKit(context.root);
  let cleanupStyles: () => void;
  try {
    cleanupStyles = adoptPackageStyles(context.root, backlogCss);
  } catch (error) {
    cleanupKit();
    throw error;
  }

  const page = document.createElement("main");
  page.className = "backlog-page teamarness-page";
  const header = document.createElement("header");
  header.className = "teamarness-page-header";
  const title = document.createElement("h1");
  title.className = "teamarness-page-title";
  title.textContent = "Backlog";
  header.append(title);
  const placeholder = document.createElement("p");
  placeholder.className = "backlog-placeholder";
  placeholder.textContent = "Epics and Tickets will appear here.";
  page.append(header, placeholder);
  context.root.append(page);

  return async () => {
    page.remove();
    cleanupStyles();
    cleanupKit();
  };
}
