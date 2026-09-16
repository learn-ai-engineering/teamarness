import type {
  PackageClient,
  PackageMountContext,
  PackageUnmount,
} from "@teamarness/package-ui-sdk";

// ─── Types ───────────────────────────────────────────────────────────────────

export type Registration = Readonly<{
  id: string;
  path: string;
  addedAt: string;
}>;

export type Head = Readonly<
  | { kind: "branch"; name: string }
  | { kind: "detached"; commit: string }
>;

export type Snapshot = Readonly<{
  registrationId: string;
  path: string;
  originRemote: string | null;
  githubUrl: string | null;
  head: Head;
  commit: string;
  isClean: boolean;
  fetchedAt: string;
}>;

type ListResponse = Readonly<{
  registrations: readonly Registration[];
}>;

type RegisterResponse = Readonly<{
  registration: Registration;
}>;

type SnapshotResponse = Readonly<{
  registrationId: string;
  snapshot: Snapshot;
}>;

type PendingOp = "none" | "register" | "snapshot" | "remove";

export type RepoAwareState = Readonly<{
  registrations: readonly Registration[];
  selectedRegistrationId: string | null;
  snapshot: Snapshot | null;
  error: string | undefined;
  pending: PendingOp;
  showRemoveConfirm: boolean;
}>;

export type RepoAwareController = Readonly<{
  start(): Promise<void>;
  register(path: string): Promise<void>;
  selectRegistration(id: string): Promise<void>;
  refresh(): Promise<void>;
  requestRemove(): void;
  confirmRemove(): Promise<void>;
  cancelRemove(): void;
  dispose(): void;
}>;

// ─── Controller ──────────────────────────────────────────────────────────────

export function createRepoAwareController(
  client: PackageClient,
  present: (state: RepoAwareState) => void,
): RepoAwareController {
  let disposed = false;
  let state: RepoAwareState = {
    registrations: [],
    selectedRegistrationId: null,
    snapshot: null,
    error: undefined,
    pending: "none",
    showRemoveConfirm: false,
  };

  const update = (changes: Partial<RepoAwareState>): void => {
    if (disposed) return;
    state = { ...state, ...changes };
    present(state);
  };

  const fail = (error: unknown): void => {
    update({
      error: error instanceof Error ? error.message : "Request failed",
      pending: "none",
    });
  };

  const loadList = async (): Promise<readonly Registration[]> => {
    const result = await client.request("repo_aware.list", {}) as ListResponse;
    return result.registrations;
  };

  const loadSnapshot = async (id: string): Promise<Snapshot> => {
    update({ pending: "snapshot" });
    const result = await client.request("repo_aware.snapshot", { registrationId: id }) as SnapshotResponse;
    return result.snapshot;
  };

  return Object.freeze({
    async start(): Promise<void> {
      if (disposed) return;
      try {
        const registrations = await loadList();
        let selectedRegistrationId: string | null = null;
        let snapshot: Snapshot | null = null;
        if (registrations.length > 0) {
          const first = registrations[0];
          if (first) {
            selectedRegistrationId = first.id;
            snapshot = await loadSnapshot(first.id);
          }
        }
        update({ registrations, selectedRegistrationId, snapshot, error: undefined, pending: "none" });
      } catch (error) {
        fail(error);
      }
    },

    async register(path: string): Promise<void> {
      if (disposed) return;
      update({ pending: "register", error: undefined });
      try {
        await client.request("repo_aware.register", { path }) as RegisterResponse;
        const registrations = await loadList();
        let selectedRegistrationId: string | null = null;
        let snapshot: Snapshot | null = null;
        if (registrations.length > 0) {
          const last = registrations[registrations.length - 1];
          if (last) {
            selectedRegistrationId = last.id;
            snapshot = await loadSnapshot(last.id);
          }
        }
        update({ registrations, selectedRegistrationId, snapshot, pending: "none", error: undefined });
      } catch (error) {
        fail(error);
      }
    },

    async selectRegistration(id: string): Promise<void> {
      if (disposed) return;
      update({ selectedRegistrationId: id, error: undefined });
      try {
        const snapshot = await loadSnapshot(id);
        update({ snapshot, pending: "none", error: undefined });
      } catch (error) {
        fail(error);
      }
    },

    async refresh(): Promise<void> {
      if (disposed || !state.selectedRegistrationId) return;
      try {
        const snapshot = await loadSnapshot(state.selectedRegistrationId);
        update({ snapshot, pending: "none", error: undefined });
      } catch (error) {
        fail(error);
      }
    },

    requestRemove(): void {
      if (disposed) return;
      update({ showRemoveConfirm: true });
    },

    async confirmRemove(): Promise<void> {
      if (disposed || !state.selectedRegistrationId) return;
      update({ pending: "remove", showRemoveConfirm: false, error: undefined });
      try {
        await client.request("repo_aware.remove", {
          registrationId: state.selectedRegistrationId,
        });
        const registrations = await loadList();
        let selectedRegistrationId: string | null = null;
        let snapshot: Snapshot | null = null;
        if (registrations.length > 0) {
          const first = registrations[0];
          if (first) {
            selectedRegistrationId = first.id;
            snapshot = await loadSnapshot(first.id);
          }
        }
        update({ registrations, selectedRegistrationId, snapshot, pending: "none", error: undefined });
      } catch (error) {
        fail(error);
      }
    },

    cancelRemove(): void {
      if (disposed) return;
      update({ showRemoveConfirm: false });
    },

    dispose(): void {
      disposed = true;
    },
  });
}

// ─── Styles ──────────────────────────────────────────────────────────────────

const REPO_AWARE_CSS = `
  :host { color: var(--teamarness-color-text); font-family: var(--teamarness-font-body); }
  article { display: grid; gap: .9rem; padding: 1rem; }
  h2, h3, p { margin: 0; }
  .controls { display: flex; flex-wrap: wrap; gap: .7rem; align-items: center; }
  input, button { font: inherit; border-radius: var(--teamarness-radius-control); padding: .45rem .65rem; }
  input { border: 1px solid var(--teamarness-color-border, #ccc); flex: 1; min-width: 12rem; }
  button { border: 0; background: var(--teamarness-color-accent); color: var(--teamarness-color-background); cursor: pointer; }
  button:disabled { opacity: .5; cursor: default; }
  .error { color: var(--teamarness-color-danger, #b42318); min-height: 1.2em; }
  .selector { display: flex; flex-direction: column; gap: .3rem; }
  .selector button { text-align: left; background: transparent; color: inherit; border: 1px solid var(--teamarness-color-border, #ccc); border-radius: var(--teamarness-radius-control); padding: .45rem .65rem; }
  .selector button[aria-selected="true"] { border-color: var(--teamarness-color-accent); background: color-mix(in srgb, var(--teamarness-color-accent) 10%, transparent); }
  .card { display: grid; gap: .3rem; padding: .7rem; border: 1px solid var(--teamarness-color-border, #ccc); border-radius: var(--teamarness-radius-control); }
  .card .label { font-weight: 600; }
  .badge { display: inline-block; padding: .1rem .45rem; border-radius: .25rem; font-size: .85em; font-weight: 600; }
  .badge.clean { background: #ecfdf5; color: #065f46; }
  .badge.dirty { background: #fef2f2; color: #991b1b; }
  .actions { display: flex; gap: .5rem; }
  .overlay { position: fixed; inset: 0; display: grid; place-items: center; background: rgba(0,0,0,.3); z-index: 100; }
  .dialog { background: var(--teamarness-color-background, #fff); padding: 1rem; border-radius: var(--teamarness-radius-control); display: grid; gap: .7rem; max-width: 24rem; }
  .dialog .buttons { display: flex; gap: .5rem; justify-content: end; }
  .link { color: var(--teamarness-color-accent); text-decoration: underline; }
`;

function installStyles(root: ShadowRoot): () => void {
  const StyleSheet = globalThis.CSSStyleSheet;
  if (typeof StyleSheet !== "function") {
    throw new Error("The Repo-aware Package requires constructed CSSStyleSheet support");
  }
  if (!("adoptedStyleSheets" in root) || root.adoptedStyleSheets === undefined) {
    throw new Error("The Repo-aware Package requires ShadowRoot.adoptedStyleSheets support");
  }
  let sheet: CSSStyleSheet;
  try {
    sheet = new StyleSheet();
  } catch (cause) {
    throw new Error("The Repo-aware Package requires constructed CSSStyleSheet support", { cause });
  }
  if (typeof sheet.replaceSync !== "function") {
    throw new Error("The Repo-aware Package requires CSSStyleSheet.replaceSync support");
  }
  sheet.replaceSync(REPO_AWARE_CSS);
  try {
    root.adoptedStyleSheets = [...root.adoptedStyleSheets, sheet];
  } catch (cause) {
    throw new Error("The Repo-aware Package requires mutable ShadowRoot.adoptedStyleSheets support", { cause });
  }
  if (!root.adoptedStyleSheets.includes(sheet)) {
    throw new Error("The Repo-aware Package requires mutable ShadowRoot.adoptedStyleSheets support");
  }

  let cleaned = false;
  return () => {
    if (cleaned) return;
    cleaned = true;
    root.adoptedStyleSheets = root.adoptedStyleSheets.filter((candidate) => candidate !== sheet);
  };
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

function button(text: string): HTMLButtonElement {
  const element = document.createElement("button");
  element.type = "button";
  element.textContent = text;
  return element;
}

function abbreviateCommit(hash: string): string {
  if (hash.length === 40) return hash.slice(0, 7);
  if (hash.length === 64) return hash.slice(0, 11);
  return hash.slice(0, 7);
}

// ─── Mount ───────────────────────────────────────────────────────────────────

export async function mount(context: PackageMountContext): Promise<PackageUnmount> {
  const cleanupStyles = installStyles(context.root);
  const article = document.createElement("article");
  const heading = document.createElement("h2");
  heading.textContent = context.package.displayName;
  article.append(heading);

  const onboardingSection = document.createElement("div");
  onboardingSection.style.display = "none";
  const onboardingDesc = document.createElement("p");
  onboardingDesc.textContent = "Register a Git repository to get started.";
  const inputRow = document.createElement("div");
  inputRow.className = "controls";
  const pathInput = document.createElement("input");
  pathInput.type = "text";
  pathInput.setAttribute("aria-label", "Repository path");
  pathInput.placeholder = "/absolute/path/to/repo";
  const addButton = button("Add");
  // Starts disabled — empty input. The input listener toggles this.
  addButton.disabled = true;
  addButton.dataset.pendingLocked = "false";

  const onInput = (): void => {
    const empty = !pathInput.value.trim();
    addButton.disabled = addButton.dataset.pendingLocked === "true" || empty;
  };
  pathInput.addEventListener("input", onInput);
  onboardingSection.append(onboardingDesc, inputRow);
  inputRow.append(pathInput, addButton);

  const contentSection = document.createElement("div");
  contentSection.style.display = "none";

  const col = document.createElement("div");
  col.className = "selector";
  const selectorLabel = document.createElement("h3");
  selectorLabel.textContent = "Repositories";
  col.append(selectorLabel);
  const selectorList = document.createElement("div");
  selectorList.className = "selector";
  col.append(selectorList);
  contentSection.append(col);

  const card = document.createElement("div");
  card.className = "card";
  card.style.display = "none";
  const pathRow = document.createElement("p");
  const originRow = document.createElement("p");
  const githubRow = document.createElement("p");
  const headRow = document.createElement("p");
  const commitRow = document.createElement("p");
  const statusRow = document.createElement("p");
  const fetchedRow = document.createElement("p");
  const actionRow = document.createElement("div");
  actionRow.className = "actions";
  const refreshButton = button("Refresh");
  const removeButton = button("Remove");
  actionRow.append(refreshButton, removeButton);
  card.append(pathRow, originRow, githubRow, headRow, commitRow, statusRow, fetchedRow, actionRow);
  contentSection.append(card);

  // Confirmation overlay
  const overlay = document.createElement("div");
  overlay.className = "overlay";
  overlay.style.display = "none";
  const dialog = document.createElement("div");
  dialog.className = "dialog";
  const dialogText = document.createElement("p");
  dialogText.textContent = "Remove this registration? The checkout directory will not be deleted.";
  const dialogButtons = document.createElement("div");
  dialogButtons.className = "buttons";
  const cancelRemoveBtn = button("Cancel");
  const confirmRemoveBtn = button("Remove");
  dialogButtons.append(cancelRemoveBtn, confirmRemoveBtn);
  dialog.append(dialogText, dialogButtons);
  overlay.append(dialog);

  const errorMessage = document.createElement("p");
  errorMessage.className = "error";
  errorMessage.setAttribute("role", "status");

  article.append(onboardingSection, contentSection, overlay, errorMessage);
  context.root.append(article);

  // ─── Render function ─────────────────────────────────────────────────────

  const render = (state: RepoAwareState): void => {
    const hasRegistrations = state.registrations.length > 0;
    onboardingSection.style.display = hasRegistrations ? "none" : "";
    contentSection.style.display = hasRegistrations ? "" : "none";

    errorMessage.textContent = state.error ?? "";

    // Update selector buttons
    selectorList.replaceChildren();
    for (const reg of state.registrations) {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.textContent = reg.path;
      btn.setAttribute("aria-selected", String(reg.id === state.selectedRegistrationId));
      btn.addEventListener("click", () => { void controller.selectRegistration(reg.id); });
      selectorList.append(btn);
    }

    // Update snapshot card
    if (state.snapshot && state.selectedRegistrationId) {
      card.style.display = "";
      const s = state.snapshot;

      const pathLabel = document.createElement("span");
      pathLabel.className = "label";
      pathLabel.textContent = "Path: ";
      pathRow.replaceChildren(pathLabel, document.createTextNode(s.path));

      const originLabel = document.createElement("span");
      originLabel.className = "label";
      originLabel.textContent = "Origin: ";
      originRow.replaceChildren(originLabel, document.createTextNode(s.originRemote ?? "No remote"));

      if (s.githubUrl) {
        githubRow.replaceChildren();
        const ghLabel = document.createElement("span");
        ghLabel.className = "label";
        ghLabel.textContent = "GitHub: ";
        const ghLink = document.createElement("a");
        ghLink.href = s.githubUrl;
        ghLink.target = "_blank";
        ghLink.rel = "noopener noreferrer";
        ghLink.className = "link";
        ghLink.textContent = s.githubUrl;
        githubRow.append(ghLabel, ghLink);
      } else {
        const ghLabel = document.createElement("span");
        ghLabel.className = "label";
        ghLabel.textContent = "GitHub: ";
        githubRow.replaceChildren(ghLabel, document.createTextNode("—"));
      }

      const headLabel = document.createElement("span");
      headLabel.className = "label";
      headLabel.textContent = "Head: ";
      if (s.head.kind === "branch") {
        headRow.replaceChildren(headLabel, document.createTextNode(s.head.name));
      } else {
        headRow.replaceChildren(headLabel, document.createTextNode(`Detached: ${abbreviateCommit(s.head.commit)}`));
      }

      const commitLabel = document.createElement("span");
      commitLabel.className = "label";
      commitLabel.textContent = "Commit: ";
      commitRow.replaceChildren(commitLabel, document.createTextNode(s.commit));

      const statusLabel = document.createElement("span");
      statusLabel.className = "label";
      statusLabel.textContent = "Status: ";
      const badge = document.createElement("span");
      badge.className = `badge ${s.isClean ? "clean" : "dirty"}`;
      badge.textContent = s.isClean ? "Clean" : "Dirty";
      statusRow.replaceChildren(statusLabel, badge);

      const fetchedLabel = document.createElement("span");
      fetchedLabel.className = "label";
      fetchedLabel.textContent = "Fetched at: ";
      fetchedRow.replaceChildren(fetchedLabel, document.createTextNode(s.fetchedAt));
    } else {
      card.style.display = "none";
    }

    // Disable during pending operations. Render runs on state changes, so we
    // also snapshot the pending state in a dataset that the input listener reads
    // to combine with input emptiness between renders.
    const pendingLocked = state.pending !== "none";
    addButton.dataset.pendingLocked = String(pendingLocked);
    addButton.disabled = pendingLocked || !pathInput.value.trim();
    refreshButton.disabled = state.pending !== "none";
    removeButton.disabled = state.pending !== "none";

    // Confirmation overlay
    overlay.style.display = state.showRemoveConfirm ? "" : "none";
  };

  const controller = createRepoAwareController(context.client, render);

  // ─── Event handlers ──────────────────────────────────────────────────────

  const onAdd = (): void => {
    const value = pathInput.value.trim();
    if (value) {
      void controller.register(value);
      pathInput.value = "";
    }
  };
  addButton.addEventListener("click", onAdd);
  pathInput.addEventListener("keydown", (e) => {
    if (e.key === "Enter") onAdd();
  });
  refreshButton.addEventListener("click", () => { void controller.refresh(); });
  removeButton.addEventListener("click", () => { controller.requestRemove(); });
  cancelRemoveBtn.addEventListener("click", () => { controller.cancelRemove(); });
  confirmRemoveBtn.addEventListener("click", () => { void controller.confirmRemove(); });

  await controller.start();

  let unmounted = false;
  return () => {
    if (unmounted) return;
    unmounted = true;
    controller.dispose();
    addButton.removeEventListener("click", onAdd);
    article.remove();
    cleanupStyles();
  };
}