import { adoptPackageStyles, installPackageUiKit, typedPackageClient } from "@teamarness/package-ui-sdk";
import exhibitsCss from "./styles.css?raw";
// Retain the CSS asset declared by the Package manifest; the Frame adopts the raw CSS below.
import "./styles.css";
import type { PackageMountContext, PackageUnmount } from "@teamarness/package-ui-sdk";
import type { ExhibitsContract, Page } from "./contract.generated";

export type { ExhibitsContract };

/**
 * Placeholder Exhibits UI: it lists Drafts newest first and refreshes on `exhibits.changed`.
 * Viewing a Draft in a nested content frame is the next step; this mount only proves the
 * Package builds, admits and answers its UI contract.
 */
class ExhibitsView {
  private readonly page: HTMLElement;
  private readonly body: HTMLElement;

  constructor(root: ShadowRoot, title: string) {
    const page = document.createElement("div");
    page.className = "teamarness-page exhibits-page";
    const header = document.createElement("header");
    header.className = "teamarness-page-header";
    const heading = document.createElement("h1");
    heading.className = "teamarness-page-title";
    heading.textContent = title;
    header.append(heading);
    this.body = document.createElement("section");
    this.body.setAttribute("aria-label", "Exhibits");
    page.append(header, this.body);
    root.append(page);
    this.page = page;
  }

  dispose(): void {
    this.page.remove();
  }

  render(listing: Page): void {
    this.body.replaceChildren();
    if (listing.exhibits.length === 0) {
      const empty = document.createElement("p");
      empty.className = "exhibits-empty";
      empty.textContent = "No Exhibits yet.";
      this.body.append(empty);
      return;
    }
    const list = document.createElement("ul");
    list.className = "exhibits-list";
    for (const exhibit of listing.exhibits) {
      const item = document.createElement("li");
      item.textContent = exhibit.title;
      list.append(item);
    }
    this.body.append(list);
  }
}

export async function mount(context: PackageMountContext): Promise<PackageUnmount> {
  const client = typedPackageClient<ExhibitsContract>(context.client);
  const cleanupKit = installPackageUiKit(context.root);
  let cleanupStyles: () => void;
  try {
    cleanupStyles = adoptPackageStyles(context.root, exhibitsCss);
  } catch (error) {
    cleanupKit();
    throw error;
  }
  let disposed = false;
  try {
    const view = new ExhibitsView(context.root, context.package.displayName);
    const refresh = async () => {
      const listing = await client.request("exhibit.list", {});
      if (!disposed) view.render(listing);
    };
    await refresh();
    const subscription = await client.subscribe("exhibits.changed", {}, () => {
      if (!disposed) void refresh().catch(() => undefined);
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
