import { adoptPackageStyles, installPackageUiKit, typedPackageClient } from "@teamarness/package-ui-sdk";
import exhibitsCss from "./styles.css?raw";
// Retain the CSS asset declared by the Package manifest; the Frame adopts the raw CSS below.
import "./styles.css";
import type { PackageMountContext, PackageSubscription, PackageUnmount } from "@teamarness/package-ui-sdk";
import type { Change, ExhibitsContract, Summary } from "./contract.generated";

export type { ExhibitsContract };

/**
 * The only sandbox the viewer frame gets: scripts run, nothing else. The content scheme also
 * sends a CSP `sandbox allow-scripts` directive, so the content stays opaque-origin even without
 * this attribute; the attribute keeps the frame's own flags explicit.
 */
export const VIEWER_SANDBOX = "allow-scripts";

const RETRY_LIMIT_MS = 30_000;

type ViewerState =
  | { readonly kind: "none" }
  | { readonly kind: "opening"; readonly exhibitId: string }
  | { readonly kind: "open"; readonly exhibitId: string; readonly revision: number; readonly url: string }
  | { readonly kind: "failed"; readonly exhibitId: string }
  | { readonly kind: "deleted"; readonly exhibitId: string };

type ListState =
  | { readonly kind: "loading" }
  | { readonly kind: "failed" }
  | { readonly kind: "ready"; readonly exhibits: readonly Summary[]; readonly nextBefore?: string };

function errorCode(error: unknown): string | undefined {
  if (typeof error !== "object" || error === null) return undefined;
  const { packageError, code } = error as { packageError?: { code?: unknown } | null; code?: unknown };
  if (typeof packageError?.code === "string") return packageError.code;
  return typeof code === "string" ? code : undefined;
}

function formatUpdated(updatedAt: string): string {
  const date = new Date(updatedAt);
  return Number.isNaN(date.getTime()) ? updatedAt : date.toLocaleString();
}

function element<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string, text?: string): HTMLElementTagNameMap[K] {
  const created = document.createElement(tag);
  if (className) created.className = className;
  if (text !== undefined) created.textContent = text;
  return created;
}

/**
 * Renders the list and the viewer. It owns the one viewer frame, which it creates with the
 * sandbox attribute set before any `src`, and reuses across revisions of the open Draft.
 *
 * The view installs no `message` listener: nothing an Exhibit posts can change Exhibits state.
 */
class ExhibitsView {
  private readonly page: HTMLElement;
  private readonly list: HTMLElement;
  private readonly viewer: HTMLElement;
  private frame: HTMLIFrameElement | null = null;

  constructor(
    root: ShadowRoot,
    title: string,
    private readonly actions: {
      readonly select: (exhibitId: string) => void;
      readonly retry: () => void;
      readonly older: () => void;
    },
  ) {
    const page = element("div", "teamarness-page exhibits-page");
    const header = element("header", "teamarness-page-header");
    header.append(element("h1", "teamarness-page-title", title));
    const body = element("div", "exhibits-body");
    this.list = element("nav", "exhibits-sidebar");
    this.list.setAttribute("aria-label", "Exhibits");
    this.viewer = element("section", "exhibits-viewer");
    this.viewer.setAttribute("aria-label", "Exhibit viewer");
    body.append(this.list, this.viewer);
    page.append(header, body);
    root.append(page);
    this.page = page;
  }

  dispose(): void {
    this.frame = null;
    this.page.remove();
  }

  renderList(state: ListState, selected: string | undefined): void {
    this.list.replaceChildren();
    if (state.kind === "loading") {
      this.list.append(element("p", "exhibits-empty", "Loading Exhibits…"));
      return;
    }
    if (state.kind === "failed") {
      const notice = element("div", "teamarness-notice teamarness-notice--danger exhibits-error");
      notice.setAttribute("role", "alert");
      notice.append(element("p", undefined, "Exhibits could not be loaded."));
      const retry = element("button", "teamarness-control exhibits-retry", "Try again");
      retry.type = "button";
      retry.addEventListener("click", () => this.actions.retry());
      notice.append(retry);
      this.list.append(notice);
      return;
    }
    if (state.exhibits.length === 0) {
      this.list.append(element("p", "exhibits-empty", "No Exhibits yet."));
      return;
    }
    const list = element("ul", "exhibits-list");
    for (const exhibit of state.exhibits) {
      const item = element("li");
      const button = element("button", "exhibits-item");
      button.type = "button";
      if (exhibit.id === selected) button.setAttribute("aria-current", "true");
      button.append(
        element("span", "exhibits-item-title", exhibit.title),
        element("span", "exhibits-item-meta", `Draft · ${formatUpdated(exhibit.updatedAt)}`),
      );
      button.addEventListener("click", () => this.actions.select(exhibit.id));
      item.append(button);
      list.append(item);
    }
    this.list.append(list);
    if (state.nextBefore !== undefined) {
      const older = element("button", "teamarness-control exhibits-older", "Show older Exhibits");
      older.type = "button";
      older.addEventListener("click", () => this.actions.older());
      this.list.append(older);
    }
  }

  renderViewer(state: ViewerState, title: string | undefined): void {
    if (state.kind === "open") {
      const frame = this.frame ?? this.createFrame();
      frame.setAttribute("title", `Exhibit: ${title ?? state.exhibitId}`);
      if (frame.getAttribute("src") !== state.url) frame.setAttribute("src", state.url);
      if (!frame.isConnected) this.viewer.replaceChildren(frame);
      return;
    }
    if (state.kind === "opening" && this.frame?.isConnected) return;
    this.frame = null;
    this.viewer.replaceChildren();
    if (state.kind === "none") {
      this.viewer.append(element("p", "exhibits-placeholder", "Select an Exhibit to view it."));
    } else if (state.kind === "opening") {
      this.viewer.append(element("p", "exhibits-placeholder", "Opening…"));
    } else if (state.kind === "deleted") {
      this.viewer.append(element("p", "exhibits-placeholder", "This Exhibit was deleted."));
    } else {
      const notice = element("div", "teamarness-notice teamarness-notice--danger", "This Exhibit could not be opened.");
      notice.setAttribute("role", "alert");
      this.viewer.append(notice);
    }
  }

  private createFrame(): HTMLIFrameElement {
    const frame = element("iframe", "exhibits-frame");
    // The sandbox is set before the first `src`, so the very first load is already sandboxed.
    frame.setAttribute("sandbox", VIEWER_SANDBOX);
    frame.setAttribute("referrerpolicy", "no-referrer");
    this.frame = frame;
    return frame;
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
  let list: ListState = { kind: "loading" };
  let viewer: ViewerState = { kind: "none" };
  let selected: string | undefined;
  let listGeneration = 0;
  let openGeneration = 0;
  let pendingOpen: string | undefined;
  let subscription: PackageSubscription | null = null;
  let retryTimer: ReturnType<typeof setTimeout> | undefined;
  let retryDelay = 0;

  const titleOf = (exhibitId: string): string | undefined =>
    list.kind === "ready" ? list.exhibits.find((exhibit) => exhibit.id === exhibitId)?.title : undefined;

  const render = () => {
    if (disposed) return;
    view.renderList(list, selected);
    view.renderViewer(viewer, selected === undefined ? undefined : titleOf(selected));
  };

  /** Opens a fresh view of `exhibitId`; only the latest open request may change the viewer. */
  const open = async (exhibitId: string): Promise<void> => {
    const generation = ++openGeneration;
    pendingOpen = exhibitId;
    if (viewer.kind !== "open" || viewer.exhibitId !== exhibitId) viewer = { kind: "opening", exhibitId };
    render();
    try {
      const opened = await client.request("exhibit.open", { exhibitId });
      if (disposed || generation !== openGeneration) return;
      pendingOpen = undefined;
      viewer = { kind: "open", exhibitId, revision: opened.revision, url: opened.url };
    } catch (error) {
      if (disposed || generation !== openGeneration) return;
      pendingOpen = undefined;
      viewer = errorCode(error) === "exhibits.not_found" && viewer.kind === "open"
        ? { kind: "deleted", exhibitId }
        : { kind: "failed", exhibitId };
    }
    render();
  };

  const select = (exhibitId: string) => {
    selected = exhibitId;
    void open(exhibitId);
  };

  /** Re-reads the first page; when the open Draft moved past the shown revision, re-opens it. */
  const refresh = async (): Promise<void> => {
    const generation = ++listGeneration;
    try {
      const page = await client.request("exhibit.list", {});
      if (disposed || generation !== listGeneration) return;
      list = { kind: "ready", exhibits: page.exhibits, nextBefore: page.nextBefore };
      if (viewer.kind === "open") {
        const shown = viewer;
        const listed = page.exhibits.find((exhibit) => exhibit.id === shown.exhibitId);
        if (listed && listed.revision > shown.revision && pendingOpen !== shown.exhibitId) void open(shown.exhibitId);
      }
    } catch {
      if (disposed || generation !== listGeneration) return;
      list = { kind: "failed" };
    }
    render();
  };

  const older = async (): Promise<void> => {
    if (list.kind !== "ready" || list.nextBefore === undefined) return;
    const current = list;
    const generation = ++listGeneration;
    try {
      const page = await client.request("exhibit.list", { before: current.nextBefore });
      if (disposed || generation !== listGeneration) return;
      list = { kind: "ready", exhibits: [...current.exhibits, ...page.exhibits], nextBefore: page.nextBefore };
    } catch {
      if (disposed || generation !== listGeneration) return;
      list = { kind: "failed" };
    }
    render();
  };

  const onChange = (change: Change) => {
    if (disposed) return;
    if (selected === change.exhibitId) {
      if (change.deleted) {
        ++openGeneration;
        pendingOpen = undefined;
        viewer = { kind: "deleted", exhibitId: change.exhibitId };
        render();
      } else if (viewer.kind !== "open" || change.revision > viewer.revision) {
        void open(change.exhibitId);
      }
    }
    void refresh();
  };

  /**
   * Follows `exhibits.changed`. A stream that ends (for example a subscriber left too far
   * behind) is reopened at once and the list re-read; a failing reopen backs off.
   */
  const follow = async (): Promise<void> => {
    let terminated = false;
    try {
      const opened = await client.subscribe("exhibits.changed", {}, onChange, {
        onTerminated: () => {
          terminated = true;
          subscription = null;
          scheduleFollow();
        },
      });
      if (disposed || terminated) {
        await opened.close();
        return;
      }
      subscription = opened;
      retryDelay = 0;
    } catch {
      scheduleFollow();
    }
  };

  const scheduleFollow = () => {
    if (disposed || retryTimer !== undefined) return;
    retryTimer = setTimeout(() => {
      retryTimer = undefined;
      void follow().then(() => (disposed ? undefined : refresh()));
    }, retryDelay);
    retryDelay = Math.min(Math.max(retryDelay * 2, 1_000), RETRY_LIMIT_MS);
  };

  const view = new ExhibitsView(context.root, context.package.displayName, {
    select,
    retry: () => void refresh(),
    older: () => void older(),
  });

  try {
    render();
    // Subscribe before the first list, so a change in between is not missed.
    await follow();
    await refresh();
  } catch (error) {
    disposed = true;
    if (retryTimer) clearTimeout(retryTimer);
    view.dispose();
    cleanupStyles();
    cleanupKit();
    throw error;
  }

  return async () => {
    if (disposed) return;
    disposed = true;
    if (retryTimer) clearTimeout(retryTimer);
    view.dispose();
    try {
      await subscription?.close();
    } finally {
      cleanupStyles();
      cleanupKit();
    }
  };
}
