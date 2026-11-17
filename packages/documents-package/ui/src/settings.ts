import {
  describePackageError,
  isPackageRequestError,
  type PackageMountContext,
  type PackageSubscription,
  type PackageUnmount,
  type TypedPackageClient,
} from "@teamarness/package-ui-sdk";
import type { DocumentsContract, FolderMapping, MappingDraft, MappingRefusal, MappingReport } from "./contract.generated";
import { icon } from "./view";

type Listed = DocumentsContract["requests"]["documents.mapping.list"]["response"]["collections"][number];
type SetRequest = DocumentsContract["requests"]["documents.mapping.set"]["request"];

/** How long the remembered draft waits for more typing before it is saved. */
export const DRAFT_SAVE_DELAY_MS = 150;

const KIT = {
  primary: "teamarness-control teamarness-control--primary",
  secondary: "teamarness-control teamarness-control--secondary",
  field: "teamarness-field",
  input: "teamarness-input",
  notice: "teamarness-notice",
  noticeDanger: "teamarness-notice teamarness-notice--danger",
} as const;

/** Why `documents.mapping.set` refused a folder, as the Member reads it. */
export const MAPPING_REFUSAL_TEXT: Readonly<Record<string, string>> = {
  not_mapped: "This Collection is no longer mapped.",
  relocation_blocked: "A required file relocation is blocked. Nothing was enabled. Preview again after resolving the refused paths.",
  not_absolute: "Enter an absolute path that starts with /.",
  not_found: "No folder exists at this path.",
  not_a_directory: "This path is a file, not a folder.",
  unreadable: "Teamarness cannot read this folder.",
  inside_data_directory: "This folder is inside the Teamarness data folder. Choose another folder.",
  overlapping_mapping: "This folder is mapped already, or it contains or is inside a mapped folder.",
  not_empty: "The folder and the Collection both have content. Map an empty folder, or create a new Collection from this folder.",
  git_working_tree: "This folder is inside a git working tree. Confirm the warning to map it.",
};

/** Why folder sync did not import a file, as the Member reads it. */
export const FILE_REFUSAL_TEXT: Readonly<Record<MappingRefusal["reason"], string>> = {
  lossy: "Importing it would lose text",
  too_large: "Larger than a Document can be",
  not_utf8: "Not UTF-8 text",
  schema_violation: "Uses Markdown that Documents cannot hold",
  merge_refused: "Its change could not be merged",
  symlink: "A symbolic link, which is never followed",
  title_too_long: "Its name is too long for a Document title",
  name_collision_unresolved: "Another file has the same name, ignoring case",
  invalid_folder: "This folder path cannot be synced to this filesystem",
  unreadable: "Cannot be read",
};

const STATE_TEXT: Readonly<Record<FolderMapping["state"], string>> = {
  active: "Synced",
  unavailable: "Folder unavailable",
  paused: "Paused while the Collection is archived",
};

/** The value of the Collection selector that means "Create Collection from folder". */
const NEW_COLLECTION = "";

function element<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string, text?: string): HTMLElementTagNameMap[K] {
  const created = document.createElement(tag);
  if (className) created.className = className;
  if (text !== undefined) created.textContent = text;
  return created;
}

function button(label: string, className: string, onClick: () => void): HTMLButtonElement {
  const created = element("button", className, label);
  created.type = "button";
  created.addEventListener("click", onClick);
  return created;
}

function plural(count: number, one: string, many = `${one}s`): string {
  return `${count} ${count === 1 ? one : many}`;
}

/** The Member-facing text of a failed mapping request. */
export function mappingErrorText(error: unknown): string {
  if (isPackageRequestError(error) && error.packageError?.code === "documents.mapping_refused") {
    const reason = error.packageError.details.reason;
    if (typeof reason === "string" && reason in MAPPING_REFUSAL_TEXT) return MAPPING_REFUSAL_TEXT[reason]!;
  }
  return `The request failed: ${describePackageError(error)}`;
}

function refusalList(refused: readonly MappingRefusal[], total: number): HTMLElement {
  const list = element("ul", "mapping-refused-list");
  for (const file of refused) {
    const item = element("li");
    item.append(element("code", undefined, file.name), document.createTextNode(`: ${FILE_REFUSAL_TEXT[file.reason]}`));
    if (file.detail) item.append(element("span", "mapping-detail", ` (${file.detail})`));
    list.append(item);
  }
  if (total > refused.length) list.append(element("li", "teamarness-help", `and ${total - refused.length} more`));
  return list;
}

function nameList(className: string, names: readonly string[], total: number): HTMLElement {
  const list = element("ul", className);
  for (const name of names) {
    const item = element("li");
    item.append(element("code", undefined, name));
    list.append(item);
  }
  if (total > names.length) list.append(element("li", "teamarness-help", `and ${total - names.length} more`));
  return list;
}

function appendReport(container: HTMLElement, report: MappingReport, summary: string): void {
  container.append(element("h3", "teamarness-section-title", "Preview"), element("p", "mapping-summary", summary));
  if (report.files.length > 0) container.append(nameList("mapping-files", report.files.map((file) => file.name), report.counts.files));
  if (report.counts.refused > 0) {
    container.append(element("h4", undefined, `${plural(report.counts.refused, "file")} will not be imported`), refusalList(report.refused, report.counts.refused));
  }
  if (report.counts.reformatted > 0) {
    container.append(
      element("h4", undefined, `${plural(report.counts.reformatted, "file")} will be reformatted`),
      element("p", "teamarness-help", "Teamarness writes these files in its canonical Markdown the first time it writes back an edit. Their text does not change."),
      nameList("mapping-reformatted", report.reformatted, report.counts.reformatted),
    );
  }
  if (report.counts.subfolders > 0) {
    container.append(
      element("h4", undefined, `${plural(report.counts.subfolders, "subfolder")} not synced`),
      nameList("mapping-subfolders", report.subfolders, report.counts.subfolders),
    );
  }
  if (report.counts.relocations > 0) container.append(
    element("h4", undefined, `${plural(report.counts.relocations, "planned relocation")}`),
    nameList("mapping-relocations", report.relocations.map((move) => `${move.from ?? "Export"} → ${move.to}`), report.counts.relocations),
  );
}

/**
 * One settings group, laid out like the Shell's About groups: its title and intro on the left,
 * its content on the right, and a rule above every group but the first.
 */
function group(id: string, title: string, intro: string, ...body: Node[]): HTMLElement {
  const section = element("section", "settings-group");
  section.setAttribute("aria-labelledby", `settings-${id}-heading`);
  const lead = element("div", "settings-group-intro");
  const heading = element("h2", undefined, title);
  heading.id = `settings-${id}-heading`;
  lead.append(heading, element("p", undefined, intro));
  const content = element("div", "settings-group-body");
  content.append(...body);
  section.append(lead, content);
  return section;
}

/**
 * The Documents settings view (the Shell's **Configuration** tab): every Collection with its
 * folder mapping, and a form that previews a folder, then maps it or creates a Collection from
 * it. The form's unsent path and name survive a Package switch in the Backend's memory.
 */
class MappingSettings {
  private readonly client: TypedPackageClient<DocumentsContract>;
  readonly article = element("article", "teamarness-page teamarness-page--readable documents-settings");
  private readonly list = element("ul", "mapping-collections");
  private readonly listStatus = element("p", "teamarness-help");
  private readonly path = element("input", KIT.input);
  private readonly target = element("select", KIT.input);
  private readonly nameField = element("label", KIT.field);
  private readonly name = element("input", KIT.input);
  private readonly preview = button("Preview", KIT.primary, () => void this.runPreview());
  private readonly map = button("Map folder", KIT.secondary, () => void this.runMap());
  /** Why Map or Create is not available yet, next to the buttons. */
  private readonly hint = element("p", "teamarness-help mapping-hint");
  private readonly report = element("section", "mapping-report");
  private readonly gitField = element("label", "mapping-git-confirm");
  private readonly gitConfirm = element("input");
  private readonly gitText = element("span");
  private readonly error = element("p", KIT.noticeDanger);
  private readonly status = element("p", KIT.notice);
  private collections: readonly Listed[] = [];
  private previewed: { key: string; report: MappingReport; direction: string } | null = null;
  private readonly recursivePanel = element("section", "mapping-recursive-preview");
  private recursivePreview: { collection: Listed; report: MappingReport } | null = null;
  private recursiveGitConfirmed = false;
  private busy = false;
  private disposed = false;
  private readonly openRefusals = new Set<string>();
  private draftTimer: ReturnType<typeof setTimeout> | undefined;
  private lastSavedDraft = "{}";
  private refreshing: Promise<void> | null = null;
  private refreshAgain = false;
  /** The remembered target, applied once the Collections it may name are listed. */
  private pendingTarget: string | null = null;

  constructor(client: TypedPackageClient<DocumentsContract>) {
    this.client = client;
    // The Shell's page header and Configuration tab already name the Package and the view.
    const description = element("p", "settings-description",
      "Map a Collection to a folder of Markdown files on this computer. Teamarness keeps the files and the Documents in sync. Mappings are not shared with the Workspace.");

    const collections = group("collections", "Collections", "Each Collection and the folder it is mapped to.", this.listStatus, this.list, this.recursivePanel);

    const pathField = element("label", KIT.field, "Folder path");
    this.path.type = "text";
    this.path.placeholder = "/absolute/path/to/folder";
    this.path.setAttribute("aria-label", "Folder path");
    this.path.spellcheck = false;
    pathField.append(this.path);
    const targetField = element("label", KIT.field, "Collection");
    this.target.setAttribute("aria-label", "Collection");
    targetField.append(this.target);
    this.nameField.textContent = "New Collection name";
    this.name.type = "text";
    this.name.maxLength = 128;
    this.name.setAttribute("aria-label", "New Collection name");
    this.nameField.append(this.name);
    this.gitConfirm.type = "checkbox";
    this.gitField.append(this.gitConfirm, this.gitText);
    const actions = element("div", "teamarness-inline mapping-actions");
    actions.append(this.preview, this.map);
    this.hint.id = "mapping-hint";
    this.map.setAttribute("aria-describedby", this.hint.id);
    this.error.setAttribute("role", "alert");
    this.status.setAttribute("role", "status");
    const form = group("map", "Map a folder",
      "Preview the folder first. Teamarness lists the files it will import, the files it will refuse and why, and the files it will reformat when it first writes them back. New mappings include subfolders. Imported files are shared with the Workspace.",
      pathField, targetField, this.nameField, actions, this.hint, this.error, this.status, this.report, this.gitField);
    form.classList.add("mapping-form");

    this.article.append(description, collections, form);
    this.path.addEventListener("input", () => this.edited());
    this.name.addEventListener("input", () => this.edited());
    this.target.addEventListener("change", () => this.edited());
    this.gitConfirm.addEventListener("change", () => this.renderForm());
    this.renderList();
    this.renderForm();
  }

  /** Restores the remembered draft, then lists the Collections. */
  async initialize(): Promise<void> {
    try {
      const draft = await this.client.request("documents.mapping.draft.get", {});
      this.path.value = draft.path ?? "";
      this.name.value = draft.name ?? "";
      this.pendingTarget = draft.collectionId ?? NEW_COLLECTION;
      this.lastSavedDraft = JSON.stringify(draft);
    } catch {
      // A draft is a convenience: the form starts empty without one.
    }
    await this.refresh();
  }

  /** Lists the Collections again. Overlapping calls coalesce into one more listing. */
  refresh(): Promise<void> {
    if (this.refreshing) {
      this.refreshAgain = true;
      return this.refreshing;
    }
    this.refreshing = (async () => {
      do {
        this.refreshAgain = false;
        try {
          const listed = await this.client.request("documents.mapping.list", {});
          if (this.disposed) return;
          this.collections = listed.collections;
          this.listStatus.textContent = "";
        } catch (error) {
          if (this.disposed) return;
          this.listStatus.textContent = `The Collections could not be listed: ${describePackageError(error)}`;
        }
        this.renderList();
        this.renderForm();
      } while (this.refreshAgain && !this.disposed);
    })().finally(() => { this.refreshing = null; });
    return this.refreshing;
  }

  dispose(): void {
    this.disposed = true;
    if (this.draftTimer !== undefined) {
      clearTimeout(this.draftTimer);
      this.saveDraft();
    }
    this.article.remove();
  }

  // ------------------------------------------------------------ list

  private renderList(): void {
    const shown = this.collections.filter((collection) => !collection.archived || collection.mapping !== null);
    this.list.replaceChildren();
    if (shown.length === 0) {
      this.list.append(element("li", "teamarness-help", "No Collections yet. Create one from a folder below."));
      return;
    }
    for (const collection of shown) this.list.append(this.collectionItem(collection));
  }

  private collectionItem(collection: Listed): HTMLLIElement {
    const item = element("li", "mapping-collection");
    item.dataset.collectionId = collection.collectionId;
    const heading = element("div", "mapping-collection-header");
    heading.append(element("strong", "mapping-collection-name", collection.name));
    const mapping = collection.mapping;
    heading.append(element("span", "mapping-state", mapping === null ? "Not mapped" : STATE_TEXT[mapping.state]));
    item.append(heading);
    if (mapping === null) return item;
    item.dataset.state = mapping.state;
    const unmap = icon("unlink", "Unmap", () => void this.unmap(collection), true);
    unmap.classList.add("mapping-unmap");
    unmap.setAttribute("aria-label", `Unmap ${collection.name}`);
    unmap.disabled = this.busy;
    heading.append(unmap);
    item.append(element("code", "mapping-path", mapping.path));
    const counts = [
      plural(mapping.counts.files, "file"),
      `${mapping.counts.refused} refused`,
      ...(!mapping.recursive && mapping.counts.subfolders > 0 ? [`${plural(mapping.counts.subfolders, "subfolder")} not synced yet`] : []),
    ];
    item.append(element("p", "mapping-counts teamarness-help", counts.join(" · ")));
    item.append(element("p", "mapping-mode", mapping.recursive ? "Subfolders synced" : "Subfolders not synced"));
    if (!mapping.recursive) {
      if (mapping.pendingFolderChanges > 0) item.append(element("p", "teamarness-help", `${plural(mapping.pendingFolderChanges, "folder change")} waiting for confirmation.`));
      const enable = button("Enable subfolders", KIT.secondary, () => void this.previewRecursive(collection));
      enable.disabled = this.busy;
      item.append(enable);
    }
    if (mapping.gitRoot !== null) {
      item.append(element("p", "mapping-git teamarness-help", `Inside the git working tree ${mapping.gitRoot}.`));
    }
    if (mapping.counts.refused > 0) {
      const details = element("details", "mapping-refused");
      details.open = this.openRefusals.has(collection.collectionId);
      details.addEventListener("toggle", () => {
        if (details.open) this.openRefusals.add(collection.collectionId);
        else this.openRefusals.delete(collection.collectionId);
      });
      details.append(element("summary", undefined, `${plural(mapping.counts.refused, "refused file")}`), refusalList(mapping.refused, mapping.counts.refused));
      item.append(details);
    }
    return item;
  }

  private renderRecursivePreview(): void {
    this.recursivePanel.replaceChildren();
    const preview = this.recursivePreview;
    if (!preview) return;
    const { collection, report } = preview;
    this.recursivePanel.append(element("h3", undefined, `Enable subfolders for ${collection.name}`),
      element("p", undefined, "Files imported from subfolders are shared with the Workspace. This enables subfolders only on this Installation. Existing file associations are kept."));
    appendReport(this.recursivePanel, report, `${plural(report.counts.files, "new file")} will be imported.`);
    if (report.blocked) this.recursivePanel.append(element("p", KIT.noticeDanger, "Required relocations are blocked. Resolve the refused paths, then preview again. Subfolders remain disabled."));
    if (report.gitRoot !== null) {
      const label = element("label", "mapping-git-confirm");
      const check = element("input");
      check.type = "checkbox";
      check.checked = this.recursiveGitConfirmed;
      check.setAttribute("aria-label", "Enable subfolders inside a git working tree");
      check.addEventListener("change", () => { this.recursiveGitConfirmed = check.checked; this.renderRecursivePreview(); });
      label.append(check, document.createTextNode(` Inside the git working tree ${report.gitRoot}. Git pulls, checkouts and merges become edits shared with the Workspace. Enable subfolders anyway.`));
      this.recursivePanel.append(label);
    }
    const confirm = button("Confirm enable subfolders", KIT.primary, () => void this.confirmRecursive());
    confirm.disabled = this.busy || report.blocked || (report.gitRoot !== null && !this.recursiveGitConfirmed);
    const cancel = button("Cancel", KIT.secondary, () => { this.recursivePreview = null; this.renderRecursivePreview(); });
    cancel.disabled = this.busy;
    const actions = element("div", "teamarness-inline mapping-actions");
    actions.append(confirm, cancel);
    this.recursivePanel.append(actions);
  }

  private async previewRecursive(collection: Listed): Promise<void> {
    this.recursivePreview = null;
    this.recursiveGitConfirmed = false;
    this.error.textContent = "";
    this.setBusy(true);
    try {
      const answer = await this.client.request("documents.mapping.enable-recursive", { collectionId: collection.collectionId, dryRun: true });
      if (this.disposed) return;
      if (answer.report) this.recursivePreview = { collection, report: answer.report };
      else await this.refresh();
    } catch (error) {
      this.error.textContent = mappingErrorText(error);
    } finally {
      this.setBusy(false);
    }
  }

  private async confirmRecursive(): Promise<void> {
    const preview = this.recursivePreview;
    if (!preview || preview.report.blocked) return;
    this.error.textContent = "";
    this.setBusy(true);
    try {
      await this.client.request("documents.mapping.enable-recursive", { collectionId: preview.collection.collectionId, dryRun: false, confirmGit: this.recursiveGitConfirmed });
      if (this.disposed) return;
      this.status.textContent = `Subfolders enabled for ${preview.collection.name}. Imported files are shared with the Workspace.`;
      this.recursivePreview = null;
    } catch (error) {
      this.error.textContent = mappingErrorText(error);
      this.recursivePreview = null;
    } finally {
      this.setBusy(false);
    }
    await this.refresh();
  }

  private async unmap(collection: Listed): Promise<void> {
    this.setBusy(true);
    this.error.textContent = "";
    try {
      await this.client.request("documents.mapping.clear", { collectionId: collection.collectionId });
      this.status.textContent = `${collection.name} is no longer mapped. Its files stay in the folder.`;
    } catch (error) {
      this.error.textContent = `${collection.name} could not be unmapped: ${describePackageError(error)}`;
    } finally {
      this.setBusy(false);
    }
    await this.refresh();
  }

  // ------------------------------------------------------------ form

  /** The Collections a folder can be mapped to: unarchived and not mapped yet. */
  private mappable(): readonly Listed[] {
    return this.collections.filter((collection) => !collection.archived && collection.mapping === null);
  }

  private renderForm(): void {
    const selected = this.pendingTarget ?? this.target.value;
    const options = [{ value: NEW_COLLECTION, label: "New Collection from this folder" },
      ...this.mappable().map((collection) => ({ value: collection.collectionId, label: collection.name }))];
    const current = [...this.target.options].map((option) => `${option.value}\u0000${option.text}`).join("\u0001");
    const next = options.map((option) => `${option.value}\u0000${option.label}`).join("\u0001");
    if (current !== next) {
      this.target.replaceChildren(...options.map((option) => {
        const created = element("option", undefined, option.label);
        created.value = option.value;
        return created;
      }));
    }
    const exists = options.some((option) => option.value === selected);
    this.target.value = exists ? selected : NEW_COLLECTION;
    if (this.pendingTarget !== null && (exists || this.collections.length > 0)) this.pendingTarget = null;
    if (this.previewed && this.previewed.key !== this.key()) this.previewed = null;

    const creating = this.target.value === NEW_COLLECTION;
    this.nameField.hidden = !creating;
    this.map.textContent = creating ? "Create Collection from folder" : "Map folder";
    this.preview.disabled = this.busy || this.path.value.trim() === "";
    const git = this.previewed?.report.gitRoot ?? null;
    this.gitField.hidden = git === null;
    if (git === null) this.gitConfirm.checked = false;
    this.gitText.textContent = git === null ? ""
      : ` This folder is inside the git working tree ${git}. Git pulls, checkouts and merges change these files, and every change becomes an edit for the whole team. Map it anyway.`;
    this.gitConfirm.setAttribute("aria-label", "Map a folder inside a git working tree");
    // Preview is the next step until a preview exists; then Map or Create is.
    const previewed = this.previewed !== null;
    this.preview.className = previewed ? KIT.secondary : KIT.primary;
    this.map.className = previewed ? KIT.primary : KIT.secondary;
    const action = creating ? "Create Collection from folder" : "Map folder";
    const waiting = !previewed
      ? this.path.value.trim() === "" ? `Enter a folder path and Preview it to enable ${action}.` : `Preview the folder to enable ${action}.`
      : creating && this.name.value.trim() === "" ? `Name the new Collection to enable ${action}.`
        : git !== null && !this.gitConfirm.checked ? `Confirm the git warning below to enable ${action}.`
          : "";
    this.hint.textContent = waiting;
    this.hint.hidden = waiting === "";
    this.map.disabled = this.busy || waiting !== "";
    this.renderReport();
  }

  /** What a preview was made for: a changed path or Collection needs a new preview. */
  private key(): string {
    return JSON.stringify([this.path.value.trim(), this.target.value]);
  }

  private renderReport(): void {
    this.report.replaceChildren();
    const previewed = this.previewed;
    if (previewed === null) return;
    const { report } = previewed;
    const summary = previewed.direction === "export"
      ? "The folder has no Markdown files. Every Document of the Collection will be written to it."
      : previewed.direction === "unchanged"
        ? "This Collection is mapped to this folder already."
        : `${plural(report.counts.files, "file")} will be imported.`;
    appendReport(this.report, report, summary);
  }

  private request(dryRun: boolean): SetRequest {
    const path = this.path.value.trim();
    const target = this.target.value === NEW_COLLECTION
      ? { newCollectionName: this.name.value.trim() || "Untitled" }
      : { collectionId: this.target.value };
    return { ...target, path, dryRun, ...(dryRun ? {} : { confirmGit: this.gitConfirm.checked }) };
  }

  private async runPreview(): Promise<void> {
    const key = this.key();
    this.setBusy(true);
    this.error.textContent = "";
    this.status.textContent = "";
    this.previewed = null;
    try {
      const answer = await this.client.request("documents.mapping.set", this.request(true));
      if (this.disposed) return;
      if (answer.report && key === this.key()) this.previewed = { key, report: answer.report, direction: answer.direction };
    } catch (error) {
      this.error.textContent = mappingErrorText(error);
    } finally {
      this.setBusy(false);
    }
  }

  private async runMap(): Promise<void> {
    const creating = this.target.value === NEW_COLLECTION;
    const label = creating ? this.name.value.trim() : this.target.selectedOptions[0]?.text ?? "The Collection";
    this.setBusy(true);
    this.error.textContent = "";
    try {
      const answer = await this.client.request("documents.mapping.set", this.request(false));
      if (this.disposed) return;
      this.status.textContent = `${label} is mapped to ${answer.path}.`;
      this.previewed = null;
      this.path.value = "";
      this.name.value = "";
      this.pendingTarget = NEW_COLLECTION;
      this.scheduleDraft();
    } catch (error) {
      this.error.textContent = mappingErrorText(error);
    } finally {
      this.setBusy(false);
    }
    await this.refresh();
  }

  private setBusy(busy: boolean): void {
    this.busy = busy;
    this.renderRecursivePreview();
    this.renderList();
    this.renderForm();
  }

  private edited(): void {
    this.error.textContent = "";
    this.status.textContent = "";
    this.renderForm();
    this.scheduleDraft();
  }

  // ------------------------------------------------------------ remembered draft

  private draft(): MappingDraft {
    const target = this.pendingTarget ?? this.target.value;
    return {
      ...(this.path.value ? { path: this.path.value.slice(0, 4096) } : {}),
      ...(this.name.value ? { name: this.name.value.slice(0, 128) } : {}),
      ...(target !== NEW_COLLECTION ? { collectionId: target } : {}),
    };
  }

  private scheduleDraft(): void {
    if (this.draftTimer !== undefined) clearTimeout(this.draftTimer);
    this.draftTimer = setTimeout(() => this.saveDraft(), DRAFT_SAVE_DELAY_MS);
  }

  private saveDraft(): void {
    this.draftTimer = undefined;
    const draft = this.draft();
    const saved = JSON.stringify(draft);
    if (saved === this.lastSavedDraft) return;
    this.lastSavedDraft = saved;
    // Losing a draft loses only typing: a failure is not shown.
    this.client.request("documents.mapping.draft.set", draft).catch(() => { this.lastSavedDraft = ""; });
  }
}

/** Mounts the settings view the manifest declares in `ui.views`. */
export async function mountSettings(
  context: PackageMountContext,
  client: TypedPackageClient<DocumentsContract>,
  cleanupStyles: () => void,
): Promise<PackageUnmount> {
  const settings = new MappingSettings(client);
  context.root.append(settings.article);
  let subscription: PackageSubscription | null = null;
  try {
    await settings.initialize();
    // Every change to Collections, Documents or mapping state is a reason to list again.
    subscription = await client.subscribe("documents.changes", {}, () => { void settings.refresh(); });
  } catch (error) {
    settings.dispose();
    cleanupStyles();
    throw error;
  }
  let unmounted = false;
  return async () => {
    if (unmounted) return;
    unmounted = true;
    settings.dispose();
    try {
      await subscription?.close();
    } finally {
      cleanupStyles();
    }
  };
}
