import type { Collection, Document } from "./contract.generated";
import type { DocumentsController } from "./controller";

const KIT = {
  primary: "teamarness-control teamarness-control--primary",
  secondary: "teamarness-control teamarness-control--secondary",
  field: "teamarness-field",
  input: "teamarness-input",
  notice: "teamarness-notice",
  noticeDanger: "teamarness-notice teamarness-notice--danger",
} as const;

const AVAILABILITY_LABEL = {
  current: "Up to date",
  catching_up: "Catching up",
  offline: "Offline",
  waiting_for_history_access: "Waiting for history access",
} as const;

function button(label: string, className: string, onClick: () => void): HTMLButtonElement {
  const element = document.createElement("button");
  element.type = "button";
  element.className = className;
  element.textContent = label;
  element.addEventListener("click", onClick);
  return element;
}

/** A one-field form built once; the view only patches its disabled state. */
function inlineForm(options: {
  label: string;
  placeholder: string;
  submitLabel: string;
  maxLength: number;
  className: string;
  onSubmit: (value: string) => Promise<boolean>;
}): { form: HTMLFormElement; input: HTMLInputElement; submit: HTMLButtonElement } {
  const form = document.createElement("form");
  form.className = `${options.className} ${KIT.field}`;
  const input = document.createElement("input");
  input.className = KIT.input;
  input.type = "text";
  input.required = true;
  input.maxLength = options.maxLength;
  input.placeholder = options.placeholder;
  input.setAttribute("aria-label", options.label);
  const submit = document.createElement("button");
  submit.type = "submit";
  submit.className = KIT.primary;
  submit.textContent = options.submitLabel;
  form.append(input, submit);
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const value = input.value.trim();
    if (!value) return;
    void options.onSubmit(value).then((ok) => {
      if (ok) input.value = "";
    });
  });
  return { form, input, submit };
}

/**
 * Builds the Documents DOM once and updates regions in place. The create forms persist across
 * renders so a remote change never discards what a Member is typing.
 */
export class DocumentsView {
  private readonly page: HTMLElement;
  private readonly status: HTMLElement;
  private readonly warnings: HTMLElement;
  private readonly error: HTMLElement;
  private readonly collectionList: HTMLUListElement;
  private readonly collectionForm: ReturnType<typeof inlineForm>;
  private readonly documentForm: ReturnType<typeof inlineForm>;
  private readonly panelHeader: HTMLElement;
  private readonly documentList: HTMLUListElement;
  private readonly archivedToggle: HTMLInputElement;
  private readonly controller: DocumentsController;
  private renamingDocumentId: string | null = null;
  private renamingCollection = false;

  constructor(root: ShadowRoot | HTMLElement, controller: DocumentsController, title: string) {
    this.controller = controller;
    this.page = document.createElement("div");
    this.page.className = "teamarness-page documents-page";

    const header = document.createElement("header");
    header.className = "teamarness-page-header";
    const heading = document.createElement("h1");
    heading.className = "teamarness-page-title";
    heading.textContent = title;
    this.status = document.createElement("p");
    this.status.className = "availability";
    this.status.setAttribute("role", "status");
    const archivedLabel = document.createElement("label");
    archivedLabel.className = "archived-toggle";
    this.archivedToggle = document.createElement("input");
    this.archivedToggle.type = "checkbox";
    this.archivedToggle.addEventListener("change", () => controller.toggleArchived());
    archivedLabel.append(this.archivedToggle, document.createTextNode(" Show archived"));
    header.append(heading, this.status, archivedLabel);

    this.warnings = document.createElement("div");
    this.warnings.className = "documents-warnings";
    this.error = document.createElement("p");
    this.error.className = KIT.noticeDanger;
    this.error.setAttribute("role", "alert");
    this.error.hidden = true;

    const shell = document.createElement("div");
    shell.className = "documents-shell";

    const rail = document.createElement("aside");
    rail.className = "collection-rail";
    rail.setAttribute("aria-label", "Collections");
    const railHeading = document.createElement("h2");
    railHeading.textContent = "Collections";
    this.collectionList = document.createElement("ul");
    this.collectionList.className = "collection-list";
    this.collectionForm = inlineForm({
      label: "New Collection name",
      placeholder: "New Collection",
      submitLabel: "Add",
      maxLength: 128,
      className: "collection-create",
      onSubmit: (name) => controller.createCollection(name),
    });
    rail.append(railHeading, this.collectionList, this.collectionForm.form);

    const panel = document.createElement("section");
    panel.className = "documents-panel";
    this.panelHeader = document.createElement("div");
    this.panelHeader.className = "documents-panel-header";
    this.documentList = document.createElement("ul");
    this.documentList.className = "document-list";
    this.documentList.setAttribute("aria-label", "Documents");
    this.documentForm = inlineForm({
      label: "New Document title",
      placeholder: "New Document title",
      submitLabel: "Create",
      maxLength: 256,
      className: "document-create",
      onSubmit: (title) => controller.createDocument(title),
    });
    panel.append(this.panelHeader, this.documentList, this.documentForm.form);

    shell.append(rail, panel);
    this.page.append(header, this.warnings, this.error, shell);
    root.append(this.page);
  }

  render(): void {
    const { snapshot, busy, error, showArchived } = this.controller.state;
    this.status.textContent = AVAILABILITY_LABEL[snapshot.availability];
    this.archivedToggle.checked = showArchived;
    this.warnings.replaceChildren(
      ...(snapshot.warnings ?? []).map((text) => {
        const notice = document.createElement("p");
        notice.className = KIT.notice;
        notice.textContent = text;
        return notice;
      }),
    );
    this.error.hidden = error === null;
    this.error.textContent = error ?? "";
    this.collectionForm.submit.disabled = busy;
    this.renderCollections();
    this.renderPanel();
    this.documentForm.submit.disabled = busy || this.controller.selectedCollection === null;
    this.documentForm.form.hidden = this.controller.selectedCollection === null;
  }

  dispose(): void {
    this.page.remove();
  }

  private renderCollections(): void {
    const selected = this.controller.state.selectedCollectionId;
    this.collectionList.replaceChildren(
      ...this.controller.visibleCollections.map((collection) => {
        const item = document.createElement("li");
        const select = button(collection.name, "collection-button", () => {
          this.renamingCollection = false;
          this.controller.selectCollection(collection.id);
        });
        if (collection.archived) select.classList.add("is-archived");
        if (collection.id === selected) select.setAttribute("aria-current", "page");
        item.append(select);
        return item;
      }),
    );
  }

  private renderPanel(): void {
    const collection = this.controller.selectedCollection;
    this.panelHeader.replaceChildren();
    if (collection === null) {
      const empty = document.createElement("p");
      empty.className = "documents-empty";
      empty.textContent = this.controller.state.snapshot.collections.length === 0
        ? "Create a Collection to start adding documents."
        : "Select a Collection.";
      this.panelHeader.append(empty);
      this.documentList.replaceChildren();
      return;
    }
    const busy = this.controller.state.busy;
    if (this.renamingCollection) {
      const rename = inlineForm({
        label: "Collection name",
        placeholder: "Collection name",
        submitLabel: "Save",
        maxLength: 128,
        className: "collection-rename",
        onSubmit: async (name) => {
          const ok = await this.controller.renameCollection(collection.id, name);
          if (ok) this.renamingCollection = false;
          this.render();
          return ok;
        },
      });
      rename.input.value = collection.name;
      rename.form.append(button("Cancel", KIT.secondary, () => { this.renamingCollection = false; this.render(); }));
      this.panelHeader.append(rename.form);
    } else {
      const name = document.createElement("h2");
      name.className = "collection-name";
      name.textContent = collection.name + (collection.archived ? " (archived)" : "");
      const actions = document.createElement("div");
      actions.className = "collection-actions";
      actions.append(
        button("Rename", KIT.secondary, () => { this.renamingCollection = true; this.render(); }),
        collection.archived
          ? button("Restore", KIT.secondary, () => { void this.controller.restoreCollection(collection.id); })
          : button("Archive", KIT.secondary, () => { void this.controller.archiveCollection(collection.id); }),
      );
      for (const control of actions.querySelectorAll("button")) control.disabled = busy;
      this.panelHeader.append(name, actions);
    }
    this.documentList.replaceChildren(...this.controller.visibleDocuments.map((d) => this.documentRow(d, collection)));
    if (this.controller.visibleDocuments.length === 0) {
      const empty = document.createElement("li");
      empty.className = "documents-empty";
      empty.textContent = "No documents yet.";
      this.documentList.append(empty);
    }
  }

  private documentRow(document_: Document, collection: Collection): HTMLLIElement {
    const row = document.createElement("li");
    row.className = "document-row";
    if (document_.archived) row.classList.add("is-archived");
    const busy = this.controller.state.busy;
    if (this.renamingDocumentId === document_.id) {
      const rename = inlineForm({
        label: "Document title",
        placeholder: "Document title",
        submitLabel: "Save",
        maxLength: 256,
        className: "document-rename",
        onSubmit: async (title) => {
          const ok = await this.controller.renameDocument(document_.id, title);
          if (ok) this.renamingDocumentId = null;
          this.render();
          return ok;
        },
      });
      rename.input.value = document_.title;
      rename.form.append(button("Cancel", KIT.secondary, () => { this.renamingDocumentId = null; this.render(); }));
      row.append(rename.form);
      return row;
    }
    const title = document.createElement("span");
    title.className = "document-title";
    title.textContent = document_.title + (document_.archived ? " (archived)" : "");
    const actions = document.createElement("div");
    actions.className = "document-actions";
    actions.append(
      button("Rename", KIT.secondary, () => { this.renamingDocumentId = document_.id; this.render(); }),
      document_.archived
        ? button("Restore", KIT.secondary, () => { void this.controller.restoreDocument(document_.id); })
        : button("Archive", KIT.secondary, () => { void this.controller.archiveDocument(document_.id); }),
    );
    const destinations = this.controller.state.snapshot.collections.filter((c) => !c.archived && c.id !== collection.id);
    if (destinations.length > 0) {
      const move = document.createElement("select");
      move.className = `${KIT.input} document-move`;
      move.setAttribute("aria-label", `Move ${document_.title} to`);
      const placeholder = document.createElement("option");
      placeholder.value = "";
      placeholder.textContent = "Move to…";
      move.append(placeholder);
      for (const destination of destinations) {
        const option = document.createElement("option");
        option.value = destination.id;
        option.textContent = destination.name;
        move.append(option);
      }
      move.addEventListener("change", () => {
        if (move.value) void this.controller.moveDocument(document_.id, move.value);
      });
      move.disabled = busy;
      actions.append(move);
    }
    for (const control of actions.querySelectorAll("button")) control.disabled = busy;
    row.append(title, actions);
    return row;
  }
}
